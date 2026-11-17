//! What the files part keeps between calls.

use super::send_outcome::Outcome;
use crate::phone_files::MountLink;

/// What the files part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    /// A send from the Send Files window is under way: from the user's choice until its result.
    pub(super) window_send: bool,
    /// How the last send from the window ended, for the window; cleared when the next one starts.
    pub(super) outcome: Option<Outcome>,
    /// The phone was asked for access to its files and shows its prompt.
    pub(super) permission_requested: bool,
    /// Deleted files went to the phone's recycle bin: the Files settings tell the user once.
    pub(super) recycle_notice: bool,
    /// The app's side of the phone's files mount, while they are mounted.
    pub(super) mount: Option<MountLink>,
}
