//! Vocabulary shared by every layer of the engine.
//!
//! It covers environments, DCG clients, ordered envelope headers, transport messages, trace
//! contexts, send options, and the result enums one layer reports to another.

use std::fmt;

use rand::RngCore;
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------------------------------------
// Environments and partners
// ------------------------------------------------------------------------------------------------

/// A DCG environment.
///
/// The legacy environment maps to no host and is never connected, so it is not
/// representable here; trust records that carry the legacy `dcg_client_id` attribute are handled
/// inside the trust store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Environment {
    Prod,
    Beta,
    Dogfood,
}

impl Environment {
    pub const ALL: [Environment; 3] = [Environment::Prod, Environment::Beta, Environment::Dogfood];

    /// The name used on the wire (`DCG-Environment` wake key, `DcgClient` string form).
    pub fn name(self) -> &'static str {
        match self {
            Environment::Prod => "Prod",
            Environment::Beta => "Beta",
            Environment::Dogfood => "Dogfood",
        }
    }

    /// Parses a wire name exactly as written by [`Environment::name`].
    pub fn from_name(name: &str) -> Option<Self> {
        Environment::ALL.into_iter().find(|e| e.name() == name)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A DCG partner: `{ dcgClientId, environment }`, string form `<dcgClientId>.<Environment>`.
///
/// The id is kept exactly as received; comparisons that the protocol defines as case-insensitive
/// (trust lookups) do their own folding.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DcgClient {
    pub id: String,
    pub env: Environment,
}

impl DcgClient {
    pub fn new(id: impl Into<String>, env: Environment) -> Self {
        Self { id: id.into(), env }
    }

    /// Whether `other` names the same client: the same environment and the same id, ignoring ASCII
    /// case.
    pub fn is_same_client(&self, other: &DcgClient) -> bool {
        self.env == other.env && self.id.eq_ignore_ascii_case(&other.id)
    }
}

impl fmt::Display for DcgClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.id, self.env)
    }
}

// ------------------------------------------------------------------------------------------------
// Envelope headers and transport messages
// ------------------------------------------------------------------------------------------------

/// Ordered `string → string` map with unique keys: the header map of the binary inner envelope,
/// written in insertion order. Inserting an existing key replaces its value in place; a new key is
/// appended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderMap(Vec<(String, String)>);

/// A second occurrence of a header key, which makes the envelope malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("duplicate header key {0:?}")]
pub struct DuplicateHeader(pub String);

impl HeaderMap {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Builds a map from pairs, failing on a duplicate key.
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, DuplicateHeader> {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            let k = k.into();
            if map.contains(&k) {
                return Err(DuplicateHeader(k));
            }
            map.0.push((k, v.into()));
        }
        Ok(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }

    /// Sets `key` to `value`, keeping the key's position when it already exists.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        let idx = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(idx).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }
}

/// `TransportMessageType` as the `SignalR` path numbers it: `App = 0`, `Platform = 1`.
///
/// The receiver maps every value but `1` (and a missing property) to `App`, so `Unknown` is not
/// representable on this path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageType {
    App,
    Platform,
}

impl MessageType {
    /// The `MessageType` property value written on a fragment.
    pub fn wire_value(self) -> i64 {
        match self {
            MessageType::App => 0,
            MessageType::Platform => 1,
        }
    }

    /// Receive rule: present and `== 1` ⇒ Platform; absent or any other value ⇒ App.
    pub fn from_wire(value: Option<i64>) -> Self {
        if value == Some(1) { MessageType::Platform } else { MessageType::App }
    }
}

/// One upper-layer message as the fragment layer carries it: message type, envelope headers and
/// payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportMessage {
    pub message_type: MessageType,
    pub headers: HeaderMap,
    pub payload: Vec<u8>,
}

impl TransportMessage {
    pub fn new(message_type: MessageType, headers: HeaderMap, payload: Vec<u8>) -> Self {
        Self { message_type, headers, payload }
    }

    pub fn route(&self) -> Option<&str> {
        self.headers.get(header::ROUTE)
    }
}

/// Envelope header names and content types.
pub mod header {
    /// Envelope header naming the route a message is addressed to.
    pub const ROUTE: &str = "_route";
    /// Envelope header carrying the id of a request that expects a response.
    pub const REQUEST_ID: &str = "_requestId";
    /// Envelope header on a response naming the request it answers.
    pub const ORIGINAL_REQUEST_ID: &str = "_originalRequestId";
    /// Envelope header on a request saying the sender can read rejection responses.
    pub const REJECTION_VERSION: &str = "_rejectionVersion";
    /// Envelope header on a rejection response giving the reason.
    pub const REJECTED_REASON: &str = "_rejectedReason";
    /// Envelope header giving the payload's content type.
    pub const MS_CONTENT_TYPE: &str = "ms-content-type";
    /// Envelope header set to `true` when the payload is gzip-compressed.
    pub const IS_GZIPPED_COMPRESSED: &str = "_isGzippedCompressed";
    /// App message kind; accepted inbound as a header or a `ValueSet` key.
    pub const CONTENT_TYPE: &str = "contentType";
    /// Correlation vector; accepted inbound as a header or a `ValueSet` key.
    pub const CORRELATION_VECTOR: &str = "correlationVector";
    /// Sender's App contract version; accepted inbound as a header or a `ValueSet` key.
    pub const CONTRACT_VERSION: &str = "contractVersion";
    /// Envelope header on an App response carrying the result code.
    pub const RESULT: &str = "result";

    /// `ms-content-type` value of a binary payload, such as a `ValueSet`.
    pub const CONTENT_TYPE_BINARY: &str = "application/x-binary";
    /// `ms-content-type` value of a JSON payload.
    pub const CONTENT_TYPE_JSON: &str = "text/json";
    /// The `_route` of every response and rejection.
    pub const RESPONSE_ROUTE: &str = "/internal/response";
}

// ------------------------------------------------------------------------------------------------
// Trace context
// ------------------------------------------------------------------------------------------------

/// The trace-state keys that name what a request belongs to.
pub mod trace_key {
    /// The scenario (the operation) a request belongs to.
    pub const SCENARIO: &str = "ypScenarioId";
    /// What started the scenario.
    pub const TRIGGER: &str = "ypTriggerId";
}

/// Trace-state entries naming `scenario` and then `trigger`.
pub fn trace_state(scenario: &str, trigger: &str) -> Vec<(String, String)> {
    vec![(trace_key::SCENARIO.to_owned(), scenario.to_owned()), (trace_key::TRIGGER.to_owned(), trigger.to_owned())]
}

/// Trace-state entries naming only `scenario`.
pub fn scenario_trace_state(scenario: &str) -> Vec<(String, String)> {
    vec![(trace_key::SCENARIO.to_owned(), scenario.to_owned())]
}

/// The W3C-style trace context attached to REST calls, hub connections and hub invocations: the
/// `traceparent` / `tracestate` request headers and the hub's trace context packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceContext {
    /// 32 lower-case hex characters (16 random non-zero bytes).
    pub trace_id: String,
    /// The correlation id of the context this one was derived from, when any.
    pub parent_id: Option<String>,
    /// 16 lower-case hex characters (8 random non-zero bytes).
    pub correlation_id: String,
    /// `0` on every PC-generated context.
    pub flags: u8,
    /// Ordered trace-state entries (`ypScenarioId`, `ypTriggerId`, …).
    pub trace_state: Vec<(String, String)>,
}

impl TraceContext {
    /// A fresh root context with the given trace-state entries.
    pub fn new_root(trace_state: Vec<(String, String)>) -> Self {
        Self {
            trace_id: random_trace_id(),
            parent_id: None,
            correlation_id: random_nonzero_hex(8),
            flags: 0,
            trace_state,
        }
    }

    /// A child: same trace id and state, parent = this context's correlation id, new correlation id.
    pub fn child(&self) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            parent_id: Some(self.correlation_id.clone()),
            correlation_id: random_nonzero_hex(8),
            flags: self.flags,
            trace_state: self.trace_state.clone(),
        }
    }

    /// A request made within this context's operation: a child carrying the request's own trace state.
    pub fn for_request(&self, trace_state: Vec<(String, String)>) -> Self {
        Self { trace_state, ..self.child() }
    }

    /// `traceparent: 00-<trace-id>-<correlation-id>-<flags>` with the flags as two upper-case hex digits.
    pub fn traceparent(&self) -> String {
        format!("00-{}-{}-{:02X}", self.trace_id, self.correlation_id, self.flags)
    }

    /// `tracestate` (and `DCG-TraceState`): `k=v,…,yppHttpParent=<correlation-id>`.
    pub fn tracestate(&self) -> String {
        let mut parts: Vec<String> = self.trace_state.iter().map(|(k, v)| format!("{k}={v}")).collect();
        parts.push(format!("yppHttpParent={}", self.correlation_id));
        parts.join(",")
    }
}

/// A trace id (as in `traceparent`): 16 random non-zero bytes as 32 lower-case hex characters.
pub fn random_trace_id() -> String {
    random_nonzero_hex(16)
}

/// `n` random bytes, each non-zero, as `2n` lower-case hex characters: how trace ids and
/// correlation ids are generated.
pub fn random_nonzero_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    let mut rng = rand::rng();
    for b in bytes.iter_mut() {
        while *b == 0 {
            *b = (rng.next_u32() & 0xFF) as u8;
        }
    }
    hex::encode(bytes)
}

/// `n` random bytes as `2n` lower-case hex characters.
pub fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// A fresh GUID rendered `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lower-case.
pub fn new_guid_lower() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

// ------------------------------------------------------------------------------------------------
// Send options and results
// ------------------------------------------------------------------------------------------------

/// Platform request options of the router's send path: `None 0`, `OneWay 1`, `IgnoreDeviceDisabled 2`,
/// `SkipWakeIfNotConnected 4`, `SkipPartnerAck 8`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SendOptions {
    pub one_way: bool,
    pub ignore_device_disabled: bool,
    pub skip_wake_if_not_connected: bool,
    pub skip_partner_ack: bool,
}

impl SendOptions {
    pub const NONE: SendOptions = SendOptions {
        one_way: false,
        ignore_device_disabled: false,
        skip_wake_if_not_connected: false,
        skip_partner_ack: false,
    };

    pub fn bits(self) -> u32 {
        (self.one_way as u32)
            | (self.ignore_device_disabled as u32) << 1
            | (self.skip_wake_if_not_connected as u32) << 2
            | (self.skip_partner_ack as u32) << 3
    }
}

/// One message handed to the fragment layer's stop-and-wait sender.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub target: DcgClient,
    pub message: TransportMessage,
    /// `0` high, `50` medium, `100` low.
    pub priority: u32,
    pub options: SendOptions,
    /// `None` for every PC-originated message; echoed only for replies to traffic that arrived via
    /// `OnReceiveSessionBasedMessage`.
    pub connection_session_id: Option<String>,
    pub trace: TraceContext,
}

/// The result of one hub invocation.
/// `Throttled` and `MsaTokenUserInteractionRequired` are unreachable on the send path and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HubSendResult {
    Success,
    NotConnected,
    InternetError,
    Failure,
    BrokenCircuitError,
}

/// Outcome of one message through the fragment layer, from the send attempt through the ack.
///
/// A failed hub open or wake keeps its `cause` as text, because the status is cloned and compared.
/// `Debug` leaves the cause out, because the status is logged under the public name `status`; the
/// site that logs the failure adds [`DcgSendStatus::cause`] as its `error`, and `Display` (the text
/// the app is handed) ends with it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DcgSendStatus {
    Success,
    /// *skip wake* was set and the hub does not report the partner connected.
    PartnerNotConnected,
    /// The hub send failed (after the hub-send retries, where they apply), or the hub connection
    /// the send opened did not open.
    HubSendFailed {
        result: HubSendResult,
        cause: Option<String>,
    },
    /// The partner was not connected and waking it failed.
    WakeFailed {
        status: WakeStatus,
        cause: Option<String>,
    },
    /// No ack after the configured attempts (`AckTimedOut` → message `TimedOut`).
    TimedOut,
    /// A failed ack with `ErrorNo 1`.
    RemoteTransportTooOld,
    /// A failed ack synthesized for an ack with `Version > 1.0`.
    RemoteTransportTooNew,
    /// A failed ack with any other `ErrorNo`.
    RemoteTransportError,
    /// The ack came from a source that is no longer trusted.
    LostTrustWithPartner,
    Canceled,
}

impl DcgSendStatus {
    /// A failed hub open of the send path as the fragment layer reports it, keeping the open's cause.
    pub fn open_failed(opened: Opened) -> Self {
        let result = match opened.status {
            OpenStatus::InternetError => HubSendResult::InternetError,
            OpenStatus::BrokenCircuitError => HubSendResult::BrokenCircuitError,
            OpenStatus::Cancelled => return Self::Canceled,
            _ => HubSendResult::NotConnected,
        };
        Self::HubSendFailed { result, cause: opened.cause }
    }

    /// A failed wake of the send path, keeping the wake's cause.
    pub fn wake_failed(woken: Woken) -> Self {
        Self::WakeFailed { status: woken.status, cause: woken.cause }
    }

    /// The text of what failed, for a failed hub open or wake that has one.
    pub fn cause(&self) -> Option<&str> {
        match self {
            Self::HubSendFailed { cause, .. } | Self::WakeFailed { cause, .. } => cause.as_deref(),
            _ => None,
        }
    }
}

impl fmt::Debug for DcgSendStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => f.write_str("Success"),
            Self::PartnerNotConnected => f.write_str("PartnerNotConnected"),
            Self::HubSendFailed { result, .. } => f.debug_tuple("HubSendFailed").field(result).finish(),
            Self::WakeFailed { status, .. } => f.debug_tuple("WakeFailed").field(status).finish(),
            Self::TimedOut => f.write_str("TimedOut"),
            Self::RemoteTransportTooOld => f.write_str("RemoteTransportTooOld"),
            Self::RemoteTransportTooNew => f.write_str("RemoteTransportTooNew"),
            Self::RemoteTransportError => f.write_str("RemoteTransportError"),
            Self::LostTrustWithPartner => f.write_str("LostTrustWithPartner"),
            Self::Canceled => f.write_str("Canceled"),
        }
    }
}

impl fmt::Display for DcgSendStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.cause() {
            Some(cause) => write!(f, "{self:?}: {cause}"),
            None => write!(f, "{self:?}"),
        }
    }
}

/// `PartnerDisconnectReason`, which also gates reconnection. `None` and `RemoteQueryFailure` are never
/// raised and omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PartnerDisconnectReason {
    NoticeFromHub,
    LocalHubDisconnect,
    TooManyAckTimeouts,
    SystemReset,
    ExceededMissedHeartbeats,
}

impl PartnerDisconnectReason {
    pub fn should_reconnect(self) -> bool {
        !matches!(self, PartnerDisconnectReason::NoticeFromHub)
    }
}

/// Library states of one hub connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HubState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// `OpenStatus`; the two iOS-only values are omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OpenStatus {
    Success,
    Failure,
    InternetError,
    CryptoManagerFailure,
    AuthTransientFailure,
    AuthHttpRequestFailure,
    NonTransientAuthFailure,
    BrokenCircuitError,
    Cancelled,
    MsaTokenUserInteractionRequired,
}

impl OpenStatus {
    /// Fatal statuses stop further attempts.
    pub fn is_fatal(self) -> bool {
        matches!(
            self,
            OpenStatus::CryptoManagerFailure
                | OpenStatus::NonTransientAuthFailure
                | OpenStatus::MsaTokenUserInteractionRequired
        )
    }
}

/// `connectReason` of the open sequence — telemetry only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConnectReason {
    SendFragment,
    SendAcknowledgement,
    WakeRequest,
    ConnectRequested,
    PersistentConnection,
    AutomaticReconnect,
}

/// Options of one wake: the `/Dispatcher/Wake` `data` keys `DCG-RequestNewSession` and
/// `IgnoreDeviceDisabledStatus`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WakeOptions {
    pub request_new_session: bool,
    pub ignore_device_disabled: bool,
}

/// Outcome of one wake: the Dispatcher statuses plus the transport failures of the hub flush and the
/// wait for `OnConnected`, and the breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WakeStatus {
    Success,
    Cancelled,
    DeviceUnreachable,
    TransportInternetError,
    TransportNetworkError,
    Failure,
    InternetError,
    Unauthorized,
    MsaTokenUserInteractionRequired,
    DeviceNotFoundError,
    IncompatibleRegionError,
    LtwDisabled,
    WakeBrokenCircuitError,
}

/// How a hub open ended: its status and, for a failure, the text of what failed, which the code
/// that handles the failure logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub status: OpenStatus,
    pub cause: Option<String>,
}

impl From<OpenStatus> for Opened {
    fn from(status: OpenStatus) -> Self {
        Self { status, cause: None }
    }
}

/// How a wake ended: its status and, for a failure with more to say than the status, the text of
/// what failed, which the code that handles the failure logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Woken {
    pub status: WakeStatus,
    pub cause: Option<String>,
}

impl From<WakeStatus> for Woken {
    fn from(status: WakeStatus) -> Self {
        Self { status, cause: None }
    }
}

// ------------------------------------------------------------------------------------------------
// DCG device list
// ------------------------------------------------------------------------------------------------

/// One `DeviceInfoItem` of `GET /DeviceAuthProxy/GetDeviceInfoList`. Optional fields keep the server's
/// `null`/absence; the protocol's defaults are applied by the readers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfoItem {
    pub id: String,
    #[serde(default)]
    pub is_linked: Option<bool>,
    #[serde(default)]
    pub certificates: Option<DeviceCertificates>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
    #[serde(default)]
    pub consent_version: Option<i64>,
    #[serde(default)]
    pub custom_data: Option<String>,
    #[serde(default)]
    pub distinguished_device_id: Option<String>,
    #[serde(default)]
    pub global_device_id: Option<String>,
    #[serde(default)]
    pub is_enabled: Option<bool>,
    #[serde(default)]
    pub last_seen_time: Option<i64>,
    #[serde(default)]
    pub manufacture: Option<String>,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub os_name: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub registration_time: Option<i64>,
}

impl DeviceInfoItem {
    /// Whether `osName` is Android (ASCII case-insensitive).
    pub fn is_android(&self) -> bool {
        self.os_name.as_deref().is_some_and(|os| os.eq_ignore_ascii_case("android"))
    }

    /// Whether `capabilities` contains `capability` (exact, case-sensitive; a missing list is empty).
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.as_ref().is_some_and(|c| c.iter().any(|s| s == capability))
    }
}

/// The `certificates` object: `CTSelfSigned` | `SelfSigned` | `PKI` → base64 DER strings. Any other
/// key makes the whole device list invalid (`deny_unknown_fields`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCertificates {
    #[serde(rename = "CTSelfSigned", default, skip_serializing_if = "Option::is_none")]
    pub ct_self_signed: Option<Vec<String>>,
    #[serde(rename = "SelfSigned", default, skip_serializing_if = "Option::is_none")]
    pub self_signed: Option<Vec<String>>,
    #[serde(rename = "PKI", default, skip_serializing_if = "Option::is_none")]
    pub pki: Option<Vec<String>>,
}

#[cfg(test)]
mod tests;
