//! Drag and drop between the Mac and one window.
//!
//! It covers files dragged from the Mac onto the phone (the window is the drop target, the phone
//! pulls each file as a blob), and files dragged out of the phone onto the Mac (the phone hands the
//! drag over when the pointer leaves the video, and the Mac drag carries one file promise per
//! file).
//!
//! The phone → Mac side is the state machine of [`next`]; the window's other parts call in through
//! [`WindowDrag`]: the GUI's drag destination and drag source, the window's pointer and focus, its
//! state, and the `/drag/*` messages its channel receives.

use std::time::Duration;

mod gui;
mod phone_machine;
mod state;

pub use gui::{DragWriteError, EnterOutcome, MacDragFile, PhoneDragItem};
pub use phone_machine::{Next, Phase, PhoneDragEvent, mac_file_name, next};
use state::{FromPhone, FromPhoneItem, ItemStatus, State, ToPhone, cancel_blobs, receive, remove_dir};
pub use state::{WindowDrag, completion_text, progress_title, size_text};

mod inbound;
mod to_mac;
mod to_phone;

/// A Mac → phone drag takes at most this many items.
pub const MAX_ITEMS: usize = 100;
/// A Mac → phone drag's largest file: 500 MB of 1 048 576 bytes.
pub const MAX_FILE_BYTES: u64 = 500 * 1_048_576;
/// A drag entering this soon after the last leave waits for the rest of it.
pub const COOL_OFF: Duration = Duration::from_millis(500);
/// Focus lost with the left button held while a phone drag waits for a destination: the drag is
/// given up this much later.
pub const NO_DROP_WAIT: Duration = Duration::from_secs(3);
/// A phone drag whose blobs stop arriving for this long is cancelled; also the check's period.
pub const IDLE: Duration = Duration::from_secs(10);
/// A phone drag's progress notification is posted again at most this often.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);
/// The progress notification's action that cancels the drag.
pub const ACTION_CANCEL: &str = "cancel";

/// The MIME type of a file whose extension has none.
pub const DEFAULT_MIME: &str = "application/octetstream";

/// `DragCancelMessage.reason`.
pub mod reason {
    pub const UNKNOWN: i32 = 0;
    pub const NO_DATA_FOUND: i32 = 1;
    pub const USER_DID_NOT_DROP: i32 = 5;
    pub const DOWNLOAD_ACTIVE: i32 = 9;
    pub const USER_CANCELLED: i32 = 11;
}

/// `/drag/complete`'s `dropTarget` when the phone app took nothing.
pub const DROP_TARGET_NOT_SUPPORTED: &str = "notSupported";

#[cfg(test)]
mod tests;
