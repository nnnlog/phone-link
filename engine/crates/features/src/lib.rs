//! What the Phone Link engine's features share: the environment a feature is installed with
//! ([`FeatureEnv`]), the feature errors, the traits one feature implements for another
//! ([`ContactDirectory`], [`NewPhotoGate`], [`PhoneAppDirectory`], [`MessagingAlerts`]), and
//! [`shared`]: the per-phone stores, the system-notification edge, the account's phones, the
//! per-phone switch and the other pieces several features use.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod shared;

use std::sync::Arc;

use phonelink_app::{AppError, AppLayer};
use phonelink_core::config::EngineConfig;
use phonelink_core::edge::{EdgeError, Edges};
use phonelink_core::seams::PartnerSessionControl;
use phonelink_core::tap::Tap;
use phonelink_core::types::DcgClient;

#[cfg(feature = "test-support")]
pub use shared::db::PhoneStores;
pub use shared::db::{FeatureDb, StoreError, StoreLocation, delete_phone_stores};
#[cfg(feature = "test-support")]
pub use shared::notifier::{NotificationAction, NotificationResponder, TextInput};
pub use shared::notifier::{NotificationResponse, NotificationRouter, SystemNotification, SystemNotifier};
pub use shared::peers::PeerDirectory;
pub use shared::phone_switch::{PhoneSwitchChanged, PhoneSwitches};

/// What every feature receives when it is installed.
#[derive(Clone)]
pub struct FeatureEnv {
    pub config: Arc<EngineConfig>,
    /// The engine's OS edges; `edges.clock` is the wall clock for times the protocol states in UTC
    /// (timers use `tokio::time`, which tests pause).
    pub edges: Edges,
    /// Requests, content processors, connect providers, request observers, the phone record.
    pub app: Arc<AppLayer>,
    /// The phone's platform connection: *available* = connected; its connect/disconnect events.
    pub sessions: Arc<dyn PartnerSessionControl>,
    pub peers: Arc<dyn PeerDirectory>,
    /// The per-phone "enabled" switch of the CDEH features (the DRM wrapper's `phone_enabled`).
    pub switches: Arc<PhoneSwitches>,
    pub notifier: Arc<dyn SystemNotifier>,
    /// Where responses to posted notifications are routed; a feature that posts registers here.
    pub responses: Arc<NotificationRouter>,
    pub stores: StoreLocation,
    pub tap: Tap,
}

/// A feature could not be installed (or an operation broke a local invariant).
#[derive(Debug, thiserror::Error)]
pub enum FeatureError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("App layer: {0}")]
    App(#[from] AppError),
    #[error("edge: {0}")]
    Edge(#[from] EdgeError),
    /// A registration on an extension point was refused (e.g. a second receiver for a path).
    #[error("registration: {0}")]
    Registration(#[from] RegistrationError),
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("programming error: {0}")]
    Programming(String),
}

/// Why a registration on an extension point was refused.
#[derive(Debug, thiserror::Error)]
pub enum RegistrationError {
    #[error("{0}")]
    Responder(#[from] shared::notifier::ResponderTaken),
    #[error("{0}")]
    Receiver(#[from] phonelink_transport::platform::drm::DrmRegistrationError),
    /// A receiver whose refusal names its path.
    #[error("{path}: {source}")]
    ReceiverAt {
        path: &'static str,
        #[source]
        source: phonelink_transport::platform::drm::DrmRegistrationError,
    },
}

/// A contact an address resolved to by the local lookup by address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactMatch {
    /// The contact that names the address — the first of its contacts with a non-empty display name,
    /// else the first of them.
    pub contact_id: i64,
    /// That contact's display name; `None` when none of the address's contacts has a non-empty one:
    /// an empty name is no name, and the address is shown instead.
    pub display_name: Option<String>,
    /// The contact whose photo shows for the address — the first of the address's contacts with a
    /// stored thumbnail, which need not be the one that names it — when any.
    pub photo_contact_id: Option<i64>,
}

/// What other features read from contacts.
///
/// The local lookup by address, whether contacts may be synced, and the `/contacts/v2` sync request
/// that messaging's permission flow and the call-history refresh send.
pub trait ContactDirectory: Send + Sync {
    /// The contact `address` resolves to on `phone`, by the engine's single lookup rule; `region` is
    /// the country of the message's SIM subscription when the caller has one.
    fn lookup(&self, phone: &DcgClient, address: &str, region: Option<&str>) -> Option<ContactMatch>;
    /// The contacts PC consent and the contacts device permission of `phone` are both on.
    fn sync_allowed(&self, phone: &DcgClient) -> bool;
    /// Starts the `/contacts/v2` sync request with `isuserinitiated = user_initiated` without waiting
    /// for it; its outcome is applied by contacts.
    fn request_sync(&self, user_initiated: bool);
}

/// One app of the phone's stored app list, as other features show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneAppInfo {
    /// The shown name.
    pub name: String,
    /// The stored icon bytes (the phone sends PNG); `None` when it sent none.
    pub icon: Option<Vec<u8>>,
}

/// Whether another feature's new-photo notifications cover a phone; photos then posts none for it.
pub trait NewPhotoGate: Send + Sync {
    /// `true`: `phone`'s new photos are notified elsewhere.
    fn covered(&self, phone: &DcgClient) -> bool;
}

/// What other features read from the phone apps: the name and icon of a package's app.
pub trait PhoneAppDirectory: Send + Sync {
    /// `package`'s app on `phone` (exact package name); `None` when none is stored.
    fn app(&self, phone: &DcgClient, package: &str) -> Option<PhoneAppInfo>;
}

/// What notifications and messaging exchange about message alerts.
///
/// Whether a group-message notification (`isReadOnlyGroupMessage`) may alert — the Messages feature's
/// PC permission and its message-notification setting — and the authentication codes the phone's
/// messages app offers.
pub trait MessagingAlerts: Send + Sync {
    fn group_message_alerts_allowed(&self, phone: &DcgClient) -> bool;
    /// The phone's messages app offered `code` in its own copy action; the new-message notification of
    /// the same message gets Copy for it.
    fn copy_code_offered(&self, phone: &DcgClient, code: &str);
}
