//! Instant hotspot for the UI ([`crate::features::hotspot`]).
//!
//! It covers whether the selected phone offers it and whether this Mac is on it, the connect that
//! joins the phone's hotspot, pairing with the phone and asking for its consent first when either is
//! missing, and the disconnect that leaves it.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::hotspot::{Hotspot, HotspotFailure, HotspotView, Unavailable};

use super::bluetooth::BluetoothError;
use super::{FeatureApiError, core_of, features_of, selected_phone};

/// What the hotspot item shows for the selected phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct HotspotPhoneState {
    /// The phone can share its connection with this Mac: offer connect.
    pub supported: bool,
    /// A connect or disconnect is running.
    pub busy: bool,
    /// This Mac's Wi-Fi is on the network the last connect joined: offer disconnect.
    pub joined: bool,
}

impl From<HotspotView> for HotspotPhoneState {
    fn from(v: HotspotView) -> Self {
        Self { supported: v.supported, busy: v.busy, joined: v.joined }
    }
}

/// Why the phone cannot share its connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HotspotUnavailable {
    AirplaneMode,
    BlockedByPolicy,
    DataPlan,
    DataSaver,
    PasswordNotSet,
    SimNotReady,
    SmartViewInUse,
    WifiDirectInUse,
    WirelessDexInUse,
    LinkToWindowsOff,
    BluetoothPermission,
    Other,
}

impl From<Unavailable> for HotspotUnavailable {
    fn from(u: Unavailable) -> Self {
        match u {
            Unavailable::AirplaneMode => Self::AirplaneMode,
            Unavailable::BlockedByPolicy => Self::BlockedByPolicy,
            Unavailable::DataPlan => Self::DataPlan,
            Unavailable::DataSaver => Self::DataSaver,
            Unavailable::PasswordNotSet => Self::PasswordNotSet,
            Unavailable::SimNotReady => Self::SimNotReady,
            Unavailable::SmartViewInUse => Self::SmartViewInUse,
            Unavailable::WifiDirectInUse => Self::WifiDirectInUse,
            Unavailable::WirelessDexInUse => Self::WirelessDexInUse,
            Unavailable::LinkToWindowsOff => Self::LinkToWindowsOff,
            Unavailable::BluetoothPermission => Self::BluetoothPermission,
            Unavailable::Other => Self::Other,
        }
    }
}

/// Why a connect ended without joining, or a disconnect without leaving.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum HotspotError {
    #[error("{error}")]
    Feature { error: FeatureApiError },
    #[error("a connect is already running")]
    Busy,
    #[error("the phone does not support instant hotspot")]
    NotSupported,
    /// The phone's Bluetooth device is not at hand: Bluetooth is off, or pairing did not finish.
    #[error("{error}")]
    Bluetooth { error: BluetoothError },
    #[error("the phone did not answer the request")]
    ConsentTimedOut,
    #[error("the phone does not trust this Mac: {reason}")]
    NotTrusted { reason: String },
    #[error("the phone could not be reached: {reason}")]
    Unreachable { reason: String },
    /// The phone offers no side channel: instant hotspot is off on the phone.
    #[error("the phone's side channel is not open")]
    SideChannelNotOpen,
    #[error("the phone did not answer")]
    NoAnswer,
    #[error("the phone answered with an error: {reason}")]
    PhoneError { reason: String },
    #[error("the phone cannot share its connection")]
    Unavailable { reason: HotspotUnavailable },
    #[error("the phone sent no credentials")]
    NoCredentials,
    #[error("the credentials are invalid: {reason}")]
    InvalidCredentials { reason: String },
    /// The credentials arrived but the network did not show up: the user can join by hand.
    #[error("the hotspot is not visible")]
    NotVisible { ssid: String, passphrase: String },
    /// The credentials arrived but joining failed: the user can join by hand.
    #[error("joining the hotspot failed: {reason}")]
    JoinFailed { ssid: String, passphrase: String, reason: String },
    /// Leaving the hotspot's network failed.
    #[error("leaving the hotspot failed: {reason}")]
    LeaveFailed { reason: String },
    #[error("internal: {message}")]
    Internal { message: String },
}

impl From<FeatureApiError> for HotspotError {
    fn from(error: FeatureApiError) -> Self {
        HotspotError::Feature { error }
    }
}

impl From<HotspotFailure> for HotspotError {
    fn from(f: HotspotFailure) -> Self {
        match f {
            HotspotFailure::Busy => Self::Busy,
            HotspotFailure::NotSupported => Self::NotSupported,
            HotspotFailure::Bluetooth(f) => Self::Bluetooth { error: f.into() },
            HotspotFailure::ConsentTimedOut => Self::ConsentTimedOut,
            HotspotFailure::NotTrusted(reason) => Self::NotTrusted { reason },
            HotspotFailure::Unreachable(reason) => Self::Unreachable { reason },
            HotspotFailure::SideChannelNotOpen => Self::SideChannelNotOpen,
            HotspotFailure::NoAnswer => Self::NoAnswer,
            HotspotFailure::PhoneError(reason) => Self::PhoneError { reason },
            HotspotFailure::Unavailable(reason) => Self::Unavailable { reason: reason.into() },
            HotspotFailure::NoCredentials => Self::NoCredentials,
            HotspotFailure::InvalidCredentials(reason) => Self::InvalidCredentials { reason },
            HotspotFailure::NotVisible(c) => Self::NotVisible { ssid: c.ssid, passphrase: c.passphrase },
            HotspotFailure::JoinFailed { credentials, reason } => {
                Self::JoinFailed { ssid: credentials.ssid, passphrase: credentials.passphrase, reason }
            }
            HotspotFailure::LeaveFailed(reason) => Self::LeaveFailed { reason },
            HotspotFailure::Internal(message) => Self::Internal { message },
        }
    }
}

/// The instant hotspot feature.
#[derive(uniffi::Object)]
pub struct HotspotApi {
    engine: Arc<Engine>,
}

impl HotspotApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    /// Runs `f` with the feature and the selected phone on the engine's runtime.
    async fn spawn<T: Send + 'static, Fut>(
        &self,
        f: impl FnOnce(Arc<Hotspot>, crate::types::DcgClient) -> Fut,
    ) -> Result<T, HotspotError>
    where
        Fut: Future<Output = Result<T, HotspotFailure>> + Send + 'static,
    {
        let core = core_of(&self.engine)?;
        let feature = features_of(&core)?.hotspot.clone();
        let phone = selected_phone(&core)?;

        let task = core.runtime_handle().spawn(f(feature, phone));
        match task.await {
            Ok(result) => Ok(result?),
            Err(e) => Err(HotspotError::Internal { message: e.to_string() }),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl HotspotApi {
    /// The selected phone's hotspot state; `None` while the phone record is not loaded.
    pub fn phone_state(&self) -> Result<Option<HotspotPhoneState>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.hotspot.clone();
        let phone = selected_phone(&core)?;
        Ok(feature.view(&phone).map(Into::into))
    }

    /// Asks the phone to turn its hotspot on and joins it; returns the network's name. Pairs with
    /// the phone over Bluetooth first when no paired device is the phone, and asks for the phone's
    /// consent and Bluetooth permissions when they are missing.
    pub async fn connect(&self) -> Result<String, HotspotError> {
        self.spawn(|f, phone| async move { f.connect(&phone).await }).await
    }

    /// Leaves the phone's hotspot while this Mac's Wi-Fi is on it; does nothing otherwise.
    pub async fn disconnect(&self) -> Result<(), HotspotError> {
        self.spawn(|f, phone| async move { f.disconnect(&phone).await }).await
    }
}
