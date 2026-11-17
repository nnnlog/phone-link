//! Receiving files: the received-files folder and the engine's transfer events.
//!
//! The Files settings open the folder; the events name a received file to open, the folder to show, or
//! a change in a transfer's progress.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::file_transfer::FileTransferChange;
use tracing::warn;

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::system;

impl EngineBridge {
    pub(in crate::engine_bridge) fn file_transfer_changed(self: Pin<&mut Self>, change: FileTransferChange) {
        match change {
            FileTransferChange::OpenFile { path } => system::open_path(&path),
            // The received files' folder: it opens in a file manager window.
            FileTransferChange::RevealInFinder { path } => system::open_path(&path),
            FileTransferChange::Changed => self.bump(Revision::Transfers),
        }
    }
}

/// Shows the received-files folder in the file manager.
///
/// Before the first file arrives it may not exist, and its parent folder is shown instead.
pub(super) fn open_received_folder(engine: &Arc<Engine>) {
    let folder = match engine.clone().file_transfer().settings() {
        Ok(settings) => settings.received_files_folder,
        Err(e) => {
            warn!(error = %e, "the received-files folder was not read");
            return;
        }
    };
    let Some(folder) = folder else { return };

    let path = Path::new(&folder);
    if path.exists() {
        system::open_path(&folder);
    } else if let Some(parent) = path.parent() {
        system::reveal_path(&parent.to_string_lossy());
    }
}
