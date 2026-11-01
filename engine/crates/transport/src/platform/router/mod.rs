//! The platform router: receiving, unknown or unsubscribed routes, sending a request,
//! `/internal/response` and rejection, and the message type and envelope header rules.
//!
//! One router per engine core serves every partner. It owns the route registry (one handler per route),
//! the pending-request table, response correlation for Platform **and** App messages, and the
//! `NotSubscribedToRoute` rejection. The request-id counter is process-wide: every router of the
//! process — a core rebuilt after an account removal or a failed install included — draws from it, so
//! `_requestId` never restarts at `1` within a process. Reassembled inbound messages arrive from the
//! fragment layer through [`MessageSink`]; requests and responses leave through [`MessageTransport`].
//!
//! Inbound dispatch never blocks the caller: route handlers, the device-proxy receiver and rejections
//! run on their own tasks, each isolated so that a panic is logged and the session continues: one bad
//! frame never takes down the shared connection. The App hook ([`AppInbound`]) is called synchronously,
//! in arrival order.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use async_trait::async_trait;
use futures_util::FutureExt;
use tokio::sync::oneshot;
use tracing::{debug, error, warn};

use super::tap_decode::{self, BodyContext};
use super::version::parse_text_int;
use super::{priority, route};
use phonelink_core::config::EngineConfig;
use phonelink_core::seams::{
    AppInbound, MessageSink, MessageTransport, PlatformMessenger, PlatformRequest, PlatformRequestStatus,
    PlatformResponse,
};
use phonelink_core::sync::{MutexExt, RwLockExt};
use phonelink_core::tap::{Direction, Tap};
use phonelink_core::types::{
    DcgClient, DcgSendStatus, HeaderMap, MessageType, OutboundMessage, SendOptions, TraceContext, TransportMessage,
    header,
};

/// The only `_rejectedReason` the PC produces in an `/internal/response` rejection.
pub const NOT_SUBSCRIBED_TO_ROUTE: &str = "NotSubscribedToRoute";
/// `_rejectionVersion` on every request and response the PC sends.
pub const REJECTION_VERSION: &str = "1";

/// The process-wide `uint` request-id counter; the first minted id is `"1"`, and it wraps from
/// `u32::MAX` to `0`.
static NEXT_REQUEST_ID: AtomicU32 = AtomicU32::new(1);

/// Options of every response: `IgnoreDeviceDisabled | SkipWakeIfNotConnected`.
pub const RESPONSE_OPTIONS: SendOptions = SendOptions {
    one_way: false,
    ignore_device_disabled: true,
    skip_wake_if_not_connected: true,
    skip_partner_ack: false,
};

/// One inbound message as a route handler receives it.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub source: DcgClient,
    /// `Some` when it arrived through `OnReceiveSessionBasedMessage`; only device-proxy replies echo it.
    pub connection_session_id: Option<String>,
    pub trace: TraceContext,
    pub message: TransportMessage,
}

/// The single handler of one platform route (the registry maps `route string → single handler`).
#[async_trait]
pub trait PlatformRouteHandler: Send + Sync {
    async fn handle(&self, inbound: InboundMessage);
}

/// A route registration that breaks the registry's rules — a programming error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteRegistrationError {
    #[error("route {0:?} already has a handler (one handler per route)")]
    AlreadyRegistered(String),
    #[error("route {0:?} is reserved for responses")]
    Reserved(String),
    #[error("route {0:?} does not start with '/'")]
    Malformed(String),
}

struct Pending {
    /// The request's `_route`, to decode the response for the tap.
    route: String,
    tx: oneshot::Sender<TransportMessage>,
}

/// Removes a pending entry when its request finishes, whichever way.
struct PendingGuard<'a> {
    pending: &'a Mutex<HashMap<String, Pending>>,
    request_id: &'a str,
}

impl Drop for PendingGuard<'_> {
    // Recovers a poisoned lock rather than panic while unwinding; removing the entry is right for any
    // state the table was left in.
    fn drop(&mut self) {
        self.pending.lock_or_recover().remove(self.request_id);
    }
}

/// The platform router. Create it with [`Router::new`]; register routes with [`Router::register_route`].
pub struct Router {
    me: Weak<Router>,
    config: Arc<EngineConfig>,
    transport: Arc<dyn MessageTransport>,
    tap: Tap,
    pending: Mutex<HashMap<String, Pending>>,
    routes: RwLock<HashMap<String, Arc<dyn PlatformRouteHandler>>>,
    app_inbound: RwLock<Option<Arc<dyn AppInbound>>>,
    device_proxy: RwLock<Option<Arc<dyn PlatformRouteHandler>>>,
}

impl Router {
    pub fn new(config: Arc<EngineConfig>, transport: Arc<dyn MessageTransport>, tap: Tap) -> Arc<Self> {
        Arc::new_cyclic(|me| Router {
            me: me.clone(),
            config,
            transport,
            tap,
            pending: Mutex::default(),
            routes: RwLock::default(),
            app_inbound: RwLock::default(),
            device_proxy: RwLock::default(),
        })
    }

    /// Installs the App layer: every inbound App message that is neither a response nor device-proxy
    /// traffic. Until then such messages are logged and dropped.
    pub fn set_app_inbound(&self, inbound: Arc<dyn AppInbound>) {
        *self.app_inbound.write_or_panic() = Some(inbound);
    }

    /// Installs the receiver of App messages whose `_route` header is
    /// `/DeviceProxyClient/TransportMiddleware`, which takes them before the rest of inbound App dispatch.
    /// [`super::routes::PlatformRoutes::install`] sets it.
    pub fn set_device_proxy_receiver(&self, receiver: Arc<dyn PlatformRouteHandler>) {
        *self.device_proxy.write_or_panic() = Some(receiver);
    }

    /// Registers the single handler of a Platform route. A second registration of the same route, the
    /// response route, or a route not starting with `/` is a programming error.
    pub fn register_route(
        &self,
        route: &str,
        handler: Arc<dyn PlatformRouteHandler>,
    ) -> Result<(), RouteRegistrationError> {
        if route == header::RESPONSE_ROUTE {
            return Err(RouteRegistrationError::Reserved(route.to_owned()));
        }
        if !route.starts_with('/') {
            return Err(RouteRegistrationError::Malformed(route.to_owned()));
        }

        let mut routes = self.routes.write_or_panic();
        if routes.contains_key(route) {
            return Err(RouteRegistrationError::AlreadyRegistered(route.to_owned()));
        }

        routes.insert(route.to_owned(), handler);
        Ok(())
    }

    /// Removes the handler of `route` (an owner that goes away, such as a partner's context source);
    /// `false` when none was registered. Later messages on the route are rejected as unsubscribed.
    pub fn unregister_route(&self, route: &str) -> bool {
        self.routes.write_or_panic().remove(route).is_some()
    }

    pub fn is_route_registered(&self, route: &str) -> bool {
        self.routes.read_or_panic().contains_key(route)
    }

    /// Requests waiting for their `/internal/response` (diagnostics).
    pub fn pending_requests(&self) -> usize {
        self.pending.lock_or_panic().len()
    }

    /// A request that keeps the caller's `SkipWakeIfNotConnected` choice, so the send may wake the
    /// phone. Only the key-rotation requester sends this way; every other request goes through
    /// [`PlatformMessenger::request`].
    pub async fn request_allowing_wake(&self, request: PlatformRequest) -> PlatformResponse {
        self.send_request(request, false).await
    }

    /// Hands a fully built message to the fragment layer as is: no header is added and no response is
    /// awaited (device-proxy replies).
    pub(crate) async fn send_raw(&self, outbound: OutboundMessage) -> DcgSendStatus {
        self.transport.send(outbound).await
    }

    pub(crate) fn tap(&self) -> &Tap {
        &self.tap
    }

    fn tap_message(
        &self,
        direction: Direction,
        partner: &DcgClient,
        message: &TransportMessage,
        body: BodyContext<'_>,
    ) {
        self.tap.emit(tap_decode::message_event(direction, partner, message, body));
    }

    /// Sends a request: assign `_requestId`, add the request headers and options, send, and await the
    /// correlated response within the request timeout.
    async fn send_request(&self, request: PlatformRequest, force_skip_wake: bool) -> PlatformResponse {
        let PlatformRequest { target, mut message, priority, mut options, timeout, trace, cancel } = request;
        let route = match message.route() {
            Some(r) if !r.is_empty() => r.to_owned(),
            _ => panic!("a platform request must carry _route"),
        };
        assert_ne!(route, header::RESPONSE_ROUTE, "responses are sent with respond(), not request()");
        assert_content_type_rule(&message, "request");

        // Keep a pre-set _requestId, else the next counter value as a decimal string.
        let request_id = match message.headers.get(header::REQUEST_ID) {
            Some(id) => id.to_owned(),
            None => {
                let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed).to_string();
                message.headers = request_id_first(&message.headers, &id);
                id
            }
        };

        // _rejectionVersion on every request; the connection-level SkipWakeIfNotConnected.
        message.headers.insert(header::REJECTION_VERSION, REJECTION_VERSION);
        if force_skip_wake {
            options.skip_wake_if_not_connected = true;
        }

        // One PlatformRequestTimeout bounds send plus wait; a caller may ask for less.
        let deadline = tokio::time::Instant::now() + timeout.min(self.config.platform.request_timeout);
        if cancel.is_cancelled() {
            return status_only(PlatformRequestStatus::Canceled);
        }

        let (tx, mut rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock_or_panic();
            assert!(
                !pending.contains_key(&request_id),
                "_requestId {request_id:?} is already pending: a caller reused a request id"
            );
            pending.insert(request_id.clone(), Pending { route: route.clone(), tx });
        }
        let _entry = PendingGuard { pending: &self.pending, request_id: &request_id };

        self.tap_message(Direction::Out, &target, &message, BodyContext::Request);
        let outbound = OutboundMessage { target, message, priority, options, connection_session_id: None, trace };
        let send = self.transport.send(outbound);
        let sent = tokio::select! {
            biased;
            _ = cancel.cancelled() => return status_only(PlatformRequestStatus::Canceled),
            sent = tokio::time::timeout_at(deadline, send) => match sent {
                Ok(status) => status,
                Err(_) => return status_only(PlatformRequestStatus::ResponseTimedOut),
            },
        };

        if sent != DcgSendStatus::Success {
            return status_only(PlatformRequestStatus::RequestSendFailed(sent));
        }

        if options.one_way {
            if rx.try_recv().is_ok() {
                debug!(request_id, route, "a response to a one-way request arrived; dropped");
            }
            return status_only(PlatformRequestStatus::Success);
        }

        let reply = tokio::select! {
            biased;
            _ = cancel.cancelled() => return status_only(PlatformRequestStatus::Canceled),
            reply = tokio::time::timeout_at(deadline, &mut rx) => match reply {
                Ok(Ok(message)) => message,
                Ok(Err(_)) => unreachable!("the pending entry of {request_id:?} vanished while its request waited"),
                Err(_) => return status_only(PlatformRequestStatus::ResponseTimedOut),
            },
        };
        map_response(reply)
    }

    /// Correlates a received response of either message type.
    fn complete_response(&self, source: &DcgClient, message: TransportMessage) {
        let Some(original) = message.headers.get(header::ORIGINAL_REQUEST_ID).map(str::to_owned) else {
            if message.message_type == MessageType::Platform {
                self.tap_message(Direction::In, source, &message, BodyContext::Opaque);
            }
            warn!(partner = %source, raw_headers = ?message.headers, "response without _originalRequestId; dropped");
            return;
        };

        let entry = self.pending.lock_or_panic().remove(&original);
        if message.message_type == MessageType::Platform {
            let body = match &entry {
                Some(p) => BodyContext::ResponseTo(p.route.as_str()),
                None => BodyContext::Opaque,
            };
            self.tap_message(Direction::In, source, &message, body);
        }
        hand_over_response(source, &original, entry, message);
    }

    /// Handles a Platform message on an unknown or unsubscribed route: log it and, when the sender can
    /// take one, reply with a `NotSubscribedToRoute` rejection.
    fn reject_unsubscribed(&self, source: DcgClient, trace: TraceContext, message: TransportMessage) {
        let route = message.route().map(str::to_owned);

        // The body is not parsed; the headers are logged.
        if let Some(reason) = no_rejection_reason(&message) {
            warn!(
                partner = %source,
                raw_route = ?route,
                raw_headers = ?message.headers,
                payload_len = message.payload.len(),
                reason,
                "platform message on an unsubscribed or missing route; dropped without a rejection"
            );
            return;
        }

        warn!(
            partner = %source,
            raw_route = ?route,
            raw_headers = ?message.headers,
            payload_len = message.payload.len(),
            "platform message on an unsubscribed or missing route; rejected with NotSubscribedToRoute"
        );

        let rejection = TransportMessage::new(
            message.message_type,
            HeaderMap::from_pairs([
                (header::REJECTED_REASON, NOT_SUBSCRIBED_TO_ROUTE),
                (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_JSON),
            ])
            .expect("distinct header keys"),
            b"{}".to_vec(),
        );

        let Some(me) = self.me.upgrade() else {
            return;
        };

        spawn_isolated(format!("rejection of {route:?}"), async move {
            let status = me.respond(&source, &message, rejection, priority::KVP, trace).await;
            if status != DcgSendStatus::Success {
                warn!(partner = %source, ?status, error = status.cause(), "NotSubscribedToRoute rejection not sent");
            }
        });
    }
}

#[async_trait]
impl PlatformMessenger for Router {
    /// Sends a request and awaits its response. Every request gets the connection-level
    /// `SkipWakeIfNotConnected`, so a request never wakes the phone; see [`Router::request_allowing_wake`].
    ///
    /// Panics on a broken caller precondition: no `_route`, a Platform message without (or an App
    /// message with) `ms-content-type`, or a `_requestId` that is already pending.
    async fn request(&self, request: PlatformRequest) -> PlatformResponse {
        self.send_request(request, true).await
    }

    /// Sends a response to `request` on `/internal/response`. Panics when `request` carries no
    /// `_requestId` (the trait's precondition: callers check first and log-and-drop such requests).
    async fn respond(
        &self,
        target: &DcgClient,
        request: &TransportMessage,
        response: TransportMessage,
        priority: u32,
        trace: TraceContext,
    ) -> DcgSendStatus {
        let original = request
            .headers
            .get(header::REQUEST_ID)
            .unwrap_or_else(|| panic!("respond(): the request carries no _requestId"));

        assert_content_type_rule(&response, "response");

        let headers = response_headers(original, &response.headers);
        let message = TransportMessage::new(response.message_type, headers, response.payload);
        if message.message_type == MessageType::Platform {
            let answered = request.route().unwrap_or_default();
            self.tap_message(Direction::Out, target, &message, BodyContext::ResponseTo(answered));
        }

        let outbound = OutboundMessage {
            target: target.clone(),
            message,
            priority,
            options: RESPONSE_OPTIONS,
            connection_session_id: None,
            trace,
        };
        self.transport.send(outbound).await
    }
}

impl MessageSink for Router {
    /// Receiving: Platform → `/internal/response` correlation or the route registry (unknown route ⇒
    /// rejection rule); App → `/internal/response` by **header** only, the device-proxy receiver by
    /// `_route` header, else the App layer. Never blocks; call it inside the tokio runtime (handlers, the
    /// device-proxy receiver and rejections are spawned).
    fn on_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        let is_response = message.route() == Some(header::RESPONSE_ROUTE);
        if is_response {
            self.complete_response(&source, message);
            return;
        }

        match message.message_type {
            MessageType::Platform => self.route_platform_request(source, connection_session_id, trace, message),
            MessageType::App => {
                if message.route() == Some(route::DEVICE_PROXY) {
                    self.route_device_proxy_message(source, connection_session_id, trace, message);
                    return;
                }
                self.deliver_to_app_layer(source, connection_session_id, trace, message);
            }
        }
    }
}

impl Router {
    /// A Platform request: spawned to its route's handler, or rejected as unsubscribed.
    fn route_platform_request(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        self.tap_message(Direction::In, &source, &message, BodyContext::Request);

        let handler = message.route().and_then(|r| self.routes.read_or_panic().get(r).cloned());
        match handler {
            Some(handler) => {
                let what = format!("platform route {}", message.route().unwrap_or_default());
                let inbound = InboundMessage { source, connection_session_id, trace, message };
                spawn_isolated(what, async move { handler.handle(inbound).await });
            }
            None => self.reject_unsubscribed(source, trace, message),
        }
    }

    /// An App message on the device-proxy route: spawned to the device-proxy receiver, dropped before
    /// it is installed.
    fn route_device_proxy_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        let receiver = self.device_proxy.read_or_panic().clone();
        match receiver {
            Some(receiver) => {
                let inbound = InboundMessage { source, connection_session_id, trace, message };
                spawn_isolated("device-proxy receiver".into(), async move { receiver.handle(inbound).await });
            }
            None => {
                warn!(partner = %source, "device-proxy message before its receiver is installed; dropped")
            }
        }
    }

    /// Any other App message: handed to the App layer, a panic in it isolated.
    fn deliver_to_app_layer(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        let app = self.app_inbound.read_or_panic().clone();
        match app {
            Some(app) => {
                let route = message.route().map(str::to_owned);
                let delivered = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    app.on_app_message(source.clone(), connection_session_id, trace, message)
                }));

                if let Err(panic) = delivered {
                    error!(partner = %source, raw_route = ?route, panic = %panic_text(panic.as_ref()), "App layer panicked on an inbound message; isolated");
                }
            }
            None => {
                warn!(partner = %source, raw_route = ?message.route(), "App message and no App layer installed; dropped")
            }
        }
    }
}

/// Hands a correlated response to its waiting request; a response that arrived too late or matches
/// no pending request is dropped.
fn hand_over_response(source: &DcgClient, original: &str, entry: Option<Pending>, message: TransportMessage) {
    match entry {
        Some(pending) => {
            if pending.tx.send(message).is_err() {
                debug!(partner = %source, request_id = %original, "the request finished before its response arrived; dropped");
            }
        }
        None => debug!(
            partner = %source,
            request_id = %original,
            "response matches no pending request (likely a one-way request); dropped"
        ),
    }
}

/// Why an unsubscribed message may not be rejected (it needs a
/// `_rejectionVersion` that parses to an integer ≥ 1 and a `_requestId`), or `None` when it may.
fn no_rejection_reason(message: &TransportMessage) -> Option<&'static str> {
    // Reject only when _rejectionVersion parses to an integer ≥ 1.
    let Some(raw) = message.headers.get(header::REJECTION_VERSION) else {
        return Some("no _rejectionVersion (remote transport too old)");
    };

    match parse_text_int(raw) {
        Some(version) if version >= 1 => {}
        Some(_) => return Some("_rejectionVersion below 1 (remote transport too old)"),
        None => return Some("unparsable _rejectionVersion"),
    }

    // A request without `_requestId` cannot be rejected.
    if !message.headers.contains(header::REQUEST_ID) {
        return Some("no _requestId");
    }
    None
}

/// A Platform message the PC builds carries `ms-content-type` (`application/x-binary` or `text/json`);
/// an App message never does.
fn assert_content_type_rule(message: &TransportMessage, what: &str) {
    let content_type = message.headers.get(header::MS_CONTENT_TYPE);
    match message.message_type {
        MessageType::Platform => assert!(
            matches!(content_type, Some(header::CONTENT_TYPE_BINARY | header::CONTENT_TYPE_JSON)),
            "{what}: a Platform message the PC builds must carry ms-content-type application/x-binary or text/json, got {content_type:?}"
        ),
        MessageType::App => assert!(content_type.is_none(), "{what}: App messages never carry ms-content-type"),
    }
}

/// `_requestId` first, then the caller's headers in their order.
fn request_id_first(headers: &HeaderMap, request_id: &str) -> HeaderMap {
    let mut out = HeaderMap::new();
    out.insert(header::REQUEST_ID, request_id);
    for (k, v) in headers.iter() {
        out.insert(k, v);
    }
    out
}

/// `_originalRequestId`, `_route = /internal/response`, the responder's own headers, `_rejectionVersion`,
/// in the order an App response carries them.
fn response_headers(original_request_id: &str, own: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    out.insert(header::ORIGINAL_REQUEST_ID, original_request_id);
    out.insert(header::ROUTE, header::RESPONSE_ROUTE);

    for (k, v) in own.iter() {
        if k != header::ORIGINAL_REQUEST_ID && k != header::ROUTE && k != header::REJECTION_VERSION {
            out.insert(k, v);
        }
    }

    out.insert(header::REJECTION_VERSION, REJECTION_VERSION);
    out
}

fn status_only(status: PlatformRequestStatus) -> PlatformResponse {
    PlatformResponse { status, message: None }
}

/// A response is a rejection when it carries `_rejectedReason` other than `None`.
fn map_response(reply: TransportMessage) -> PlatformResponse {
    match reply.headers.get(header::REJECTED_REASON).and_then(rejection_status) {
        Some(status) => status_only(status),
        None => PlatformResponse { status: PlatformRequestStatus::Success, message: Some(reply) },
    }
}

/// The rejection table of `/internal/response`.
///
/// `_rejectedReason` is read by [`parse_rejection_reason`]; `None` (1) is not a rejection (`None`
/// returned); `2`–`7` map to their statuses; any other value that parses — `Unspecified` (0) or a
/// number no member has — is `NoResponse`; a value that does not parse is treated as
/// `VersionTooHigh`.
pub fn rejection_status(reason: &str) -> Option<PlatformRequestStatus> {
    let Some(value) = parse_rejection_reason(reason) else {
        return Some(PlatformRequestStatus::RemoteVersionTooHigh);
    };

    Some(match value {
        1 => return None,
        2 => PlatformRequestStatus::RemoteSyncNotAllowed,
        3 => PlatformRequestStatus::RemoteVersionTooLow,
        4 => PlatformRequestStatus::RemoteVersionTooHigh,
        5 => PlatformRequestStatus::RemoteAllSyncDisabled,
        6 => PlatformRequestStatus::RemoteMeteredSyncDisabled,
        7 => PlatformRequestStatus::RemoteNotSubscribedToRoute,
        _ => PlatformRequestStatus::NoResponse,
    })
}

/// `_rejectedReason` names and values.
const REJECTION_REASONS: [(&str, i32); 8] = [
    ("Unspecified", 0),
    ("None", 1),
    ("NotAllowed", 2),
    ("VersionTooLow", 3),
    ("VersionTooHigh", 4),
    ("AllSyncDisabled", 5),
    ("MeteredSyncDisabled", 6),
    (NOT_SUBSCRIBED_TO_ROUTE, 7),
];

/// Parses a `_rejectedReason` (case-sensitive).
///
/// Leading white space is skipped; a value starting with an ASCII digit, `-` or `+` is an `Int32`
/// (optional sign, digits, trailing white space or NULs) and parses to that number whether or not a
/// member has it (an overflow fails); otherwise — or when that number does not parse — it is a
/// `,`-separated list of member names, each trimmed, whose values are OR-ed.
///
/// `None` when it does not parse.
pub fn parse_rejection_reason(value: &str) -> Option<i32> {
    let value = value.trim_start();
    let first = value.chars().next()?;
    if first.is_ascii_digit() || first == '-' || first == '+' {
        match parse_int32(value) {
            Int32::Ok(n) => return Some(n),
            Int32::Overflow => return None,
            Int32::Failed => {}
        }
    }

    let mut result = 0;
    let mut rest = value;
    while !rest.is_empty() {
        let name = match rest.find(',') {
            None => std::mem::take(&mut rest),
            Some(comma) if comma == rest.len() - 1 => return None,
            Some(comma) => {
                let (name, tail) = rest.split_at(comma);
                rest = &tail[1..];
                name
            }
        };

        let name = name.trim();
        result |= REJECTION_REASONS.iter().find(|(n, _)| *n == name)?.1;
    }

    Some(result)
}

enum Int32 {
    Ok(i32),
    Overflow,
    Failed,
}

/// An `Int32` with an optional sign: `[+-]digits`, then white space (`\t`–`\r`, space), then only
/// NULs.
fn parse_int32(value: &str) -> Int32 {
    let bytes = value.as_bytes();
    let mut i = 0;
    let negative = match bytes.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };

    let digits_start = i;
    let mut magnitude: i64 = 0;
    let mut overflow = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        magnitude = magnitude.saturating_mul(10).saturating_add(i64::from(bytes[i] - b'0'));
        if magnitude > i64::from(i32::MAX) + 1 {
            overflow = true;
        }
        i += 1;
    }

    if i == digits_start {
        return Int32::Failed;
    }

    while i < bytes.len() && matches!(bytes[i], b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ') {
        i += 1;
    }
    if !bytes[i..].iter().all(|b| *b == 0) {
        return Int32::Failed;
    }

    let signed = if negative { -magnitude } else { magnitude };
    match i32::try_from(signed) {
        Ok(n) if !overflow => Int32::Ok(n),
        _ => Int32::Overflow,
    }
}

/// Runs `future` on its own task; a panic is logged and goes no further (the session continues).
pub(crate) fn spawn_isolated<F>(what: String, future: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(panic) = AssertUnwindSafe(future).catch_unwind().await {
            error!(%what, panic = %panic_text(panic.as_ref()), "inbound handler panicked; isolated, the session continues");
        }
    });
}

pub fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

#[cfg(test)]
mod tests;
