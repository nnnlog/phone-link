//! Outbound App requests: building, sending with timeouts and retries, and correlating the response.
//!
//! A request is built here — envelope headers `_requestId` then `_route`, the feature's `ValueSet`
//! with the legacy keys and the four standard keys written over it, the options' priority, and the
//! hub trace context — then sent through the local gates, the connection wait and the platform
//! router ([`phonelink_core::seams::PlatformMessenger::request`]), and its response read into an
//! [`AppOutcome`].

use std::time::Duration;

use prost::Message;
use serde_json::json;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use phonelink_core::codec::valueset::{self, ValueSet};
use phonelink_core::seams::{
    ConnectOutcome, ConnectRequest, PlatformRequest, PlatformRequestStatus, PlatformResponse, StartPolicy,
};
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use phonelink_core::types::{
    DcgClient, HeaderMap, MessageType, SendOptions, TraceContext, TransportMessage, header, new_guid_lower,
};

use super::AppError;
use super::AppLayer;
use super::consts::{key, priority, route};
use super::message::{AppMessage, ContractVersion, read_app_message};
use super::outcome::{
    AppOutcome, ResourceFailure, outcome_for_connection_failure, outcome_for_platform_status, outcome_for_result,
    outcome_for_send_failure,
};
use phonelink_proto::platform_sdk::v1::{
    DeviceResourceManagerMessage, DeviceResourceManagerResponseMessage, DeviceResourceManagerResponseMessageType,
    ProtoDeviceResourceRequestType, ProtoDeviceResourceType,
};
use phonelink_transport::platform::drm::resource_type_path;

/// How the shared layer completes the feature's `ValueSet`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    /// The four standard keys only. A `Bool` `isuserinitiated: true` in the feature's `ValueSet`
    /// makes the request user-initiated; a `Bool` `ignoreResponse` sets the *ignore response*
    /// option; both stay as the feature put them.
    Kvp,
    /// `isuserinitiated` and `ignoreResponse` (from the options), `transportType: Int32 0`, then the
    /// four standard keys — each written over a same-named key of the feature.
    Legacy,
}

/// Local request options; [`Default`] is what a request sent without explicit options gets,
/// whatever triggered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOptions {
    /// Bounds waiting for the connection plus sending; `None` = the configured default (120 s).
    pub timeout: Option<Duration>,
    /// Fragment-queue priority, `100` by default; a user-initiated request is sent at `0`.
    pub priority: u32,
    /// Connection start policy `NewSession` and router option `IgnoreDeviceDisabled`.
    pub user_initiated: bool,
    /// No wire effect besides the legacy key; tells the feature to suppress its error UI.
    pub ignore_response: bool,
    /// `ypTriggerId` of the trace state (free-form).
    pub trigger_id: String,
    /// `ypScenarioId` of the trace state (free-form).
    pub scenario_id: String,
    /// The trace state says `isuserinitiated` `True` although the request is not user-initiated
    /// (a request the user started that is sent with default options).
    pub traced_as_user_initiated: bool,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            priority: priority::LOW,
            user_initiated: false,
            ignore_response: false,
            trigger_id: String::new(),
            scenario_id: String::new(),
            traced_as_user_initiated: false,
        }
    }
}

impl RequestOptions {
    /// User-initiated options (priority `0`).
    pub fn user_initiated() -> Self {
        Self { priority: priority::HIGH, user_initiated: true, ..Self::default() }
    }

    /// Sets the trace-state names.
    pub fn named(mut self, trigger_id: impl Into<String>, scenario_id: impl Into<String>) -> Self {
        self.trigger_id = trigger_id.into();
        self.scenario_id = scenario_id.into();
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Traces the request as user-initiated without making it one ([`Self::traced_as_user_initiated`]).
    pub fn traced_as_user_initiated(mut self) -> Self {
        self.traced_as_user_initiated = true;
        self
    }
}

/// One App request a feature hands this layer.
#[derive(Debug, Clone, PartialEq)]
pub struct AppRequest {
    /// The feature route.
    pub route: String,
    pub kind: RequestKind,
    /// The feature's `ValueSet` (`contentType`, `correlationVector` and the feature keys).
    pub payload: ValueSet,
    pub options: RequestOptions,
    /// The trace context the request is sent with; `None` = a fresh root context. Its correlation
    /// id becomes the hub packet's `ParentId` (features whose `correlationVector` is "the
    /// request's trace correlation id" put that id there); its flags are sent as `0` and its trace
    /// state is replaced by this layer's four keys.
    pub trace: Option<TraceContext>,
}

impl AppRequest {
    pub fn kvp(route: impl Into<String>, payload: ValueSet) -> Self {
        Self::new(route, RequestKind::Kvp, payload)
    }

    pub fn legacy(route: impl Into<String>, payload: ValueSet) -> Self {
        Self::new(route, RequestKind::Legacy, payload)
    }

    pub fn new(route: impl Into<String>, kind: RequestKind, payload: ValueSet) -> Self {
        Self { route: route.into(), kind, payload, options: RequestOptions::default(), trace: None }
    }

    pub fn with_options(mut self, options: RequestOptions) -> Self {
        self.options = options;
        self
    }

    pub fn with_trace(mut self, trace: TraceContext) -> Self {
        self.trace = Some(trace);
        self
    }
}

/// One `/DeviceResourceManager` request a feature sends through this layer (a DRM content request):
/// addressed by resource type, its body the feature's protobuf bytes, no `ValueSet` and no standard
/// keys.
///
/// It passes the same local gates, connection wait, "all features disabled" flag and request
/// observers as an App request; the wire message is a Platform request on `/DeviceResourceManager`.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmAppRequest {
    pub resource_type: ProtoDeviceResourceType,
    pub request_type: ProtoDeviceResourceRequestType,
    pub payload: Vec<u8>,
    /// The timeout, user-initiated flag and trace-state names (the priority is the protobuf default).
    pub options: RequestOptions,
    /// The trace context; `None` = a fresh root context.
    pub trace: Option<TraceContext>,
}

/// Which local gate failed a request; nothing was sent and the outcome is [`AppOutcome::Canceled`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocalGate {
    /// No phone is selected.
    NoPhone,
    /// The PC session is locked (the macOS lock test said locked or unknown).
    Locked,
    /// The enrolled account is missing or its token cannot be refreshed ([`AccountGate`]).
    ///
    /// [`AccountGate`]: super::AccountGate
    AccountNotReady,
    /// A device-list phone without `AsyncTrust`, and the request is not user-initiated.
    AsyncTrust,
}

/// What a feature gets back.
#[derive(Debug, Clone, PartialEq)]
pub struct AppResponse {
    pub outcome: AppOutcome,
    /// The `result` read from the response (`-1` when missing or non-integer); `None` when no
    /// response arrived, or when the response's payload could not be read (not a `ValueSet`).
    pub result: Option<i32>,
    /// The response (lifted headers, parsed payload) when one arrived and parsed.
    pub message: Option<AppMessage>,
    /// Set when a local gate failed the request.
    pub local_gate: Option<LocalGate>,
    /// The request's own caller canceled it (an outcome of `Canceled` the caller caused, not one the
    /// transport reported).
    pub canceled_by_caller: bool,
    /// The `DeviceResourceManagerResponseMessage.payload` of a [`DrmAppRequest`] that succeeded.
    pub resource_payload: Option<Vec<u8>>,
}

impl AppResponse {
    pub(crate) fn gated(gate: LocalGate) -> Self {
        Self {
            outcome: AppOutcome::Canceled,
            result: None,
            message: None,
            local_gate: Some(gate),
            canceled_by_caller: false,
            resource_payload: None,
        }
    }

    pub(crate) fn failed(outcome: AppOutcome) -> Self {
        Self {
            outcome,
            result: None,
            message: None,
            local_gate: None,
            canceled_by_caller: false,
            resource_payload: None,
        }
    }

    /// The response `ValueSet`, when one arrived.
    pub fn values(&self) -> Option<&ValueSet> {
        self.message.as_ref().map(|m| &m.values)
    }

    /// A response arrived: an App response, or the payload of a successful DRM request.
    pub fn arrived(&self) -> bool {
        self.message.is_some() || self.resource_payload.is_some()
    }
}

/// The four standard keys: Phone Link's App-layer values.
#[derive(Debug, Clone, PartialEq)]
pub struct StandardKeys {
    pub display_name: String,
    pub app_version: String,
    pub installation_id: String,
    pub contract_version: f64,
}

impl StandardKeys {
    /// Writes the keys over same-named ones.
    pub fn write(&self, values: &mut ValueSet) {
        values.insert(key::DISPLAY_NAME, self.display_name.clone());
        values.insert(key::APP_VERSION, self.app_version.clone());
        values.insert(key::INSTALLATION_ID, self.installation_id.clone());
        values.insert(key::CONTRACT_VERSION, self.contract_version);
    }
}

/// A request ready for the router.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub route: String,
    pub message: TransportMessage,
    pub values: ValueSet,
    pub user_initiated: bool,
    pub priority: u32,
    pub timeout: Duration,
    pub trace: TraceContext,
}

fn bool_text(b: bool) -> String {
    if b { "True" } else { "False" }.to_string()
}

/// Rejects routes this layer must not build a `ValueSet` request for.
fn validate_route(r: &str) -> Result<(), AppError> {
    if !r.starts_with('/') || r.len() < 2 {
        return Err(AppError::Programming(format!("App request route {r:?} is not a route")));
    }
    if r == route::RESPONSE || r == route::DEVICE_RESOURCE_MANAGER || r == route::PHONE_CONTENT {
        return Err(AppError::Programming(format!("{r} is not an App request route on the Android path")));
    }
    Ok(())
}

/// Refuses a DRM request without a concrete resource type or a request type.
fn check_drm_request(
    resource_type: ProtoDeviceResourceType,
    request_type: ProtoDeviceResourceRequestType,
) -> Result<(), AppError> {
    if matches!(resource_type, ProtoDeviceResourceType::Unspecified | ProtoDeviceResourceType::Unknown) {
        return Err(AppError::Programming(format!(
            "a DRM request through the App layer is addressed by a resource type, not {resource_type:?}"
        )));
    }
    if request_type == ProtoDeviceResourceRequestType::Unspecified {
        return Err(AppError::Programming("a DRM request needs a request type".into()));
    }
    Ok(())
}

/// Builds the envelope, the completed `ValueSet`, the priority and the trace context.
pub(crate) fn prepare(
    request: AppRequest,
    standard: &StandardKeys,
    foreground: bool,
    default_timeout: Duration,
) -> Result<Prepared, AppError> {
    validate_route(&request.route)?;
    let AppRequest { route, kind, payload: mut values, options, trace } = request;

    // The *ignore response* option (a KVP request's `Bool` `ignoreResponse`, else the option) has
    // no wire effect besides the legacy key; the feature applies it to its own error UI.
    let user_initiated = match kind {
        RequestKind::Kvp => options.user_initiated || values.bool(key::IS_USER_INITIATED) == Some(true),
        RequestKind::Legacy => {
            values.insert(key::IS_USER_INITIATED, options.user_initiated);
            values.insert(key::IGNORE_RESPONSE, options.ignore_response);
            values.insert(key::TRANSPORT_TYPE, 0i32);
            options.user_initiated
        }
    };

    standard.write(&mut values);
    let priority = if user_initiated { priority::HIGH } else { options.priority };

    let mut trace = trace.unwrap_or_else(|| TraceContext::new_root(Vec::new()));
    trace.flags = 0;
    trace.trace_state = vec![
        (phonelink_core::types::trace_key::TRIGGER.to_string(), options.trigger_id.clone()),
        (phonelink_core::types::trace_key::SCENARIO.to_string(), options.scenario_id.clone()),
        ("isuserinitiated".to_string(), bool_text(user_initiated || options.traced_as_user_initiated)),
        ("isForeground".to_string(), bool_text(foreground)),
    ];

    let mut headers = HeaderMap::new();
    headers.insert(header::REQUEST_ID, new_guid_lower());
    headers.insert(header::ROUTE, route.clone());
    let message = TransportMessage::new(MessageType::App, headers, valueset::encode(&values));

    Ok(Prepared {
        route,
        message,
        values,
        user_initiated,
        priority,
        timeout: options.timeout.unwrap_or(default_timeout),
        trace,
    })
}

/// The tap summary of one App message.
pub(crate) fn tap_summary(kind: &str, headers: &HeaderMap, values: Option<&ValueSet>) -> serde_json::Value {
    json!({
        "kind": kind,
        "headers": headers.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>(),
        "valueSet": values.map(ValueSet::to_json),
    })
}

/// Maps the result of a [`DrmAppRequest`]'s platform request to the App layer's response.
fn drm_response(status: PlatformRequestStatus, message: Option<TransportMessage>) -> AppResponse {
    let failed = |f| AppResponse::failed(AppOutcome::ResourceFailed(f));
    match status {
        PlatformRequestStatus::Success => {
            let Some(message) = message else {
                warn!("DRM request succeeded without a response message; the request fails");
                return failed(ResourceFailure::PayloadError);
            };

            match DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()) {
                Ok(r)
                    if r.response_type
                        == Some(DeviceResourceManagerResponseMessageType::ResourceHandlerNotRegistered as i32) =>
                {
                    failed(ResourceFailure::PayloadError)
                }
                Ok(r) => AppResponse {
                    outcome: AppOutcome::Success,
                    result: Some(super::consts::result_code::SUCCESS),
                    message: None,
                    local_gate: None,
                    canceled_by_caller: false,
                    resource_payload: Some(r.payload),
                },
                Err(e) => {
                    warn!(error = %e, "DRM response body is not a DeviceResourceManagerResponseMessage; the request fails");
                    failed(ResourceFailure::PayloadError)
                }
            }
        }
        PlatformRequestStatus::Canceled => AppResponse::failed(AppOutcome::Canceled),
        PlatformRequestStatus::NoResponse | PlatformRequestStatus::ResponseTimedOut => {
            AppResponse::failed(AppOutcome::TimedOut)
        }
        PlatformRequestStatus::RequestSendFailed(cause) => AppResponse::failed(outcome_for_send_failure(&cause)),
        PlatformRequestStatus::RemoteNotSubscribedToRoute => failed(ResourceFailure::PayloadError),
        PlatformRequestStatus::RemoteAllSyncDisabled => failed(ResourceFailure::RemoteAllSyncDisabled),
        PlatformRequestStatus::RemoteMeteredSyncDisabled => failed(ResourceFailure::RemoteMeteredSyncDisabled),
        PlatformRequestStatus::RemoteSyncNotAllowed => failed(ResourceFailure::RemoteSyncNotAllowed),
        PlatformRequestStatus::RemoteVersionTooLow | PlatformRequestStatus::RemoteVersionTooHigh => {
            failed(ResourceFailure::NotAllowed)
        }
    }
}

impl AppLayer {
    /// Sends one App request to the selected phone and waits for its outcome. `Err` only for a
    /// request this layer must not build (a programming error); every runtime result is an
    /// [`AppResponse`].
    pub async fn request(&self, request: AppRequest) -> Result<AppResponse, AppError> {
        self.send_request(request, CancellationToken::new()).await
    }

    /// [`AppLayer::request`] with a cancellation token (cancel → [`AppOutcome::Canceled`]).
    pub async fn request_cancellable(
        &self,
        request: AppRequest,
        cancel: CancellationToken,
    ) -> Result<AppResponse, AppError> {
        self.send_request(request, cancel).await
    }

    pub(crate) async fn send_request(
        &self,
        request: AppRequest,
        cancel: CancellationToken,
    ) -> Result<AppResponse, AppError> {
        let prepared = prepare(request, &self.standard_keys, self.is_foreground(), self.config.app.request_timeout)?;
        let Some(phone) = self.selected_phone() else {
            return Ok(AppResponse::gated(LocalGate::NoPhone));
        };
        let deadline = Instant::now() + prepared.timeout;

        // Local gates. A request the account gate stops clears the "all features disabled" flag like
        // any completed request; the AsyncTrust and lock gates leave it unchanged.
        if let Some(gate) = self.local_gate(&phone, prepared.user_initiated).await {
            return Ok(self.gated_response(&phone, &prepared.route, gate));
        }

        // Connection policy and the wait for the connection.
        let mut response = match self.ensure_connected(&phone, &prepared, deadline, &cancel).await {
            Err(outcome) => AppResponse::failed(outcome),
            // Send, wait for the response, map the outcome.
            Ok(()) => self.send_prepared(&phone, &prepared, deadline, &cancel).await,
        };
        self.finish_attempt(&phone, &prepared.route, &mut response, &cancel);
        Ok(response)
    }

    /// Ends an attempt that passed the local gates: a request its caller canceled is marked so and
    /// leaves the "all features disabled" flag as it was; any other updates it.
    fn finish_attempt(&self, phone: &DcgClient, route: &str, response: &mut AppResponse, cancel: &CancellationToken) {
        if response.outcome == AppOutcome::Canceled && cancel.is_cancelled() {
            response.canceled_by_caller = true;
            // Canceled by its caller: the flag is left as it was.
            self.notify_request_observers(phone, route, response);
        } else {
            self.after_attempt(phone, route, response);
        }
    }

    /// Sends one `/DeviceResourceManager` request to the selected phone through this layer's request
    /// path — the local gates, the connection wait, the "all features disabled" flag and the request
    /// observers of [`AppLayer::request`] — and maps its result: a reply that is a
    /// `DeviceResourceManagerResponseMessage` without `RESOURCE_HANDLER_NOT_REGISTERED` is
    /// [`AppOutcome::Success`] (`result` `0`, [`AppResponse::resource_payload`] set); every other
    /// result is a failure outcome. The observers see the route `/DeviceResourceManager`.
    pub async fn request_drm(&self, request: DrmAppRequest) -> Result<AppResponse, AppError> {
        let DrmAppRequest { resource_type, request_type, payload, options, trace } = request;
        check_drm_request(resource_type, request_type)?;

        let user_initiated = options.user_initiated;
        let mut trace = trace.unwrap_or_else(|| TraceContext::new_root(Vec::new()));
        trace.flags = 0;
        trace.trace_state = vec![
            (phonelink_core::types::trace_key::TRIGGER.to_string(), options.trigger_id.clone()),
            (phonelink_core::types::trace_key::SCENARIO.to_string(), options.scenario_id.clone()),
            ("isuserinitiated".to_string(), bool_text(user_initiated || options.traced_as_user_initiated)),
            ("isForeground".to_string(), bool_text(self.is_foreground())),
        ];

        let body = DeviceResourceManagerMessage {
            resource_type: resource_type as i32,
            request_type: request_type as i32,
            payload,
            resource_path: resource_type_path(resource_type).to_owned(),
        }
        .encode_to_vec();
        let headers = HeaderMap::from_pairs([
            (header::ROUTE, route::DEVICE_RESOURCE_MANAGER),
            (header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY),
        ])
        .expect("distinct header keys");

        let prepared = Prepared {
            route: route::DEVICE_RESOURCE_MANAGER.to_owned(),
            message: TransportMessage::new(MessageType::Platform, headers, body),
            values: ValueSet::new(),
            user_initiated,
            priority: phonelink_transport::platform::priority::PROTOBUF_DEFAULT,
            timeout: options.timeout.unwrap_or(self.config.app.request_timeout),
            trace,
        };

        let Some(phone) = self.selected_phone() else {
            return Ok(AppResponse::gated(LocalGate::NoPhone));
        };
        let deadline = Instant::now() + prepared.timeout;

        if let Some(gate) = self.local_gate(&phone, prepared.user_initiated).await {
            return Ok(self.gated_response(&phone, &prepared.route, gate));
        }

        let response = match self.ensure_connected(&phone, &prepared, deadline, &CancellationToken::new()).await {
            Err(outcome) => AppResponse::failed(outcome),
            Ok(()) => self.send_prepared_drm(&phone, &prepared, deadline).await,
        };
        self.after_attempt(&phone, &prepared.route, &response);
        Ok(response)
    }

    /// The response of a request a local gate stopped, after the observers heard of it: the account
    /// gate clears the "all features disabled" flag like any completed request; the `AsyncTrust` and
    /// lock gates leave it unchanged.
    fn gated_response(&self, phone: &DcgClient, route: &str, gate: LocalGate) -> AppResponse {
        let response = AppResponse::gated(gate);
        if gate == LocalGate::AccountNotReady {
            self.after_attempt(phone, route, &response);
        } else {
            self.notify_request_observers(phone, route, &response);
        }
        response
    }

    /// The platform request of a [`DrmAppRequest`]: option `IgnoreDeviceDisabled`, bounded by the
    /// request's remaining time and the router's own timeout.
    async fn send_prepared_drm(&self, phone: &DcgClient, prepared: &Prepared, deadline: Instant) -> AppResponse {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let router_cancel = CancellationToken::new();
        let request = PlatformRequest {
            target: phone.clone(),
            message: prepared.message.clone(),
            priority: prepared.priority,
            options: SendOptions { ignore_device_disabled: true, ..SendOptions::NONE },
            timeout: remaining.min(self.config.platform.request_timeout),
            trace: prepared.trace.clone(),
            cancel: router_cancel.clone(),
        };

        let PlatformResponse { status, message } = tokio::select! {
            r = self.messenger.request(request) => r,
            _ = tokio::time::sleep_until(deadline) => {
                router_cancel.cancel();
                return AppResponse::failed(AppOutcome::TimedOut);
            }
        };

        drm_response(status, message)
    }

    /// The local gates in order: `AsyncTrust`, the session lock, the account.
    async fn local_gate(&self, phone: &DcgClient, user_initiated: bool) -> Option<LocalGate> {
        let Some(record) = self.phone_record() else {
            return Some(LocalGate::NoPhone);
        };

        if !user_initiated && record.is_from_device_list() && !record.is_async_trust_capable() {
            return Some(LocalGate::AsyncTrust);
        }
        if self.is_locked() {
            return Some(LocalGate::Locked);
        }
        if !self.account.is_ready(phone).await {
            return Some(LocalGate::AccountNotReady);
        }
        None
    }

    /// The time-out outcome: *internet error* while offline, else *timed out*.
    fn timed_out(&self) -> AppOutcome {
        if self.edges.reachability.is_internet_available() { AppOutcome::TimedOut } else { AppOutcome::InternetError }
    }

    /// The connection policy and the wait for the connection: when the phone is not connected,
    /// start a connect with the request's policy, bounded by the request timeout. The connect
    /// returns when the connection is connected or disconnected, or when its feedback window ends;
    /// the request then goes on only when the phone is connected. Otherwise it fails at once —
    /// *connect timed out* when the whole timeout passed (*internet error* when offline), else with
    /// the connection's failure: the last failure the connect reports, *unknown* when it reports
    /// none. It does not wait past the connect. The `Err` is the request's outcome.
    async fn ensure_connected(
        &self,
        phone: &DcgClient,
        prepared: &Prepared,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<(), AppOutcome> {
        if self.sessions.is_connected(phone) {
            return Ok(());
        }

        // User-initiated → NewSession + IgnoreDeviceDisabled; otherwise Default.
        let request = ConnectRequest {
            policy: if prepared.user_initiated { StartPolicy::NewSession } else { StartPolicy::Default },
            ignore_device_disabled: prepared.user_initiated,
            feedback_window: self.config.platform.connection_timeout,
            trace: prepared.trace.clone(),
        };

        let outcome = tokio::select! {
            o = self.sessions.connect(phone, request) => o,
            _ = tokio::time::sleep_until(deadline) => return Err(self.timed_out()),
            _ = cancel.cancelled() => return Err(AppOutcome::Canceled),
        };

        match outcome {
            ConnectOutcome::Connected => Ok(()),
            ConnectOutcome::Failed(failure) => Err(outcome_for_connection_failure(failure)),
            // The feedback window ended without a connection: the state decides now.
            ConnectOutcome::Pending { last_failure } => {
                if self.sessions.is_connected(phone) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(self.timed_out());
                }
                Err(last_failure.map_or(AppOutcome::Unknown, outcome_for_connection_failure))
            }
        }
    }

    async fn send_prepared(
        &self,
        phone: &DcgClient,
        prepared: &Prepared,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> AppResponse {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // The router's own cancellation: fired on the deadline too, so it can drop its pending entry.
        let router_cancel = cancel.child_token();
        let request = PlatformRequest {
            target: phone.clone(),
            message: prepared.message.clone(),
            priority: prepared.priority,
            options: SendOptions {
                ignore_device_disabled: prepared.user_initiated,
                // The router adds it to every request anyway; a send never wakes the phone.
                skip_wake_if_not_connected: true,
                ..SendOptions::NONE
            },
            timeout: remaining.min(self.config.platform.request_timeout),
            trace: prepared.trace.clone(),
            cancel: router_cancel.clone(),
        };

        self.tap.emit(TapEvent::new(
            TapLayer::App,
            Direction::Out,
            Some(phone.to_string()),
            tap_summary("request", &prepared.message.headers, Some(&prepared.values)),
            Some(&prepared.message.payload),
        ));

        let PlatformResponse { status, message } = tokio::select! {
            r = self.messenger.request(request) => r,
            _ = tokio::time::sleep_until(deadline) => {
                router_cancel.cancel();
                return AppResponse::failed(AppOutcome::TimedOut);
            }
            _ = cancel.cancelled() => return AppResponse::failed(AppOutcome::Canceled),
        };

        match (status, message) {
            (PlatformRequestStatus::Success, Some(message)) => self.read_response(phone, &prepared.route, &message),
            (PlatformRequestStatus::Success, None) => {
                warn!(route = %prepared.route, "App request succeeded without a response message; the request fails");
                AppResponse::failed(AppOutcome::Unknown)
            }
            (status, _) => AppResponse::failed(outcome_for_platform_status(&status)),
        }
    }

    /// Reads a response and maps its `result` to the request's outcome. A response whose payload is
    /// not a `ValueSet` (it does not inflate or parse) fails the request as *failure* and is
    /// logged; nothing of the message is read — not even a `result` envelope header — so the
    /// attempt sets the "all features disabled" flag `false`.
    fn read_response(&self, phone: &DcgClient, route: &str, message: &TransportMessage) -> AppResponse {
        match read_app_message(message) {
            Ok(parsed) => {
                self.tap.emit(TapEvent::new(
                    TapLayer::App,
                    Direction::In,
                    Some(phone.to_string()),
                    tap_summary("response", &parsed.headers, Some(&parsed.values)),
                    Some(&message.payload),
                ));

                let result = parsed.result();
                let contract = match parsed.contract_version() {
                    ContractVersion::Number(v) => Some(v),
                    ContractVersion::Missing | ContractVersion::NotANumber => None,
                };

                AppResponse {
                    outcome: outcome_for_result(result, self.config.app.contract_version, contract),
                    result: Some(result),
                    message: Some(parsed),
                    local_gate: None,
                    canceled_by_caller: false,
                    resource_payload: None,
                }
            }
            Err(e) => {
                warn!(
                    route,
                    error = %e,
                    original_request_id = ?message.headers.get(header::ORIGINAL_REQUEST_ID),
                    "App response payload is not a ValueSet; the request fails"
                );

                self.tap.emit(TapEvent::new(
                    TapLayer::Undecodable,
                    Direction::In,
                    Some(phone.to_string()),
                    json!({"layer": "app", "kind": "response", "error": e.to_string(),
                           "headers": message.headers.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}),
                    Some(&message.payload),
                ));

                AppResponse::failed(AppOutcome::Failure)
            }
        }
    }

    /// Every request that passes the `AsyncTrust` and lock gates sets the "all features disabled"
    /// flag: `true` when the response's `result` is `13` or the request failed with *LTW disabled*
    /// — a wake of the send, or of the connect the request waited for — else `false`. A request the
    /// account gate stopped, one that timed out, and one whose response could not be read set
    /// `false`: they failed with neither. A request its caller canceled does not come here.
    fn after_attempt(&self, phone: &DcgClient, route: &str, response: &AppResponse) {
        let disabled = response.result == Some(super::consts::result_code::ALL_FEATURES_DISABLED)
            || response.outcome.is_ltw_disabled();
        self.update_record_for(phone, |r| r.all_features_disabled = disabled);
        self.notify_request_observers(phone, route, response);
    }
}

#[cfg(test)]
mod tests;
