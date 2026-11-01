//! The connected camera for the UI ([`crate::features::connected_camera`]).
//!
//! It covers the selected phone's camera state, its setting, the control window's actions, and the
//! signals of the Mac's camera — its clients starting and stopping, and the camera becoming
//! available.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::connected_camera::{
    CameraError, CameraFace, CameraState, CameraStatus, ConnectedCamera, ConnectedCameraError, ConnectedCameraEvent,
    Effect, EffectLabel, ErrorAction, OemLabel,
};

use super::{FeatureApiError, core_of, count, features_of, selected_phone};

impl From<ConnectedCameraError> for FeatureApiError {
    fn from(e: ConnectedCameraError) -> Self {
        match e {
            ConnectedCameraError::Store(e) => e.into(),
            other => FeatureApiError::NotAvailable { reason: other.to_string() },
        }
    }
}

/// Where the phone's camera stands: offered to this Mac, connecting, streaming, paused, or failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCameraState {
    Unregistered,
    Registered,
    Connecting,
    Streaming,
    Paused,
    Error,
    TerminalError,
    /// The phone asks its user to tap its notification to start.
    StartStreamNotification,
}

impl From<CameraState> for PhoneCameraState {
    fn from(s: CameraState) -> Self {
        match s {
            CameraState::Unregistered => Self::Unregistered,
            CameraState::Registered => Self::Registered,
            CameraState::Connecting => Self::Connecting,
            CameraState::Streaming => Self::Streaming,
            CameraState::Paused => Self::Paused,
            CameraState::Error => Self::Error,
            CameraState::TerminalError => Self::TerminalError,
            CameraState::StartStreamNotification => Self::StartStreamNotification,
        }
    }
}

/// The camera's error, by its wire value (`PhoneCameraStatus::error_code`), named for the UI's texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCameraError {
    None,
    CameraDeviceFatal,
    CameraDisabled,
    CameraInUse,
    CameraServiceFatal,
    MaxCamerasInUse,
    Disconnected,
    NoCameraAvailable,
    ScreenLocked,
    Unknown,
    CameraPermissionNeeded,
    LtwDisabled,
    CallInProgress,
    MeteredConnection,
    DeviceHeat,
    ChannelNotCreated,
    CameraPermissionDenied,
    PostNotificationPermissionNeeded,
    ChannelDisconnected,
    SessionConfigurationFailed,
    CameraPermissionPermanentlyDenied,
    CameraPermissionNeededAutoprompt,
    CameraPermissionRevoked,
    /// A value the Mac does not name.
    Other,
}

impl From<CameraError> for PhoneCameraError {
    fn from(e: CameraError) -> Self {
        match e {
            CameraError::None => Self::None,
            CameraError::CameraDeviceFatal => Self::CameraDeviceFatal,
            CameraError::CameraDisabled => Self::CameraDisabled,
            CameraError::CameraInUse => Self::CameraInUse,
            CameraError::CameraServiceFatal => Self::CameraServiceFatal,
            CameraError::MaxCamerasInUse => Self::MaxCamerasInUse,
            CameraError::Disconnected => Self::Disconnected,
            CameraError::NoCameraAvailable => Self::NoCameraAvailable,
            CameraError::ScreenLocked => Self::ScreenLocked,
            CameraError::Unknown => Self::Unknown,
            CameraError::CameraPermissionNeeded => Self::CameraPermissionNeeded,
            CameraError::LtwDisabled => Self::LtwDisabled,
            CameraError::CallInProgress => Self::CallInProgress,
            CameraError::MeteredConnection => Self::MeteredConnection,
            CameraError::DeviceHeat => Self::DeviceHeat,
            CameraError::ChannelNotCreated => Self::ChannelNotCreated,
            CameraError::CameraPermissionDenied => Self::CameraPermissionDenied,
            CameraError::PostNotificationPermissionNeeded => Self::PostNotificationPermissionNeeded,
            CameraError::ChannelDisconnected => Self::ChannelDisconnected,
            CameraError::SessionConfigurationFailed => Self::SessionConfigurationFailed,
            CameraError::CameraPermissionPermanentlyDenied => Self::CameraPermissionPermanentlyDenied,
            CameraError::CameraPermissionNeededAutoprompt => Self::CameraPermissionNeededAutoprompt,
            CameraError::CameraPermissionRevoked => Self::CameraPermissionRevoked,
            CameraError::Other(_) => Self::Other,
        }
    }
}

/// What the error screen offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCameraErrorAction {
    Refresh,
    RequestPermissions,
    /// Close the error (tells the phone).
    Close,
    None,
}

impl From<ErrorAction> for PhoneCameraErrorAction {
    fn from(a: ErrorAction) -> Self {
        match a {
            ErrorAction::Refresh => Self::Refresh,
            ErrorAction::RequestPermissions => Self::RequestPermissions,
            ErrorAction::Close => Self::Close,
            ErrorAction::None => Self::None,
        }
    }
}

/// Which of the phone's cameras streams: the front or the back one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCameraFace {
    Front,
    Back,
}

impl From<CameraFace> for PhoneCameraFace {
    fn from(f: CameraFace) -> Self {
        match f {
            CameraFace::Front => Self::Front,
            CameraFace::Back => Self::Back,
        }
    }
}

impl From<PhoneCameraFace> for CameraFace {
    fn from(f: PhoneCameraFace) -> Self {
        match f {
            PhoneCameraFace::Front => Self::Front,
            PhoneCameraFace::Back => Self::Back,
        }
    }
}

/// One effect of the list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneCameraEffect {
    /// "None".
    None,
    Automatic,
    Bokeh,
    FaceRetouch,
    Hdr,
    Night,
    /// An effect of the phone's maker, with its label for the UI's locale.
    Oem {
        label: OemEffectLabel,
        icon: Option<String>,
    },
}

/// An OEM effect's label.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum OemEffectLabel {
    /// The UI's string for this key.
    Known { label: EffectLabel },
    /// This text as it is.
    Text { text: String },
}

fn effect(e: &Effect, locale: &str) -> PhoneCameraEffect {
    match e {
        Effect::None | Effect::BuiltIn(0) => PhoneCameraEffect::None,
        Effect::BuiltIn(1) => PhoneCameraEffect::Automatic,
        Effect::BuiltIn(2) => PhoneCameraEffect::Bokeh,
        Effect::BuiltIn(4) => PhoneCameraEffect::FaceRetouch,
        Effect::BuiltIn(8) => PhoneCameraEffect::Hdr,
        Effect::BuiltIn(16) => PhoneCameraEffect::Night,
        Effect::BuiltIn(other) => unreachable!("the effect list holds single known flags, not {other}"),
        Effect::Oem(o) => {
            let label = match e.oem_label(locale).expect("an OEM effect has a label") {
                OemLabel::Known(label) => OemEffectLabel::Known { label },
                OemLabel::Text(text) => OemEffectLabel::Text { text },
            };
            PhoneCameraEffect::Oem { label, icon: o.icon.clone() }
        }
    }
}

/// The selected phone's camera.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PhoneCameraStatus {
    /// The Mac's camera extension is running.
    pub mac_available: bool,
    pub capable: bool,
    /// Show the setting.
    pub supported: bool,
    /// "Use as a camera".
    pub camera_enabled: bool,
    pub registered: bool,
    /// Publishing the camera failed: show the banner and the retry.
    pub registration_failed: bool,
    pub permission_required: bool,
    pub camera_permission: bool,
    pub state: PhoneCameraState,
    pub error: PhoneCameraError,
    pub error_code: i32,
    pub error_action: PhoneCameraErrorAction,
    pub face: Option<PhoneCameraFace>,
    pub effects: Vec<PhoneCameraEffect>,
    pub selected_effect: Option<u32>,
    /// `-1` before the phone reported it.
    pub battery_level: i32,
    pub is_charging: bool,
    pub low_network: bool,
    pub qr_code_uri: Option<String>,
    pub control_window: bool,
}

fn status(s: CameraStatus, locale: &str) -> PhoneCameraStatus {
    PhoneCameraStatus {
        mac_available: s.mac_available,
        capable: s.capable,
        supported: s.supported,
        camera_enabled: s.camera_enabled,
        registered: s.registered,
        registration_failed: s.registration_failed,
        permission_required: s.permission_required,
        camera_permission: s.camera_permission,
        state: s.state.into(),
        error: s.error.into(),
        error_code: s.error.wire(),
        error_action: s.error_action.into(),
        face: s.face.map(Into::into),
        effects: s.effects.iter().map(|e| effect(e, locale)).collect(),
        selected_effect: s.selected_effect.map(count),
        battery_level: s.battery_level,
        is_charging: s.is_charging,
        low_network: s.low_network,
        qr_code_uri: s.qr_code_uri,
        control_window: s.control_window,
    }
}

/// A change to the connected camera: re-read the status, or show or hide the control window.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectedCameraChange {
    /// Re-read `status`.
    Changed,
    /// Show or hide the camera control window.
    ControlWindow { shown: bool },
    /// Turning the camera on replaced the camera of the phone named `name`.
    Replaced { name: String },
}

impl From<&ConnectedCameraEvent> for ConnectedCameraChange {
    fn from(e: &ConnectedCameraEvent) -> Self {
        match e {
            ConnectedCameraEvent::Changed { .. } => Self::Changed,
            ConnectedCameraEvent::ControlWindow { shown, .. } => Self::ControlWindow { shown: *shown },
            ConnectedCameraEvent::Replaced { name, .. } => Self::Replaced { name: name.clone() },
        }
    }
}

/// The connected camera of the selected phone.
#[derive(uniffi::Object)]
pub struct ConnectedCameraApi {
    engine: Arc<Engine>,
}

impl ConnectedCameraApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<(Arc<ConnectedCamera>, crate::types::DcgClient), FeatureApiError> {
        let core = core_of(&self.engine)?;
        let phone = selected_phone(&core)?;
        Ok((features_of(&core)?.connected_camera.clone(), phone))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ConnectedCameraApi {
    /// The camera's state; OEM effect labels for `locale` (a locale name such as `en-US`).
    pub fn status(&self, locale: String) -> Result<PhoneCameraStatus, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(status(feature.status(&phone)?, &locale))
    }

    /// "Use as a camera".
    pub fn set_camera_enabled(&self, on: bool) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.set_camera_enabled(&phone, on)?)
    }

    /// The banner's retry after publishing the camera failed.
    pub fn retry_registration(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.retry_registration(&phone)?)
    }

    /// A client of camera `device_id` started (`true`) or stopped its stream.
    pub fn client_changed(&self, device_id: String, started: bool) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        if !phone.id.eq_ignore_ascii_case(&device_id) {
            return Err(FeatureApiError::UnknownItem { what: "camera".to_owned() });
        }
        feature.client_changed(&phone, started);
        Ok(())
    }

    /// The Mac's camera extension became available or unavailable.
    pub fn availability_changed(&self) -> Result<(), FeatureApiError> {
        let (feature, _) = self.feature()?;
        feature.availability_changed();
        Ok(())
    }

    /// Pauses the phone's video; the status shows `Paused` at once.
    ///
    /// Fails with `NotAvailable` when no stream is open to the phone.
    pub fn pause(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.pause(&phone)?)
    }

    /// Resumes the paused video; the status shows `Streaming` at once.
    ///
    /// Fails with `NotAvailable` when no stream is open to the phone.
    pub fn resume(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.resume(&phone)?)
    }

    /// Asks the phone to stream from camera `face`; the status changes when the phone confirms.
    ///
    /// Fails with `NotAvailable` when no stream is open to the phone.
    pub fn switch_camera(&self, face: PhoneCameraFace) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.switch_camera(&phone, face.into())?)
    }

    /// Applies effect `index` of the status's `effects` to the video.
    ///
    /// Fails with `NotAvailable` when `index` is not in the list or no stream is open to the phone.
    pub fn select_effect(&self, index: u32) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.select_effect(&phone, index as usize)?)
    }

    /// The fatal-error screen's close.
    pub fn close_fatal_error(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.close_fatal_error(&phone)?)
    }

    /// The user closed the camera control window: the stream closes.
    pub fn close_control_window(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        feature.close_control_window(&phone);
        Ok(())
    }

    /// The error screen's refresh.
    pub fn refresh(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.refresh(&phone)?)
    }

    /// The permission error screen's retry.
    pub fn request_permissions(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.request_permissions(&phone)?)
    }

    /// The setting's "grant permission": `true` when the phone holds the camera permission after it.
    pub async fn settings_request_permission(&self) -> Result<bool, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.settings_request_permission(&phone).await?)
    }
}
