//! What the shell part keeps between calls.

use std::collections::BTreeSet;

use phonelink_engine::api::engine::DeviceRegistration;

use super::pairing::Pairing;

/// What the shell part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    /// Why the last connect the user asked for did not reach the phone; `None` once a connect
    /// succeeds or the phone is connected.
    pub(super) connect_failure: Option<String>,
    pub(super) pairing: Pairing,
    /// The account's sign-in name as last read (`None` until read): it is read off the GUI thread,
    /// from the keyring.
    pub(super) account_name: Option<Option<String>>,
    pub(super) reading_account_name: bool,
    /// This computer's registration as last read: `None` until read.
    pub(super) registration: Option<Option<DeviceRegistration>>,
    pub(super) reading_registration: bool,
    /// A sign-out or a reset of the registration runs.
    pub(super) signing_out: bool,
    /// The PCs being unlinked.
    pub(super) unlinking: BTreeSet<String>,
    /// Quitting waits for the phone session to stop.
    pub(super) quitting: bool,
    /// Whether SIGTERM and SIGINT are watched, to quit the app as its Quit command does.
    pub(super) quit_signals_watched: bool,
}
