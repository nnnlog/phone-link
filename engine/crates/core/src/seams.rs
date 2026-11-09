//! The interfaces between layers that different modules implement: auth ↔ relay ↔ dcg ↔ platform.
//!
//! Each trait sits on one boundary between protocol layers. Adapters between them are wired in
//! the `engine` module of `phonelink-engine`.

use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::codec::hub::MultiplexPacket;
use crate::types::{
    ConnectReason, DcgClient, DcgSendStatus, Environment, HubSendResult, HubState, OpenStatus, Opened, OutboundMessage,
    PartnerDisconnectReason, TraceContext, TransportMessage, WakeOptions, WakeStatus, Woken,
};

// ------------------------------------------------------------------------------------------------
// auth → relay: the DCG services token
// ------------------------------------------------------------------------------------------------

/// Why a services token could not be produced. [`TokenError::open_status`] is the relay's mapping
/// onto `OpenStatus`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("the MSA token needs user interaction")]
    UserInteractionRequired,
    #[error("DCG answered Unauthorized: {0}")]
    Unauthorized(String),
    #[error("no internet connectivity")]
    Internet,
    #[error("an auth REST call failed: {0}")]
    Http(String),
    #[error("transient key-store failure: {0}")]
    KeyStoreTransient(String),
    #[error("crypto failure: {0}")]
    Crypto(String),
    /// The MSA token provider failed for a reason other than needing the user: the relay's start
    /// retry filter does not retry it, and it opens as `NonTransientAuthFailure` (or
    /// `InternetError` when the root cause is no connectivity). An MSA token endpoint that cannot
    /// be reached is `TokenError::Internet` instead (`AuthError::to_token_error`).
    #[error("the MSA token provider failed: {0}")]
    Msa(String),
    #[error("no provisioned identity for this environment")]
    NotProvisioned,
    #[error("{0}")]
    Other(String),
}

impl TokenError {
    pub fn open_status(&self) -> OpenStatus {
        match self {
            TokenError::UserInteractionRequired => OpenStatus::MsaTokenUserInteractionRequired,
            TokenError::Unauthorized(_) | TokenError::KeyStoreTransient(_) => OpenStatus::AuthTransientFailure,
            TokenError::Http(_) => OpenStatus::AuthHttpRequestFailure,
            TokenError::Crypto(_) => OpenStatus::CryptoManagerFailure,
            TokenError::Internet => OpenStatus::InternetError,
            TokenError::Msa(_) | TokenError::NotProvisioned | TokenError::Other(_) => {
                OpenStatus::NonTransientAuthFailure
            }
        }
    }
}

/// The DCG services token and its refresh policy.
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// The services token of `env`: cached unless missing or expired (`exp − 10 min < now`); the
    /// refresh is automatic, so the cache need never be forced.
    async fn services_token(&self, env: Environment, force_refresh: bool) -> Result<String, TokenError>;

    /// Invalidates the cached services token of `env`, so the next request refreshes it automatically.
    /// A server-side 401 is the signal to drop the cache. The default does nothing.
    async fn invalidate_services_token(&self, _env: Environment) {}
}

// ------------------------------------------------------------------------------------------------
// auth → dcg and platform: the trust store
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustRotationError {
    #[error("no crypto-trust relationship with the partner")]
    NotFound,
    #[error("the partner certificate is missing or invalid")]
    InvalidCertificate,
    #[error("trust store failure: {0}")]
    Store(String),
}

/// Trust validation, used by the fragment layer's inbound gate and the key-rotation routes.
pub trait TrustGate: Send + Sync {
    /// An unexpired relationship names `partner` (and, for `AsyncD2D`, its `A2D` anchor is valid).
    fn is_trusted(&self, partner: &DcgClient) -> bool;
    /// The partner key expiration time of the relationship with `partner`, if any.
    fn partner_key_expiration(&self, partner: &DcgClient) -> Option<SystemTime>;
    /// Stores `certificate_der` as the partner's new certificate (the previous one becomes the
    /// temporary certificate for its grace window).
    fn rotate_partner_certificate(&self, partner: &DcgClient, certificate_der: &[u8])
    -> Result<(), TrustRotationError>;
    /// An inbound message or wake came from an unknown `dcgClientId`: the trust store may run a
    /// `ForceCloud` refresh (at most the configured number per source per hour).
    fn on_untrusted_source(&self, partner: &DcgClient) {
        let _ = partner;
    }
}

// ------------------------------------------------------------------------------------------------
// relay ↔ auth: waking the phone
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("no OnConnected payload within the wait")]
pub struct HubRegionTimeout;

/// What the wake sequence needs from the hub connection: the flush, the region and the partner wait.
#[async_trait]
pub trait WakeHub: Send + Sync {
    /// `SendConnectedAsync(target)`, awaited for its completion.
    async fn flush_connected(&self, target: &DcgClient) -> HubSendResult;
    /// The `OnConnected` payload's `RegionName` of `env`'s connection (`Ok(None)` when the
    /// payload has none), waiting up to `timeout` for the payload.
    async fn hub_region(&self, env: Environment, timeout: Duration) -> Result<Option<String>, HubRegionTimeout>;
    /// Waits up to `timeout` for `target` to appear in the partner set.
    async fn wait_partner_connected(&self, target: &DcgClient, timeout: Duration) -> bool;
}

/// The wake sequence, used by the per-partner connection's attempts and by the stop-and-wait sender's
/// send-path wake.
///
/// The waker keeps no TTL state: each caller passes the TTL — the attempts the
/// connection's alternating one, a send the default.
#[async_trait]
pub trait Waker: Send + Sync {
    /// Runs one wake whose Dispatcher requests carry `ttl` (and whose partner wait is
    /// `max(ttl, 10 s)`); cancelling `cancel` during the partner wait sends the cancel wake and
    /// returns `Cancelled`. A wake already in flight for the target is awaited instead, with its
    /// own options and TTL.
    async fn wake(&self, target: &DcgClient, options: WakeOptions, ttl: Duration, cancel: CancellationToken) -> Woken;
    /// Resets every wake circuit breaker: the PC's session was unlocked or it resumed from sleep.
    fn reset_breakers(&self);
}

// ------------------------------------------------------------------------------------------------
// relay ↔ dcg: packets
// ------------------------------------------------------------------------------------------------

/// How the fragment layer reaches the hub: the hub RPCs that carry a packet.
#[async_trait]
pub trait HubLink: Send + Sync {
    /// `SendMessageAsync` (no `connection_session_id`) or `SendSessionBasedMessageAsync`, awaited for
    /// its completion; `NotConnected` without invoking when the hub connection is not `Connected`.
    async fn send_packet(
        &self,
        target: &DcgClient,
        trace: &TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    ) -> HubSendResult;
    /// The hub reports `partner` connected (it is in the connection's partner set).
    fn is_partner_connected(&self, partner: &DcgClient) -> bool;
    /// Removes `partner` from its connection's partner set with `reason` and reports
    /// `PartnerDisconnected` — the fragment layer's partner-disconnect breaker after too many ack
    /// timeouts (`TooManyAckTimeouts`). A partner that is not in the set is left alone.
    fn remove_partner(&self, partner: &DcgClient, reason: PartnerDisconnectReason);
}

/// Inbound `OnReceiveMessage` / `OnReceiveSessionBasedMessage` packets, handed to the DCG layer in
/// arrival order.
///
/// `trace` is already the inbound message's context (packet `TraceId` and `ParentId`
/// kept, new correlation id; a local context when the packet's was nil).
pub trait PacketSink: Send + Sync {
    fn on_packet(
        &self,
        sender: DcgClient,
        trace: TraceContext,
        packet: MultiplexPacket,
        connection_session_id: Option<String>,
    );
}

/// The per-partner connection's answer to a send that may wake: the stop-and-wait sender's wake
/// step, for a message without *skip wake*.
#[async_trait]
pub trait PartnerConnector: Send + Sync {
    /// `trace` is the message's trace context: a hub connection opened here carries it on its
    /// headers (a connection's trace headers are those of the first send).
    async fn connect_for_send(
        &self,
        target: &DcgClient,
        ignore_device_disabled: bool,
        max_wait: Duration,
        trace: &TraceContext,
    ) -> Result<(), DcgSendStatus>;
}

// ------------------------------------------------------------------------------------------------
// dcg ↔ platform: messages
// ------------------------------------------------------------------------------------------------

/// Reassembled inbound messages, handed up by the fragment receiver.
pub trait MessageSink: Send + Sync {
    fn on_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    );
}

/// Sending one message through the fragment layer.
#[async_trait]
pub trait MessageTransport: Send + Sync {
    async fn send(&self, message: OutboundMessage) -> DcgSendStatus;
}

// ------------------------------------------------------------------------------------------------
// relay → platform: the hub connection and partner presence
// ------------------------------------------------------------------------------------------------

/// What the relay reports upward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayEvent {
    /// The partner entered the partner set (`OnPartnerConnected`, or a message from it).
    PartnerConnected(DcgClient),
    /// The partner left the partner set.
    PartnerDisconnected { partner: DcgClient, reason: PartnerDisconnectReason },
    /// The hub connection of `env` changed state.
    HubState { env: Environment, state: HubState },
    /// An `OnConnected` payload arrived.
    OnConnected { env: Environment, region: Option<String> },
}

/// Control of the hub connections, used by the per-partner connection's attempts.
#[async_trait]
pub trait HubControl: Send + Sync {
    /// Gets or creates the connection serving `partner` (created with `DCG-PartnerId = partner`, no
    /// `DCG-HubRegion`) and opens it. `trace` is the trace context of the request that caused the
    /// open; it goes on the connection headers when this call creates the connection.
    async fn open(&self, partner: &DcgClient, reason: ConnectReason, trace: &TraceContext) -> Opened;
    fn hub_state(&self, env: Environment) -> HubState;
    fn is_partner_connected(&self, partner: &DcgClient) -> bool;
    /// `SendConnectedAsync(partner)` (re-announce).
    async fn send_connected(&self, partner: &DcgClient) -> HubSendResult;
    fn subscribe(&self) -> broadcast::Receiver<RelayEvent>;
    /// Stops the connection of `env` (when it is not `Disconnected`).
    async fn close(&self, env: Environment);
}

// ------------------------------------------------------------------------------------------------
// platform → app: requests, responses and inbound App messages
// ------------------------------------------------------------------------------------------------

/// `PlatformRequestStatus`: the outcome of a router request, including the phone's rejections
/// carried on `/internal/response`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlatformRequestStatus {
    Success,
    Canceled,
    NoResponse,
    /// The fragment layer could not deliver the request; the cause is kept for the callers' mappings.
    RequestSendFailed(DcgSendStatus),
    ResponseTimedOut,
    RemoteSyncNotAllowed,
    RemoteVersionTooLow,
    RemoteVersionTooHigh,
    RemoteAllSyncDisabled,
    RemoteMeteredSyncDisabled,
    RemoteNotSubscribedToRoute,
}

impl PlatformRequestStatus {
    /// The text of what failed, for a send whose hub open or wake failed with one.
    pub fn cause(&self) -> Option<&str> {
        match self {
            Self::RequestSendFailed(status) => status.cause(),
            _ => None,
        }
    }
}

/// One platform request (Platform- or App-typed).
#[derive(Debug, Clone)]
pub struct PlatformRequest {
    pub target: DcgClient,
    /// Headers may already carry `_requestId` (App requests mint a GUID); the router keeps it.
    pub message: TransportMessage,
    pub priority: u32,
    pub options: crate::types::SendOptions,
    /// Bounds send plus wait (120 s by default; callers may pass less).
    pub timeout: Duration,
    pub trace: TraceContext,
    pub cancel: CancellationToken,
}

/// The outcome of a request: `message` is the response when `status == Success` and the request
/// was not one-way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformResponse {
    pub status: PlatformRequestStatus,
    pub message: Option<TransportMessage>,
}

/// The router's sending side.
#[async_trait]
pub trait PlatformMessenger: Send + Sync {
    /// Sends `request` and waits for its answer.
    ///
    /// It resolves `_requestId`, adds `_rejectionVersion: "1"`, sends, correlates the
    /// `/internal/response` and maps rejections.
    async fn request(&self, request: PlatformRequest) -> PlatformResponse;
    /// Answers `request` (which must carry `_requestId`): forces `_route =
    /// /internal/response`, sets `_originalRequestId` and `_rejectionVersion: "1"`, keeps
    /// `response`'s own message type, sends with `IgnoreDeviceDisabled | SkipWakeIfNotConnected`.
    async fn respond(
        &self,
        target: &DcgClient,
        request: &TransportMessage,
        response: TransportMessage,
        priority: u32,
        trace: TraceContext,
    ) -> DcgSendStatus;
}

/// Inbound App messages that are neither responses nor device-proxy traffic, from the start of the
/// App layer's inbound dispatch.
pub trait AppInbound: Send + Sync {
    fn on_app_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    );
}

// ------------------------------------------------------------------------------------------------
// platform → app: the per-partner connection
// ------------------------------------------------------------------------------------------------

/// Start policy of a connect request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartPolicy {
    Default,
    NewSession,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    pub policy: StartPolicy,
    pub ignore_device_disabled: bool,
    /// How long the request waits for connected/disconnected before reporting the current state
    /// (70 s for App requests).
    pub feedback_window: Duration,
    /// The trace context of the request that caused the connect; a hub connection this connect
    /// creates carries it on its headers.
    pub trace: TraceContext,
}

/// Why the connection to a partner is not up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionFailure {
    /// The hub connection could not be opened.
    Hub(OpenStatus),
    /// The wake did not bring the partner onto the hub.
    Wake(WakeStatus),
    /// `ConnectionDropped` (own hub connection lost) — `LocalHubDisconnect`.
    ConnectionDropped,
    /// `PartnerDisconnect` — every other partner disconnect reason.
    PartnerDisconnect(PartnerDisconnectReason),
    /// The phone ended the session (`/PlatformDisconnectRequest`); only a `NewSession` start leaves it.
    SessionEnded { reason: i32 },
    /// The connection was stopped locally.
    LocalStop,
}

/// What a connect request reports at the end of its feedback window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    Connected,
    /// The window ended while attempts go on; the last failure, if any.
    Pending {
        last_failure: Option<ConnectionFailure>,
    },
    /// The connection is in a state the request cannot start from, or reported a failure.
    Failed(ConnectionFailure),
}

/// Session-level events for the App layer (connect-message triggers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Connected(DcgClient),
    Disconnected { partner: DcgClient, failure: ConnectionFailure },
}

/// The per-partner connections: their start policies and session control.
#[async_trait]
pub trait PartnerSessionControl: Send + Sync {
    async fn connect(&self, partner: &DcgClient, request: ConnectRequest) -> ConnectOutcome;
    fn is_connected(&self, partner: &DcgClient) -> bool;
    fn subscribe(&self) -> broadcast::Receiver<SessionEvent>;
}
