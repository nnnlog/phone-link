//! New-photo notifications for the UI ([`crate::features::remote_snipping`]): the selected phone's
//! settings, the "Allow permissions" link, and the image a clicked notification opens.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::remote_snipping::{
    PermissionOutcome, PhoneSettings, RemoteSnipping, RemoteSnippingError, RemoteSnippingEvent,
};

use super::{FeatureApiError, core_of, features_of, selected_phone};

impl From<RemoteSnippingError> for FeatureApiError {
    fn from(e: RemoteSnippingError) -> Self {
        match e {
            RemoteSnippingError::UnknownPhone => FeatureApiError::NotAvailable { reason: e.to_string() },
            RemoteSnippingError::Store(e) => e.into(),
        }
    }
}

/// The selected phone's new-photo notification settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RemoteSnippingSettings {
    /// The phone supports new-photo notifications: show "Get new photo notifications".
    pub applicable: bool,
    /// "Get new photo notifications".
    pub enabled: bool,
    /// Show the "Allow permissions" link.
    pub needs_permission: bool,
    /// The permission request runs: the link reads "Finishing up".
    pub requesting_permission: bool,
    /// These notifications stand in for the photos feature's own: its "Notify me about new photos"
    /// does nothing for this phone.
    pub covers_photos_notifications: bool,
}

impl From<PhoneSettings> for RemoteSnippingSettings {
    fn from(s: PhoneSettings) -> Self {
        Self {
            applicable: s.applicable,
            enabled: s.enabled,
            needs_permission: s.needs_permission,
            requesting_permission: s.requesting_permission,
            covers_photos_notifications: s.covers_new_photo_notifications,
        }
    }
}

/// How the permission request ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteSnippingPermission {
    Granted,
    /// The phone shows its prompt.
    RequestSent,
    Failed {
        reason: String,
    },
}

impl From<PermissionOutcome> for RemoteSnippingPermission {
    fn from(o: PermissionOutcome) -> Self {
        match o {
            PermissionOutcome::Granted => RemoteSnippingPermission::Granted,
            PermissionOutcome::RequestSent => RemoteSnippingPermission::RequestSent,
            PermissionOutcome::Failed(reason) => RemoteSnippingPermission::Failed { reason },
        }
    }
}

/// A change to new-photo notifications: re-read the settings, or open a photo the user clicked.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteSnippingChange {
    /// Re-read `settings`.
    SettingsChanged,
    /// The user clicked a new-photo notification: open `bytes` (the photo, else its thumbnail) for
    /// viewing and editing.
    OpenImage { media_id: i64, bytes: Vec<u8> },
}

impl From<&RemoteSnippingEvent> for RemoteSnippingChange {
    fn from(e: &RemoteSnippingEvent) -> Self {
        match e {
            RemoteSnippingEvent::Changed { .. } => RemoteSnippingChange::SettingsChanged,
            RemoteSnippingEvent::OpenImage { media_id, bytes, .. } => {
                RemoteSnippingChange::OpenImage { media_id: *media_id, bytes: bytes.clone() }
            }
        }
    }
}

/// New-photo notifications of the selected phone.
#[derive(uniffi::Object)]
pub struct RemoteSnippingApi {
    engine: Arc<Engine>,
}

impl RemoteSnippingApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<(Arc<RemoteSnipping>, crate::types::DcgClient), FeatureApiError> {
        let core = core_of(&self.engine)?;
        let phone = selected_phone(&core)?;
        Ok((features_of(&core)?.remote_snipping.clone(), phone))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl RemoteSnippingApi {
    /// The selected phone's new-photo notification settings and permission state.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn settings(&self) -> Result<RemoteSnippingSettings, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.settings(&phone)?.into())
    }

    /// "Get new photo notifications".
    pub fn set_enabled(&self, on: bool) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.set_enabled(&phone, on)?)
    }

    /// The "Allow permissions" link: asks the phone for photo access.
    pub async fn request_permission(&self) -> Result<RemoteSnippingPermission, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.request_permission(&phone).await?.into())
    }
}
