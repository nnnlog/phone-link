//! Writing an authentication code to the pasteboard when the user presses Copy on a notification.
//!
//! The code goes on the pasteboard as plain text marked transient, so the clipboard recognizes no kind
//! and announces nothing to the phone; the write is still recorded as the app's own, like a phone clip.
//! It does not depend on the clipboard's switches and stops only once the feature has shut down.

use std::sync::{Arc, Weak};

use tokio::runtime::Handle;
use tracing::{debug, info, warn};

use phonelink_core::edge::EdgeError;
use phonelink_core::sync::MutexExt;
use phonelink_features::shared::notifier::CodeCopier;

use super::pasteboard::{ClipboardPasteboardData, uti};
use super::{Clipboard, blocking};

/// Why a code was not written.
#[derive(Debug, thiserror::Error)]
enum CodeWriteError {
    #[error("pasteboard write: {0}")]
    Pasteboard(#[source] EdgeError),
    /// The pasteboard refused a representation; the ones before it were written.
    #[error("pasteboard write stopped part-way (its change count is recorded): {0}")]
    StoppedPartWay(String),
}

impl Clipboard {
    /// The copier the notification router hands the Copy action to. Call inside the tokio runtime: the
    /// writes run on it.
    pub fn code_copier(&self) -> Arc<dyn CodeCopier> {
        Arc::new(CodeWriter { clipboard: self.me.clone(), runtime: Handle::current() })
    }

    /// Writes `code`: the transient marker first, so a refused marker leaves no plain text behind, then
    /// the text. Records the change count for echo suppression, also of a write stopped part-way.
    /// Blocking.
    fn write_code(&self, code: &str) -> Result<(), CodeWriteError> {
        if self.is_stopped() {
            return Ok(());
        }

        let representations = vec![
            ClipboardPasteboardData { uti: uti::TRANSIENT.to_string(), data: Vec::new() },
            ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_string(), data: code.as_bytes().to_vec() },
        ];

        let mut watch = self.watch.lock_or_panic();
        let written = self.pasteboard.write(representations).map_err(CodeWriteError::Pasteboard)?;
        watch.own_write = Some(written.change_count);
        match written.failure {
            Some(failure) => Err(CodeWriteError::StoppedPartWay(failure)),
            None => Ok(()),
        }
    }
}

/// The clipboard's [`CodeCopier`]: each code is written off the caller's thread, which may be the
/// main thread.
struct CodeWriter {
    clipboard: Weak<Clipboard>,
    runtime: Handle,
}

impl CodeCopier for CodeWriter {
    fn copy_code(&self, code: String) {
        let Some(clipboard) = self.clipboard.upgrade() else {
            debug!("authentication code not copied; the clipboard is gone");
            return;
        };

        self.runtime.spawn(async move {
            match blocking("code write", move || clipboard.write_code(&code)).await {
                Some(Ok(())) => info!("authentication code copied to the pasteboard"),
                Some(Err(e)) => warn!(error = %e, "authentication code not copied"),
                None => {}
            }
        });
    }
}
