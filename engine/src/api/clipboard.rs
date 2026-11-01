//! The clipboard feature for the UI ([`crate::features::clipboard`]): the global switch and the
//! selected phone's toggle and state.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::FeatureError;
use crate::features::clipboard::{ClipKind, Clipboard, ClipboardEvent, ClipboardPhone};

use super::{FeatureApiError, core_of, features_of, selected_phone};

/// What the clipboard settings show for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardPhoneState {
    /// The per-phone toggle (default off).
    pub toggle: bool,
    /// The toggle applies: the phone supports clipboard sharing and the global switch is on.
    pub applicable: bool,
    /// The phone supports clipboard sharing.
    pub capable: bool,
    /// The phone is connected.
    pub available: bool,
    /// Clipboard sharing runs with this phone now.
    pub supported: bool,
}

impl From<&ClipboardPhone> for ClipboardPhoneState {
    fn from(p: &ClipboardPhone) -> Self {
        Self {
            toggle: p.toggle,
            applicable: p.applicable,
            capable: p.capable,
            available: p.available,
            supported: p.supported,
        }
    }
}

/// What a clip carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClipContent {
    Image,
    Html,
    Text,
}

impl From<ClipKind> for ClipContent {
    fn from(k: ClipKind) -> Self {
        match k {
            ClipKind::Image => ClipContent::Image,
            ClipKind::Html => ClipContent::Html,
            ClipKind::Text => ClipContent::Text,
        }
    }
}

/// A change to clipboard sharing, or a clip that moved between this Mac and the phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ClipboardChange {
    GlobalSwitchChanged {
        enabled: bool,
    },
    /// What `phone_state` reports may have changed.
    PhoneChanged,
    /// The phone's clip was written to the pasteboard.
    Received {
        content: ClipContent,
    },
    /// This Mac's clipboard was announced to the phone.
    Announced {
        content: ClipContent,
    },
}

impl From<&ClipboardEvent> for ClipboardChange {
    fn from(e: &ClipboardEvent) -> Self {
        match e {
            ClipboardEvent::GlobalSwitchChanged { enabled } => {
                ClipboardChange::GlobalSwitchChanged { enabled: *enabled }
            }
            ClipboardEvent::PhoneChanged { .. } => ClipboardChange::PhoneChanged,
            ClipboardEvent::Received { kind, .. } => ClipboardChange::Received { content: (*kind).into() },
            ClipboardEvent::Announced { kind } => ClipboardChange::Announced { content: (*kind).into() },
        }
    }
}

/// The clipboard feature.
#[derive(uniffi::Object)]
pub struct ClipboardApi {
    engine: Arc<Engine>,
}

impl ClipboardApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(
        &self,
        f: impl FnOnce(&Clipboard, &crate::types::DcgClient) -> Result<T, FeatureError>,
    ) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.clipboard.clone();
        let phone = selected_phone(&core)?;
        Ok(f(&feature, &phone)?)
    }
}

#[uniffi::export]
impl ClipboardApi {
    /// The global switch (default on); it decides whether this Mac offers clipboard sharing at all.
    pub fn global_switch(&self) -> Result<bool, FeatureApiError> {
        self.run(|f, _| f.global_switch())
    }

    /// Turns clipboard sharing for this Mac on or off; a change is saved and told to every phone.
    pub fn set_global_switch(&self, enabled: bool) -> Result<(), FeatureApiError> {
        self.run(|f, _| f.set_global_switch(enabled))
    }

    /// The selected phone's clipboard state; `None` while the phone is not in the account's list.
    pub fn phone_state(&self) -> Result<Option<ClipboardPhoneState>, FeatureApiError> {
        self.run(|f, phone| f.phone(phone)).map(|p| p.as_ref().map(Into::into))
    }

    /// The selected phone's toggle.
    pub fn set_phone_toggle(&self, enabled: bool) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.set_phone_toggle(phone, enabled))
    }
}
