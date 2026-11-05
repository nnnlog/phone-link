//! Platform routes, request/response correlation and rejection, and the per-partner connection.
//!
//! - [`router`]: receiving, unknown or unsubscribed routes, sending a request, `/internal/response` and
//!   rejection, and the message type and envelope headers.
//! - [`kvp`]: `text/json` bodies.
//! - [`session_validation`]: `/SessionValidation` and its sequence.
//! - [`session_control`]: `/PlatformDisconnectRequest`, `/PlatformActivateSessionRequest`, and the messages
//!   of the session control sequences.
//! - [`key_rotation`]: `/CryptoTrustKeyRotationCheck` and `/CryptoTrustKeyRotation`, the inbound key
//!   rotation sequence.
//! - [`msaep`]: `/Context/Publish` (MSAEP envelope) and its sequence.
//! - [`context_message`]: `/Context/Message` and `/Context/Message/{id}`, inbound, while a context source
//!   exists.
//! - [`drm`]: `/DeviceResourceManager` and its sequence (receivers and the Cross Device Experience Host
//!   request wrapper).
//! - [`device_proxy`]: the device-proxy variant of `/DeviceResourceManager`
//!   (`/DeviceProxyClient/TransportMiddleware`).
//! - [`session`]: connection start policies and session control sequences; the relay's partner disconnect
//!   reasons and reconnection gating, the reactions to the hub connection's automatic-reconnect events, the
//!   trace an open carries in its connection headers, and a request's wait for the connection.
//! - [`routes`]: wiring; registers every route above on a [`Router`].
//! - [`version`]: the four-part version comparison of the phone `clientVersion` check.
//!
//! `/Context/Message/{partnerId}` is registered while the partner has a context source, as Phone Link
//! on Windows does; its messages reach the partner's [`ContextHandle`], over which the Nano device
//! connection exchanges its connection information. The bare `/Context/Message` has no subscriber and
//! is rejected as an unsubscribed route.

pub mod context_message;
pub mod device_proxy;
pub mod drm;
pub mod key_rotation;
pub mod kvp;
pub mod msaep;
pub mod router;
pub mod routes;
pub mod session;
pub mod session_control;
pub mod session_validation;
mod tap_decode;
pub mod version;

#[cfg(feature = "test-support")]
pub use context_message::ContextHub;
pub use context_message::{ContextHandle, ContextRoute, ContextSendError};
#[cfg(feature = "test-support")]
pub use device_proxy::DeviceProxyReceiver;
pub use drm::{
    AnswerDelivery, Drm, DrmAddress, DrmAnswer, DrmFollowUpReceiver, DrmReceiver, DrmReceiverError, DrmRequest,
    DrmSendOutcome, DrmSendRequest, FollowUp,
};
#[cfg(feature = "test-support")]
pub use drm::{DrmChannel, DrmRegistrationError};
#[cfg(feature = "test-support")]
pub use kvp::{Kvp, KvpError};
#[cfg(feature = "test-support")]
pub use msaep::PublishHandle;
pub use msaep::{DeviceFilter, Msaep, Publication, PublicationObserver, PublishOutcome, Subscription};
pub use router::Router;
#[cfg(feature = "test-support")]
pub use router::{InboundMessage, PlatformRouteHandler, RouteRegistrationError};
pub use routes::PlatformDeps;
pub use routes::PlatformRoutes;
pub use session::PartnerSessions;
#[cfg(feature = "test-support")]
pub use session::SessionPhase;
pub use session_validation::SessionValidator;
#[cfg(feature = "test-support")]
pub use session_validation::{AcceptedCapabilities, ContextSource, ValidationOutcome};
pub use version::ClientVersion;

/// Route literals: the inventory of platform routes.
pub mod route {
    /// Responses of both message types.
    pub const INTERNAL_RESPONSE: &str = phonelink_core::types::header::RESPONSE_ROUTE;
    pub const SESSION_VALIDATION: &str = "/SessionValidation";
    pub const PLATFORM_DISCONNECT_REQUEST: &str = "/PlatformDisconnectRequest";
    pub const PLATFORM_ACTIVATE_SESSION_REQUEST: &str = "/PlatformActivateSessionRequest";
    pub const CRYPTO_TRUST_KEY_ROTATION_CHECK: &str = "/CryptoTrustKeyRotationCheck";
    pub const CRYPTO_TRUST_KEY_ROTATION: &str = "/CryptoTrustKeyRotation";
    pub const CONTEXT_PUBLISH: &str = "/Context/Publish";
    /// Bare context-source route (not registered) and prefix of `/Context/Message/{senderDcgClientId}`.
    pub const CONTEXT_MESSAGE: &str = "/Context/Message";
    pub const DEVICE_RESOURCE_MANAGER: &str = "/DeviceResourceManager";
    /// App-typed: taken by the device-proxy receiver by its `_route` header.
    pub const DEVICE_PROXY: &str = "/DeviceProxyClient/TransportMiddleware";
}

/// Fragment-queue priorities a platform message is sent with.
pub mod priority {
    pub const HIGH: u32 = 0;
    pub const MEDIUM: u32 = 50;
    pub const LOW: u32 = 100;
    /// Protobuf-bodied platform messages default to `100`.
    pub const PROTOBUF_DEFAULT: u32 = LOW;
    /// KVP-bodied platform messages are always `0`.
    pub const KVP: u32 = HIGH;
    /// `/SessionValidation` and `/PlatformDisconnectRequest`, protobuf-bodied, go at `0`.
    pub const SESSION_CONTROL: u32 = HIGH;
    /// Context-source messages pass `0`.
    pub const CONTEXT_SOURCE: u32 = HIGH;
    /// Device-proxy replies are `0`.
    pub const DEVICE_PROXY_REPLY: u32 = HIGH;
}
