//! How a send from the Send Files window ended.
//!
//! The outcome is what the window shows; the log keeps a line for it.

use phonelink_engine::api::file_transfer::{FileSendFailure, FileSendResult};
use serde_json::{Value, json};
use tracing::{debug, info, warn};

use crate::l10n::{fill_plural, translate, translated};
use crate::user_text;

/// How a send from the window ended, as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) title: String,
    pub(super) message: String,
    pub(super) succeeded: bool,
    /// The phone needs the files permission: the window offers to ask for it.
    pub(super) needs_permission: bool,
}

impl Outcome {
    pub(super) fn failed(message: String) -> Self {
        Self { title: translate("Couldn't Send").to_owned(), message, succeeded: false, needs_permission: false }
    }

    pub(super) fn json(&self) -> Value {
        json!({
            "title": self.title,
            "message": self.message,
            "succeeded": self.succeeded,
            "needsPermission": self.needs_permission,
        })
    }
}

/// How a send from the window ended.
///
/// A cancel is the user's own; the other failures say why.
pub(super) fn outcome_of(result: &FileSendResult) -> Outcome {
    let failure = match result {
        FileSendResult::Sent { count } => {
            return Outcome {
                title: translated("Sent"),
                message: fill_plural("%u files sent to your phone.", *count as usize, &[count.to_string()]),
                succeeded: true,
                needs_permission: false,
            };
        }
        FileSendResult::Failed { failure, .. } => failure,
    };

    let text = match failure {
        FileSendFailure::Cancelled => {
            return Outcome {
                title: translated("Canceled"),
                message: translated("The transfer was canceled."),
                succeeded: false,
                needs_permission: false,
            };
        }
        FileSendFailure::TurnedOff => translated("Sending files to this phone is turned off in Settings › Files."),
        FileSendFailure::PhoneNotSupported => translated("Your phone can't receive files from this computer."),
        FileSendFailure::NoFile => translated("Choose files to send; folders can't be sent."),
        FileSendFailure::AnotherTransferInProgress => {
            translated("Another transfer with your phone is in progress. Try again when it ends.")
        }
        FileSendFailure::PermissionRequired => {
            return Outcome {
                needs_permission: true,
                ..Outcome::failed(translated("Your phone needs to allow Link to Windows to receive files."))
            };
        }
        FileSendFailure::PcLocked => translated("Unlock this computer and try again."),
        FileSendFailure::Metered => translated("Your phone doesn't transfer files on a metered connection."),
        // The engine's message names the cause for the log (`log_send`), not for the window.
        FileSendFailure::Failed => user_text::something_went_wrong(),
    };
    Outcome::failed(text)
}

/// Logs how a send ended.
///
/// The files sent show at the info level, a failure as a warning, and the user's own cancel only at
/// the debug level.
pub(super) fn log_send(result: &FileSendResult) {
    match result {
        FileSendResult::Sent { count } => info!(file_count = count, "files sent to the phone"),
        FileSendResult::Failed { failure: FileSendFailure::Cancelled, .. } => debug!("the file send was cancelled"),
        FileSendResult::Failed { failure, message } => warn!(?failure, error = %message, "the files were not sent"),
    }
}
