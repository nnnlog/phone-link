//! What the calls part keeps between calls, and the requests the user sent to the phone from it.

use phonelink_engine::api::calling::{ActiveCall, DialFailureKind};

use super::presentation::Presentation;
use super::ringer::Ringer;
use super::sync_wait::SyncWait;

/// What the calls part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    pub(super) presentation: Presentation,
    pub(super) ringer: Ringer,
    /// The phone's current calls as last read; a failed read keeps them.
    pub(super) calls: Vec<ActiveCall>,
    /// The last dial's failure, until the user closes it or dials again.
    pub(super) dial_notice: Option<DialFailureKind>,
    /// A *Connect*, *Try Again* or *Begin Pairing* runs.
    pub(super) connecting: bool,
    /// A dial runs.
    pub(super) dialing: bool,
    /// The number of a `tel:` link, until the Calls section's keypad took it.
    pub(super) keypad_number: Option<String>,
    /// The user turned off the notices of calls taken on the phone (they are on until the window's
    /// preferences say otherwise).
    pub(super) phone_call_notices_off: bool,
    /// The call history permission request of the Calls section.
    pub(super) history_request: PhoneRequest,
    /// The call history access request of Settings runs.
    pub(super) history_access_pending: bool,
    /// The contacts permission request of the Contacts section.
    pub(super) contacts_request: PhoneRequest,
    /// The contacts access request of Settings runs.
    pub(super) contacts_access_pending: bool,
    pub(super) calls_wait: SyncWait,
    pub(super) contacts_wait: SyncWait,
}

/// A request the user sent to the phone from a section's prompt.
#[derive(Default)]
pub(super) struct PhoneRequest {
    pub(super) pending: bool,
    /// The phone did not answer the last request.
    pub(super) timed_out: bool,
}
