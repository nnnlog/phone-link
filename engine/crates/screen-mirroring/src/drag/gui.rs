//! What the GUI passes and gets.

use std::path::PathBuf;

/// A file the Mac drags onto the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacDragFile {
    pub path: PathBuf,
    /// The MIME type of its extension, when the Mac knows one.
    pub mime_type: Option<String>,
}

/// The window's answer to a drag entering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnterOutcome {
    /// Copy: the phone was told (or will be, after the cool-off).
    Accepted,
    /// Nothing to send, or the window cannot take it now.
    Refused,
    /// Over the limits (too many items or a file too large): dropping it shows the over-limit notice.
    OverLimit,
}

/// One file of a phone → Mac drag, as the Mac drag offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneDragItem {
    pub data_id: String,
    /// Its name on the Mac ([`mac_file_name`]).
    pub name: String,
    pub size: u64,
    pub mime_type: String,
}

/// Why writing a phone file to its destination failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DragWriteError {
    #[error("no such item in the drag")]
    UnknownItem,
    #[error("the file did not arrive from the phone")]
    NotReceived,
    #[error("writing the file failed: {0}")]
    Io(String),
}
