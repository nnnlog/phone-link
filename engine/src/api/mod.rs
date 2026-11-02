//! The `uniffi` surface `gui/` drives the features through.
//!
//! It covers one object per feature, reached from [`Engine`], each resolving the engine's current
//! core on every call — `Engine::start` replaces a core that needs a rebuild, so a handle kept
//! across calls would go stale — and records and enums that mirror the features' views in an
//! FFI-friendly shape (no `usize`, tuples or `&'static str`; the selected phone implied, since the
//! UI shows one phone).
//!
//! The typed events
//! [`events::EngineEvent`] carries are built here too.
//!
//! The features' own types stay free of `uniffi`: a mirror can drop what the UI does not read and
//! rename what would clash in one Swift module (the two features' `SyncState`).

pub mod audio;
pub mod bluetooth;
pub mod calling;
pub mod clipboard;
pub mod connected_camera;
pub mod contacts;
pub mod device_status;
pub mod engine;
pub mod events;
pub mod file_transfer;
pub mod hotspot;
pub mod logging;
pub mod messaging;
pub mod notifications;
pub mod pairing;
pub mod phone;
pub mod phone_apps;
pub mod phone_files;
pub mod photos;
pub mod remote_control;
pub mod remote_snipping;
pub mod screen_mirroring;
pub mod wallpaper;

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::app::{AppError, AppOutcome};
use crate::engine::EngineCore;
use crate::features::{FeatureError, Features, StoreError};
use crate::types::DcgClient;

/// Why a feature operation could not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum FeatureApiError {
    /// The engine has not started (or its core was dropped for a rebuild): call `Engine::start`.
    #[error("the engine is not started")]
    NotStarted,
    #[error("no phone is selected")]
    NoPhone,
    /// The item the call names is not stored (a notification, conversation, send, part, photo).
    #[error("unknown {what}")]
    UnknownItem { what: String },
    /// The operation does not apply now (not offered by the phone, not enabled, not cached).
    #[error("not available: {reason}")]
    NotAvailable { reason: String },
    /// The arguments break a rule of the feature (nothing to send, too many attachments, …).
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    #[error("store: {message}")]
    Store { message: String },
    #[error("internal: {message}")]
    Internal { message: String },
}

impl From<StoreError> for FeatureApiError {
    fn from(e: StoreError) -> Self {
        FeatureApiError::Store { message: e.to_string() }
    }
}

impl From<AppError> for FeatureApiError {
    fn from(e: AppError) -> Self {
        FeatureApiError::Internal { message: e.to_string() }
    }
}

impl From<FeatureError> for FeatureApiError {
    fn from(e: FeatureError) -> Self {
        match e {
            FeatureError::Store(e) => e.into(),
            other => FeatureApiError::Internal { message: other.to_string() },
        }
    }
}

/// The outcome of a request to the phone, as the features report it (`AppOutcome`).
///
/// The transport causes of a failed connection or send are kept as text, except the two a feature
/// sees as a `result` of the app response.
///
/// A connection or send that failed with *LTW disabled* (a wake that failed with LTW disabled, or a
/// session the phone ended with reason `ALL_SYNC_DISABLED`) is
/// [`RequestOutcome::AllFeaturesDisabled`], as `result` 13 is, and a session the phone ended with
/// reason `METERED_SYNC_DISABLED` is [`RequestOutcome::MeteredSyncDisabled`], as `result` 12 is.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RequestOutcome {
    Success,
    Failure,
    PhoneContractIncompatible,
    PcContractIncompatible,
    Accepted,
    Denied,
    PermanentlyDenied,
    NeedsPermission,
    NeedsInitialPermission,
    MeteredConnection,
    MeteredSyncDisabled,
    AllFeaturesDisabled,
    InvalidRequest,
    DatabaseFailure,
    DeviceNotSupported,
    AppLaunched,
    TwoPhoneMode,
    NonDefaultProfile,
    Unknown,
    TimedOut,
    InternetError,
    Canceled,
    /// The connection to the phone failed while the request waited for it.
    ConnectionFailed {
        reason: String,
    },
    /// The request could not be delivered.
    SendFailed {
        reason: String,
    },
    /// The phone did not serve the resource the request asked for.
    ResourceFailed {
        reason: String,
    },
}

impl From<&AppOutcome> for RequestOutcome {
    fn from(o: &AppOutcome) -> Self {
        match o {
            AppOutcome::ConnectionFailed(_) | AppOutcome::SendFailed(_) | AppOutcome::ResourceFailed(_)
                if o.is_ltw_disabled() =>
            {
                RequestOutcome::AllFeaturesDisabled
            }
            AppOutcome::ConnectionFailed(_) | AppOutcome::ResourceFailed(_) if o.is_metered_sync_disabled() => {
                RequestOutcome::MeteredSyncDisabled
            }
            AppOutcome::Success => RequestOutcome::Success,
            AppOutcome::Failure => RequestOutcome::Failure,
            AppOutcome::PhoneContractIncompatible => RequestOutcome::PhoneContractIncompatible,
            AppOutcome::PcContractIncompatible => RequestOutcome::PcContractIncompatible,
            AppOutcome::Accepted => RequestOutcome::Accepted,
            AppOutcome::Denied => RequestOutcome::Denied,
            AppOutcome::PermanentlyDenied => RequestOutcome::PermanentlyDenied,
            AppOutcome::NeedsPermission => RequestOutcome::NeedsPermission,
            AppOutcome::NeedsInitialPermission => RequestOutcome::NeedsInitialPermission,
            AppOutcome::MeteredConnection => RequestOutcome::MeteredConnection,
            AppOutcome::MeteredSyncDisabled => RequestOutcome::MeteredSyncDisabled,
            AppOutcome::AllFeaturesDisabled => RequestOutcome::AllFeaturesDisabled,
            AppOutcome::InvalidRequest => RequestOutcome::InvalidRequest,
            AppOutcome::DatabaseFailure => RequestOutcome::DatabaseFailure,
            AppOutcome::DeviceNotSupported => RequestOutcome::DeviceNotSupported,
            AppOutcome::AppLaunched => RequestOutcome::AppLaunched,
            AppOutcome::TwoPhoneMode => RequestOutcome::TwoPhoneMode,
            AppOutcome::NonDefaultProfile => RequestOutcome::NonDefaultProfile,
            AppOutcome::Unknown => RequestOutcome::Unknown,
            AppOutcome::TimedOut => RequestOutcome::TimedOut,
            AppOutcome::InternetError => RequestOutcome::InternetError,
            AppOutcome::Canceled => RequestOutcome::Canceled,
            AppOutcome::ConnectionFailed(f) => RequestOutcome::ConnectionFailed { reason: format!("{f:?}") },
            AppOutcome::SendFailed(s) => RequestOutcome::SendFailed { reason: s.to_string() },
            AppOutcome::ResourceFailed(r) => RequestOutcome::ResourceFailed { reason: format!("{r:?}") },
        }
    }
}

impl From<AppOutcome> for RequestOutcome {
    fn from(o: AppOutcome) -> Self {
        (&o).into()
    }
}

/// A feature whose event stream lost events ([`crate::edge::EngineEvent::Resync`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FeatureKind {
    Audio,
    Notifications,
    Clipboard,
    DeviceStatus,
    Messaging,
    Contacts,
    Calling,
    Photos,
    Wallpaper,
    RemoteSnipping,
    FileTransfer,
    PhoneFiles,
    ConnectedCamera,
    PhoneApps,
    ScreenMirroring,
}

/// The engine's current core, or `NotStarted`.
pub(crate) fn core_of(engine: &Engine) -> Result<Arc<EngineCore>, FeatureApiError> {
    engine.current_core().ok_or(FeatureApiError::NotStarted)
}

/// The installed features of `core` (installed by its first start).
pub(crate) fn features_of(core: &EngineCore) -> Result<Arc<Features>, FeatureApiError> {
    core.features().ok_or(FeatureApiError::NotStarted)
}

/// The phone the UI shows: the one the start selected.
pub(crate) fn selected_phone(core: &EngineCore) -> Result<DcgClient, FeatureApiError> {
    core.phone().ok_or(FeatureApiError::NoPhone)
}

/// A `usize` count or position as the FFI carries it.
pub(crate) fn count(n: usize) -> u32 {
    u32::try_from(n).expect("a count the UI reads fits in u32")
}

#[cfg(test)]
mod tests;
