//! Phone to Mac: the drag's state machine.

/// Where a phone → Mac drag is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Ready,
    WatchingPointer,
    RequestingOwnership,
    DragStarted,
    Drop,
    CancelWait,
}

/// What moves a phone → Mac drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneDragEvent {
    StartPhone,
    Complete,
    PhoneCancel,
    OutOfBounds,
    UserCancelled,
    ClipData,
    Dropped,
    NoItems,
    NoDropTarget,
    Failure,
    AllWritten,
}

/// Where an event takes the drag: on to a phase, or to its end (each ends in a reset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    To(Phase),
    Completed,
    Cancelled,
    /// Cancelled, telling the phone (`cancel` reason 5).
    CancelBroadcast,
}

/// The phone → Mac transitions; a pair the table does not have is [`Next::CancelBroadcast`].
pub fn next(from: Phase, event: PhoneDragEvent) -> Next {
    use Next::{CancelBroadcast, Cancelled, Completed, To};
    use Phase::{CancelWait, DragStarted, Drop, Ready, RequestingOwnership, WatchingPointer};
    use PhoneDragEvent as E;
    match (from, event) {
        (Ready, E::StartPhone) => To(WatchingPointer),
        (Ready | WatchingPointer, E::Complete) => Completed,
        (Ready | WatchingPointer | RequestingOwnership | DragStarted | Drop, E::PhoneCancel) => Cancelled,
        (WatchingPointer, E::OutOfBounds) => To(RequestingOwnership),
        (WatchingPointer | DragStarted, E::UserCancelled) => Cancelled,
        (RequestingOwnership, E::ClipData) => To(DragStarted),
        (DragStarted | Drop, E::Dropped) => To(Drop),
        (DragStarted, E::NoItems) => Cancelled,
        (DragStarted, E::NoDropTarget | E::Failure) => CancelBroadcast,
        (Drop, E::AllWritten | E::Complete) => Completed,
        (Drop, E::UserCancelled) => To(CancelWait),
        (CancelWait, E::UserCancelled) => Cancelled,
        _ => CancelBroadcast,
    }
}

/// The name a phone file gets on the Mac: `/` and `:` become `_`; an empty, `.` or `..` name is `_`.
pub fn mac_file_name(phone_name: &str) -> String {
    let name: String = phone_name.chars().map(|c| if c == '/' || c == ':' { '_' } else { c }).collect();
    if name.is_empty() || name == "." || name == ".." { "_".to_string() } else { name }
}
