//! Remote control for the UI ([`crate::features::remote_control`]): the selected phone's toggle and
//! whether it applies.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::FeatureError;
use crate::features::remote_control::{RemoteControl, RemoteControlEvent, RemoteControlPhone};

use super::{FeatureApiError, core_of, features_of, selected_phone};

/// What the remote-control setting shows for the selected phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RemoteControlPhoneState {
    /// The per-phone toggle (default on).
    pub toggle: bool,
    /// The toggle applies: the phone supports remote control and remote control is on on this Mac.
    pub applicable: bool,
    /// The phone supports remote control (lock at least).
    pub capable: bool,
    /// The phone can also sleep, shut down and restart this Mac.
    pub v2_capable: bool,
}

impl From<&RemoteControlPhone> for RemoteControlPhoneState {
    fn from(p: &RemoteControlPhone) -> Self {
        Self { toggle: p.toggle, applicable: p.applicable, capable: p.capable, v2_capable: p.v2_capable }
    }
}

/// A change to remote control: re-read [`RemoteControlApi::phone_state`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RemoteControlChange {
    /// What `phone_state` reports may have changed.
    PhoneChanged,
}

impl From<&RemoteControlEvent> for RemoteControlChange {
    fn from(e: &RemoteControlEvent) -> Self {
        match e {
            RemoteControlEvent::PhoneChanged { .. } => RemoteControlChange::PhoneChanged,
        }
    }
}

/// The remote-control feature.
#[derive(uniffi::Object)]
pub struct RemoteControlApi {
    engine: Arc<Engine>,
}

impl RemoteControlApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&RemoteControl, &crate::types::DcgClient) -> Result<T, FeatureError>,
    ) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.remote_control.clone();
        let phone = selected_phone(&core)?;
        Ok(f(&feature, &phone)?)
    }
}

#[uniffi::export]
impl RemoteControlApi {
    /// The selected phone's remote-control state; `None` while the phone is not in the account's list.
    pub fn phone_state(&self) -> Result<Option<RemoteControlPhoneState>, FeatureApiError> {
        self.run(|f, phone| Ok(f.phone(phone)?)).map(|p| p.as_ref().map(Into::into))
    }

    /// The selected phone's toggle: whether it may lock, sleep, shut down or restart this Mac.
    pub fn set_phone_toggle(&self, enabled: bool) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.set_phone_toggle(phone, enabled))
    }
}
