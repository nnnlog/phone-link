//! What the core reports to the layer above it: the phone session and record, the account, the
//! phone's commands, connects and pairing steps ([`CoreEvent`]), and the moment its features are
//! installed, so that layer can subscribe to them before any of them runs.

use crate::app::AppOutcome;
use crate::auth::pairing::PairingStep;
use crate::features::Features;

/// A change the core reports.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    /// The platform session with the selected phone connected, or ended (`failure` says why).
    PhoneSession { connected: bool, failure: Option<String> },
    /// The selected phone left the account's linked devices.
    PhoneUnlinked,
    /// The account was removed while the features ran.
    AccountRemoved,
    /// The phone asked to bring the window forward with this `ms-phone:` URI.
    BringWindowForward { uri: String },
    /// The phone turned every feature off for this PC, or back on.
    AllFeaturesDisabled { disabled: bool },
    /// The phone record changed (name, permissions, capabilities).
    PhoneChanged,
    /// A connect exchange finished; `immediate` for one the user or app activation started.
    ConnectCompleted { immediate: bool, outcome: AppOutcome },
    /// A pairing session's step changed.
    Pairing { session: u64, step: PairingStep },
}

/// Receives what the core reports, on the thread that raised it.
pub trait CoreEvents: Send + Sync {
    /// Receives one event of the core.
    fn core_event(&self, event: CoreEvent);
    /// The features were installed: called once, before the App layer starts and before any feature
    /// handles a message.
    fn features_installed(&self, features: &Features);
}
