//! `/DeviceResourceManager`.
//!
//! It covers the receiver registry (one receiver per path, matched with any `?query` stripped), the
//! inbound rules and response builder, and the Cross Device Experience Host request wrapper every
//! outbound feature request goes through.
//!
//! A failing receiver is isolated — logged, no response, the session continues (differs from Phone
//! Link on Windows). A receiver may hand back work that waits for its answer to be sent
//! ([`DrmAnswer::follow_up`]): it runs once the send has finished, told whether the transport reported
//! the answer sent. The wrapper's certificate-recovery prompt is not built.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use super::router::{InboundMessage, PlatformRouteHandler, RouteRegistrationError, Router, panic_text};
use super::session::PartnerSessions;
use super::version::{ClientVersion, client_version_passes};
use super::{priority, route};
use phonelink_core::config::EngineConfig;
use phonelink_core::edge::{LockState, SessionLock};
use phonelink_core::seams::{
    ConnectOutcome, ConnectRequest, ConnectionFailure, PartnerSessionControl, PlatformMessenger, PlatformRequest,
    PlatformRequestStatus, PlatformResponse, StartPolicy,
};
use phonelink_core::sync::{MutexExt, RwLockExt};
use phonelink_core::types::{
    DcgClient, DcgSendStatus, HeaderMap, MessageType, SendOptions, TraceContext, TransportMessage, WakeStatus, header,
};
use phonelink_proto::platform_sdk::v1::{
    DeviceResourceManagerMessage, DeviceResourceManagerResponseMessage, DeviceResourceManagerResponseMessageType,
    ProtoDeviceResourceRequestType, ProtoDeviceResourceType,
};

/// How a request reached the PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrmChannel {
    /// The Platform route `/DeviceResourceManager`.
    Platform,
    /// The cloud device proxy (`/DeviceProxyClient/TransportMiddleware`).
    DeviceProxy,
}

/// One inbound request as a receiver sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmRequest {
    pub source: DcgClient,
    /// The request's `_requestId` (Platform) or `RequestId` (device proxy). Without it no answer can be
    /// sent; the receiver may skip its work.
    pub request_id: Option<String>,
    /// The full path, including any `?query`; derived from the resource type when the wire path is empty.
    pub path: String,
    /// The inbound-mapped type: wire `UNSPECIFIED` and `UNKNOWN` are both [`ProtoDeviceResourceType::Unknown`].
    pub resource_type: ProtoDeviceResourceType,
    pub request_type: ProtoDeviceResourceRequestType,
    pub payload: Vec<u8>,
    pub channel: DrmChannel,
}

/// A receiver's own failure: logged, no response, the session continues.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DrmReceiverError(pub String);

/// The receiver registered for one path.
#[async_trait]
pub trait DrmReceiver: Send + Sync {
    /// `Ok(Some(bytes))`: answer with `DeviceResourceManagerResponseMessage { payload: bytes,
    /// response_type: SUCCESS }`; `Ok(None)`: send no answer; `Err`: isolated failure, no answer.
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError>;
}

/// Whether the transport reported an answer sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnswerDelivery {
    Sent,
    /// The send failed, or no answer could be sent (no request id, a reply over its size limit).
    NotSent,
}

/// Work that waits for an answer's send to finish.
pub type FollowUp = Box<dyn FnOnce(AnswerDelivery) -> BoxFuture<'static, ()> + Send>;

/// A receiver's answer, and the work to run once its send has finished.
pub struct DrmAnswer {
    /// `DeviceResourceManagerResponseMessage.payload` (the response type is `SUCCESS`).
    pub payload: Vec<u8>,
    pub follow_up: Option<FollowUp>,
}

impl DrmAnswer {
    /// An answer with `follow_up` to run after its send.
    pub fn then<F, Fut>(payload: Vec<u8>, follow_up: F) -> Self
    where
        F: FnOnce(AnswerDelivery) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        Self { payload, follow_up: Some(Box::new(move |delivery| follow_up(delivery).boxed())) }
    }
}

impl From<Vec<u8>> for DrmAnswer {
    fn from(payload: Vec<u8>) -> Self {
        Self { payload, follow_up: None }
    }
}

impl std::fmt::Debug for DrmAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DrmAnswer")
            .field("payload_len", &self.payload.len())
            .field("has_follow_up", &self.follow_up.is_some())
            .finish()
    }
}

/// A receiver whose answer may carry a [`FollowUp`]; registered with
/// [`Drm::register_follow_up_receiver`].
#[async_trait]
pub trait DrmFollowUpReceiver: Send + Sync {
    /// As [`DrmReceiver::receive`], the answer with its follow-up.
    async fn receive(&self, request: DrmRequest) -> Result<Option<DrmAnswer>, DrmReceiverError>;
}

/// A [`DrmReceiver`] in the registry: its answers have no follow-up.
struct PlainReceiver(Arc<dyn DrmReceiver>);

#[async_trait]
impl DrmFollowUpReceiver for PlainReceiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<DrmAnswer>, DrmReceiverError> {
        Ok(self.0.receive(request).await?.map(DrmAnswer::from))
    }
}

/// Runs an answer's follow-up, if any, once its send finished; a panic in it is isolated.
pub(crate) async fn finish_answer(
    follow_up: Option<FollowUp>,
    delivery: AnswerDelivery,
    source: &DcgClient,
    path: &str,
) {
    let Some(follow_up) = follow_up else { return };
    if let Err(panic) = AssertUnwindSafe(follow_up(delivery)).catch_unwind().await {
        error!(partner = %source, %path, panic = %panic_text(panic.as_ref()), "DRM answer follow-up panicked; isolated, the session continues");
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrmRegistrationError {
    #[error("a receiver is already registered for {0:?} (one receiver per path)")]
    AlreadyRegistered(String),
    #[error("a receiver path must start with '/' and carry no '?query': {0:?}")]
    InvalidPath(String),
    #[error("the /DeviceResourceManager route could not be registered: {0}")]
    Route(#[from] RouteRegistrationError),
}

/// Why an inbound request faults while it is parsed (never dispatched, no reply).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrmFault {
    #[error("request_type {0} is UNSPECIFIED or not a known request type")]
    RequestType(i32),
    #[error("resource_type {0} is not in the inbound mapping")]
    ResourceType(i32),
}

/// Inbound `request_type`: `GET`, `UPDATE`, `DELETE`, `SYNC`; `UNSPECIFIED` (or an unknown value) faults.
pub fn inbound_request_type(value: i32) -> Result<ProtoDeviceResourceRequestType, DrmFault> {
    match ProtoDeviceResourceRequestType::try_from(value) {
        Ok(ProtoDeviceResourceRequestType::Unspecified) | Err(_) => Err(DrmFault::RequestType(value)),
        Ok(t) => Ok(t),
    }
}

/// Inbound `resource_type`: `0` and `4` map to unknown; `7` (`RING_MY_PHONE`), `8` (`LOCK_MY_PC`) and
/// values outside the enumeration are not in the inbound mapping and fault.
pub fn inbound_resource_type(value: i32) -> Result<ProtoDeviceResourceType, DrmFault> {
    use ProtoDeviceResourceType as T;
    match T::try_from(value) {
        Ok(T::Unspecified | T::Unknown) => Ok(T::Unknown),
        Ok(T::RingMyPhone | T::LockMyPc) | Err(_) => Err(DrmFault::ResourceType(value)),
        Ok(t) => Ok(t),
    }
}

/// `"/" + lower-case type name` — the path of a type-addressed request and of an inbound request
/// without a path (e.g. `/filesmetadata`, `/ringmyphone`).
pub fn resource_type_path(resource_type: ProtoDeviceResourceType) -> &'static str {
    use ProtoDeviceResourceType as T;
    match resource_type {
        T::Unspecified | T::Unknown => "/unknown",
        T::FilesMetadata => "/filesmetadata",
        T::CameraMetadata => "/camerametadata",
        T::Permissions => "/permissions",
        T::Wallpaper => "/wallpaper",
        T::Media => "/media",
        T::RingMyPhone => "/ringmyphone",
        T::LockMyPc => "/lockmypc",
        T::FileShare => "/fileshare",
        T::RemoteControl => "/remotecontrol",
    }
}

impl DrmRequest {
    /// Applies the inbound parse rules to the wire fields.
    pub fn from_wire(
        source: DcgClient,
        request_id: Option<String>,
        wire: DeviceResourceManagerMessage,
        channel: DrmChannel,
    ) -> Result<Self, DrmFault> {
        let request_type = inbound_request_type(wire.request_type)?;
        let resource_type = inbound_resource_type(wire.resource_type)?;
        let path = if wire.resource_path.is_empty() {
            resource_type_path(resource_type).to_owned()
        } else {
            wire.resource_path
        };

        Ok(Self { source, request_id, path, resource_type, request_type, payload: wire.payload, channel })
    }
}

/// What dispatching an inbound request produced.
#[derive(Debug)]
pub(crate) enum DrmDispatch {
    Answer(DrmAnswer),
    NoAnswer,
    NoReceiver,
    Failed,
}

/// Addressing of an outbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrmAddress {
    /// A well-formed relative URI (`/clipboard`, `/appcontext?since=5`); wire type `UNKNOWN`.
    Path(String),
    /// A resource type; the wire path is `/` + its lower-case name.
    Type(ProtoDeviceResourceType),
}

/// One outbound request through the Cross Device Experience Host wrapper.
#[derive(Debug, Clone)]
pub struct DrmSendRequest {
    pub target: DcgClient,
    pub address: DrmAddress,
    pub request_type: ProtoDeviceResourceRequestType,
    pub payload: Vec<u8>,
    /// The phone's `clientVersion` from the device list.
    pub client_version: Option<String>,
    /// The local per-phone switch of the feature (off ⇒ `UserDisabled`).
    pub phone_enabled: bool,
    /// Start a session with policy `NewSession` (else `Default`) when the phone is not available.
    pub force_session_start: bool,
    /// Bounds send plus wait (at most `PlatformConfig::request_timeout`).
    pub timeout: Duration,
    pub trace: TraceContext,
    pub cancel: CancellationToken,
}

/// The wrapper's result. Its `Debug` shows a success's payload by its length only, because an
/// outcome is logged under a public field and the payload is the phone's content.
#[derive(Clone, PartialEq, Eq)]
pub enum DrmSendOutcome {
    /// The phone answered; `payload` is `DeviceResourceManagerResponseMessage.payload`.
    Success {
        payload: Vec<u8>,
    },
    /// Programming error: a path that is not a well-formed relative URI, or an `UNSPECIFIED` type;
    /// the text says which.
    InvalidRequest(&'static str),
    UserDisabled,
    /// The PC session is locked or its lock state is unknown.
    Locked,
    LtwMinVersionRequired,
    /// The platform session could not be started within `PlatformConfig::session_maximum_wait`.
    SessionStartFailed(ConnectOutcome),
    Canceled,
    /// `RemoteNotSubscribedToRoute`, or `response_type == RESOURCE_HANDLER_NOT_REGISTERED`.
    UnsupportedResourceType,
    /// `NoResponse` or `ResponseTimedOut`.
    ResponseTimeout,
    /// A success response without a body, or whose body is not a
    /// `DeviceResourceManagerResponseMessage` (what is wrong with it).
    InvalidResponse(String),
    /// Every other request status, including `RemoteAllSyncDisabled` — also the mapped status of a
    /// session start that failed with the phone's LTW disabled ("`PeerDisabled`") —
    /// `RemoteMeteredSyncDisabled` and `RequestSendFailed(cause)`.
    Request(PlatformRequestStatus),
}

impl DrmSendOutcome {
    /// The text of what failed, for a send whose hub open or wake failed with one.
    pub fn cause(&self) -> Option<&str> {
        match self {
            Self::Request(status) => status.cause(),
            _ => None,
        }
    }
}

impl std::fmt::Debug for DrmSendOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success { payload } => f.debug_struct("Success").field("payload_len", &payload.len()).finish(),
            Self::InvalidRequest(why) => f.debug_tuple("InvalidRequest").field(why).finish(),
            Self::UserDisabled => f.write_str("UserDisabled"),
            Self::Locked => f.write_str("Locked"),
            Self::LtwMinVersionRequired => f.write_str("LtwMinVersionRequired"),
            Self::SessionStartFailed(outcome) => f.debug_tuple("SessionStartFailed").field(outcome).finish(),
            Self::Canceled => f.write_str("Canceled"),
            Self::UnsupportedResourceType => f.write_str("UnsupportedResourceType"),
            Self::ResponseTimeout => f.write_str("ResponseTimeout"),
            Self::InvalidResponse(why) => f.debug_tuple("InvalidResponse").field(why).finish(),
            Self::Request(status) => f.debug_tuple("Request").field(status).finish(),
        }
    }
}

/// A well-formed relative URI, as far as the engine's paths need: non-empty, no scheme, no
/// authority, RFC 3986 characters with valid percent-escapes.
pub fn is_well_formed_relative_uri(path: &str) -> bool {
    if path.is_empty() || path.starts_with("//") {
        return false;
    }

    if let Some(colon) = path.find(':') {
        let before = &path[..colon];
        let scheme_like = before.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            && before.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if scheme_like {
            return false;
        }
    }

    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let valid = bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit);
            if !valid {
                return false;
            }
            i += 3;
            continue;
        }

        if !(b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?#".contains(&b)) {
            return false;
        }
        i += 1;
    }

    true
}

/// The DRM layer: receivers, the inbound route, and the outbound wrapper.
pub struct Drm {
    me: Weak<Drm>,
    config: Arc<EngineConfig>,
    router: Arc<Router>,
    sessions: Arc<PartnerSessions>,
    lock: Arc<dyn SessionLock>,
    minimum_client_version: ClientVersion,
    receivers: RwLock<HashMap<String, Arc<dyn DrmFollowUpReceiver>>>,
    metered_sync_disabled: Mutex<HashMap<DcgClient, bool>>,
}

impl Drm {
    /// Panics when `config.platform.drm_minimum_client_version` is not a `System.Version` (a broken
    /// configuration).
    pub fn new(
        config: Arc<EngineConfig>,
        router: Arc<Router>,
        sessions: Arc<PartnerSessions>,
        lock: Arc<dyn SessionLock>,
    ) -> Arc<Self> {
        let minimum_client_version =
            ClientVersion::parse(&config.platform.drm_minimum_client_version).unwrap_or_else(|| {
                panic!("drm_minimum_client_version {:?} is not a version", config.platform.drm_minimum_client_version)
            });

        Arc::new_cyclic(|me| Drm {
            me: me.clone(),
            config,
            router,
            sessions,
            lock,
            minimum_client_version,
            receivers: RwLock::default(),
            metered_sync_disabled: Mutex::default(),
        })
    }

    /// Registers the receiver of `path` (no `?query`; one receiver per path). The first registration
    /// also subscribes `/DeviceResourceManager` on the router — before it the route is rejected
    /// `NotSubscribedToRoute`, as Phone Link on Windows rejects it.
    pub fn register_receiver(&self, path: &str, receiver: Arc<dyn DrmReceiver>) -> Result<(), DrmRegistrationError> {
        self.register_follow_up_receiver(path, Arc::new(PlainReceiver(receiver)))
    }

    /// [`Drm::register_receiver`] for a receiver whose answers may carry a follow-up.
    pub fn register_follow_up_receiver(
        &self,
        path: &str,
        receiver: Arc<dyn DrmFollowUpReceiver>,
    ) -> Result<(), DrmRegistrationError> {
        if !path.starts_with('/') || path.contains('?') {
            return Err(DrmRegistrationError::InvalidPath(path.to_owned()));
        }

        let first = {
            let mut receivers = self.receivers.write_or_panic();
            if receivers.contains_key(path) {
                return Err(DrmRegistrationError::AlreadyRegistered(path.to_owned()));
            }
            let first = receivers.is_empty();
            receivers.insert(path.to_owned(), receiver);
            first
        };

        if first {
            let me = self.me.upgrade().expect("Drm is alive while registering");
            self.router.register_route(route::DEVICE_RESOURCE_MANAGER, Arc::new(DrmRoute(me)))?;
        }

        Ok(())
    }

    /// The phone's metered-sync-disabled flag, reassigned after every wrapper send (set only by a
    /// `RemoteMeteredSyncDisabled` failure).
    pub fn metered_sync_disabled(&self, partner: &DcgClient) -> bool {
        self.metered_sync_disabled.lock_or_panic().get(partner).copied().unwrap_or(false)
    }

    /// Dispatches to the receiver of the path with its `?query` stripped; a failing receiver is isolated.
    pub(crate) async fn dispatch(&self, request: DrmRequest) -> DrmDispatch {
        let key = request.path.split('?').next().unwrap_or_default().to_owned();
        let receiver = self.receivers.read_or_panic().get(&key).cloned();
        let Some(receiver) = receiver else {
            warn!(partner = %request.source, raw_path = %request.path, "no DRM receiver for the path; no response");
            return DrmDispatch::NoReceiver;
        };

        let (source, path) = (request.source.clone(), request.path.clone());
        match AssertUnwindSafe(receiver.receive(request)).catch_unwind().await {
            Ok(Ok(Some(answer))) => DrmDispatch::Answer(answer),
            Ok(Ok(None)) => DrmDispatch::NoAnswer,
            Ok(Err(failure)) => {
                warn!(partner = %source, %path, %failure, "DRM receiver failed; isolated, no response, the session continues");
                DrmDispatch::Failed
            }
            Err(panic) => {
                error!(partner = %source, %path, panic = %panic_text(panic.as_ref()), "DRM receiver panicked; isolated, no response, the session continues");
                DrmDispatch::Failed
            }
        }
    }

    /// Inbound Platform request on `/DeviceResourceManager`.
    async fn on_platform_request(&self, inbound: InboundMessage) {
        let InboundMessage { source, trace, message, .. } = inbound;
        let Some((request, request_id)) = parse_platform_request(&source, &message) else {
            return;
        };

        let path = request.path.clone();
        let DrmDispatch::Answer(DrmAnswer { payload, follow_up }) = self.dispatch(request).await else {
            return;
        };

        if request_id.is_none() {
            warn!(partner = %source, %path, "DRM request without _requestId; the answer is dropped");
            finish_answer(follow_up, AnswerDelivery::NotSent, &source, &path).await;
            return;
        }

        let status =
            self.router.respond(&source, &message, response_message(payload), priority::PROTOBUF_DEFAULT, trace).await;
        let delivery = if status == DcgSendStatus::Success {
            AnswerDelivery::Sent
        } else {
            warn!(partner = %source, %path, ?status, error = status.cause(), "DRM answer not delivered");
            AnswerDelivery::NotSent
        };
        finish_answer(follow_up, delivery, &source, &path).await;
    }

    /// The Cross Device Experience Host request wrapper, in order: relative-path check, per-phone switch,
    /// lock state, phone `clientVersion`, session start when the phone is not available, cancellation,
    /// then one send (no retry) and the metered-sync flag.
    pub async fn send(&self, request: DrmSendRequest) -> DrmSendOutcome {
        let DrmSendRequest {
            target,
            address,
            request_type,
            payload,
            client_version,
            phone_enabled,
            force_session_start,
            timeout,
            trace,
            cancel,
        } = request;

        let (resource_type, resource_path) = match resolve_address(address, request_type) {
            Ok(resolved) => resolved,
            Err(outcome) => return outcome,
        };

        if !phone_enabled {
            return DrmSendOutcome::UserDisabled;
        }
        if self.lock_state().await != LockState::Unlocked {
            return DrmSendOutcome::Locked;
        }
        if !client_version_passes(client_version.as_deref(), self.minimum_client_version) {
            return DrmSendOutcome::LtwMinVersionRequired;
        }
        if !self.sessions.is_connected(&target) {
            if let Err(outcome) = self.start_session(&target, force_session_start, &trace, &cancel).await {
                return outcome;
            }
        }
        if cancel.is_cancelled() {
            return DrmSendOutcome::Canceled;
        }

        let response = self
            .router
            .request(PlatformRequest {
                target: target.clone(),
                message: drm_request_message(resource_type, request_type, payload, resource_path),
                priority: priority::PROTOBUF_DEFAULT,
                options: SendOptions { ignore_device_disabled: true, ..SendOptions::NONE },
                timeout,
                trace,
                cancel,
            })
            .await;

        self.metered_sync_disabled
            .lock_or_panic()
            .insert(target, response.status == PlatformRequestStatus::RemoteMeteredSyncDisabled);
        map_send_response(response)
    }

    /// Starts the session to `target` before a DRM send, unless `cancel` fires first; the outcome
    /// the send ends with when the session does not connect.
    async fn start_session(
        &self,
        target: &DcgClient,
        force_session_start: bool,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<(), DrmSendOutcome> {
        let start = ConnectRequest {
            policy: if force_session_start { StartPolicy::NewSession } else { StartPolicy::Default },
            ignore_device_disabled: false,
            feedback_window: self.config.platform.session_maximum_wait,
            trace: trace.clone(),
        };

        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(DrmSendOutcome::Canceled),
            outcome = self.sessions.connect(target, start) => outcome,
        };

        match outcome {
            ConnectOutcome::Connected => Ok(()),
            ConnectOutcome::Failed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)) => {
                Err(DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled))
            }
            other => Err(DrmSendOutcome::SessionStartFailed(other)),
        }
    }

    /// The macOS lock test, off the async runtime (`phonelink_core::edge` traits block).
    async fn lock_state(&self) -> LockState {
        let lock = self.lock.clone();
        match tokio::task::spawn_blocking(move || lock.lock_state()).await {
            Ok(state) => state,
            Err(e) => {
                warn!(error = %e, "session-lock query failed; counted as unknown");
                LockState::Unknown
            }
        }
    }
}

/// `DeviceResourceManagerResponseMessage { payload, response_type: SUCCESS }`, `application/x-binary`
/// (the router puts it on `/internal/response`).
pub fn response_message(payload: Vec<u8>) -> TransportMessage {
    let body = DeviceResourceManagerResponseMessage {
        payload,
        response_type: Some(DeviceResourceManagerResponseMessageType::Success as i32),
    }
    .encode_to_vec();

    let headers = HeaderMap::from_pairs([(header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY)]).expect("one key");
    TransportMessage::new(MessageType::Platform, headers, body)
}

/// Parses an inbound `/DeviceResourceManager` request and its `_requestId`; `None` (logged) for an
/// empty, undecodable or faulted body, which gets no reply.
fn parse_platform_request(source: &DcgClient, message: &TransportMessage) -> Option<(DrmRequest, Option<String>)> {
    if message.payload.is_empty() {
        warn!(partner = %source, "empty /DeviceResourceManager body; ignored, no reply");
        return None;
    }

    let wire = match DeviceResourceManagerMessage::decode(message.payload.as_slice()) {
        Ok(wire) => wire,
        Err(e) => {
            warn!(partner = %source, error = %e, "undecodable /DeviceResourceManager body; dropped, no reply");
            return None;
        }
    };

    let request_id = message.headers.get(header::REQUEST_ID).map(str::to_owned);
    match DrmRequest::from_wire(source.clone(), request_id.clone(), wire, DrmChannel::Platform) {
        Ok(request) => Some((request, request_id)),
        Err(fault) => {
            warn!(partner = %source, %fault, "/DeviceResourceManager request faulted while parsing; dropped, no reply");
            None
        }
    }
}

/// The wire resource type and path of a DRM send's address, after the request type; `InvalidRequest`
/// (a programming error, logged) for a malformed path or an `UNSPECIFIED` type.
fn resolve_address(
    address: DrmAddress,
    request_type: ProtoDeviceResourceRequestType,
) -> Result<(ProtoDeviceResourceType, String), DrmSendOutcome> {
    let resolved = match address {
        DrmAddress::Path(path) => {
            if !is_well_formed_relative_uri(&path) {
                return Err(DrmSendOutcome::InvalidRequest("the resource path is not a well-formed relative URI"));
            }
            (ProtoDeviceResourceType::Unknown, path)
        }
        DrmAddress::Type(ProtoDeviceResourceType::Unspecified) => {
            return Err(DrmSendOutcome::InvalidRequest("the request is addressed by the UNSPECIFIED type"));
        }
        DrmAddress::Type(t) => (t, resource_type_path(t).to_owned()),
    };

    if request_type == ProtoDeviceResourceRequestType::Unspecified {
        return Err(DrmSendOutcome::InvalidRequest("the request type is UNSPECIFIED"));
    }
    Ok(resolved)
}

/// The Platform message of an outbound DRM request on `/DeviceResourceManager`.
fn drm_request_message(
    resource_type: ProtoDeviceResourceType,
    request_type: ProtoDeviceResourceRequestType,
    payload: Vec<u8>,
    resource_path: String,
) -> TransportMessage {
    let body = DeviceResourceManagerMessage {
        resource_type: resource_type as i32,
        request_type: request_type as i32,
        payload,
        resource_path,
    }
    .encode_to_vec();

    let headers = HeaderMap::from_pairs([
        (header::ROUTE, route::DEVICE_RESOURCE_MANAGER),
        (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY),
    ])
    .expect("distinct header keys");
    TransportMessage::new(MessageType::Platform, headers, body)
}

fn map_send_response(response: PlatformResponse) -> DrmSendOutcome {
    match response.status {
        PlatformRequestStatus::Success => {
            let Some(message) = response.message else {
                return DrmSendOutcome::InvalidResponse("a success without a response message".to_owned());
            };

            match DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()) {
                Ok(r)
                    if r.response_type
                        == Some(DeviceResourceManagerResponseMessageType::ResourceHandlerNotRegistered as i32) =>
                {
                    DrmSendOutcome::UnsupportedResourceType
                }
                Ok(r) => DrmSendOutcome::Success { payload: r.payload },
                Err(e) => DrmSendOutcome::InvalidResponse(format!("not a DeviceResourceManagerResponseMessage: {e}")),
            }
        }
        PlatformRequestStatus::RemoteNotSubscribedToRoute => DrmSendOutcome::UnsupportedResourceType,
        PlatformRequestStatus::NoResponse | PlatformRequestStatus::ResponseTimedOut => DrmSendOutcome::ResponseTimeout,
        PlatformRequestStatus::Canceled => DrmSendOutcome::Canceled,
        other => DrmSendOutcome::Request(other),
    }
}

/// The `/DeviceResourceManager` route handler (registered with the first receiver).
struct DrmRoute(Arc<Drm>);

#[async_trait]
impl PlatformRouteHandler for DrmRoute {
    async fn handle(&self, inbound: InboundMessage) {
        self.0.on_platform_request(inbound).await;
    }
}

#[cfg(test)]
mod tests;
