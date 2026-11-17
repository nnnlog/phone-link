//! A message's attachments.
//!
//! They are downloaded from the phone, opened in their app, and saved where the user chose.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cxx_qt::Threading;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::messaging::MessagePartRef;
use phonelink_engine::api::{FeatureApiError, RequestOutcome};
use serde_json::json;
use tracing::warn;

use super::arguments::SavePart;
use super::refresh::TRANSCRIPT;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::l10n::translate;
use crate::messaging_edges::file_types;
use crate::views::messages::{bare_content_type, parse_part_key, part_file_name, part_key};
use crate::{images, user_text};

/// After the phone accepted a download, its bytes are awaited this long before it counts as failed.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(60);

impl EngineBridge {
    /// Asks the phone for a part's bytes.
    ///
    /// The part shows as downloading until its bytes arrive with the conversation's content, or the
    /// request fails, or the bytes do not come within the wait — whichever conversation the window
    /// shows meanwhile.
    pub(super) fn download(mut self: Pin<&mut Self>, engine: Arc<Engine>, part: MessagePartRef) {
        if self.areas.messages.downloading(part) {
            return;
        }
        self.as_mut().msg().requested_downloads.insert(part);
        self.as_mut().mark(TRANSCRIPT);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.messaging().download_part(part).await;
            let _ = thread.queue(move |bridge| bridge.download_requested(part, result));
        });
    }

    fn download_requested(
        mut self: Pin<&mut Self>,
        part: MessagePartRef,
        result: Result<RequestOutcome, FeatureApiError>,
    ) {
        self.as_mut().msg().requested_downloads.remove(&part);
        self.as_mut().mark(TRANSCRIPT);
        let failure = match &result {
            Ok(outcome) => user_text::outcome_failure(outcome),
            Err(e) => Some(user_text::feature_error(e)),
        };
        if let Some(text) = failure {
            return self.show_failure(Failure::new("Attachment Not Downloaded", text));
        }
        if self.areas.messages.arrived(part) {
            return;
        }

        let state = self.as_mut().msg();
        state.download_waits += 1;
        let wait = state.download_waits;
        state.awaited_downloads.insert(part, wait);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            tokio::time::sleep(DOWNLOAD_WAIT).await;
            let _ = thread.queue(move |bridge| bridge.download_wait_ended(part, wait));
        });
    }

    /// The wait ran out without the shown transcript seeing the bytes.
    ///
    /// The part's conversation may not be the one shown, so the engine says whether they came
    /// meanwhile; only when they did not is the download reported as failed.
    fn download_wait_ended(self: Pin<&mut Self>, part: MessagePartRef, wait: u64) {
        if self.areas.messages.awaited_downloads.get(&part) != Some(&wait) {
            return;
        }
        let Some(engine) = self.engine.clone() else { return };

        let thread = self.qt_thread();
        // The bytes may be large: they are read off the GUI thread.
        self.runtime.spawn_blocking(move || {
            let arrived = engine.messaging().part_bytes(part).map(|bytes| bytes.is_some());
            let _ = thread.queue(move |mut bridge| {
                if bridge.areas.messages.awaited_downloads.get(&part) != Some(&wait) {
                    return;
                }
                bridge.as_mut().msg().awaited_downloads.remove(&part);
                bridge.as_mut().mark(TRANSCRIPT);
                match arrived {
                    Ok(true) => return,
                    Ok(false) => warn!("an attachment was not downloaded; its bytes did not arrive within the wait"),
                    Err(e) => warn!(error = %e, "a downloaded attachment was not read"),
                }
                let text = translate("Your phone didn't send the attachment.").to_owned();
                bridge.show_failure(Failure::new("Attachment Not Downloaded", text));
            });
        });
    }

    /// Opens audio, video, a document or a picture in its app, from a copy in the image cache.
    ///
    /// The cache is emptied when the account or the phone changes; the bytes are read and written off
    /// the GUI thread.
    pub(super) fn open_part(self: Pin<&mut Self>, engine: Arc<Engine>, part: MessagePartRef) {
        let Some(found) = self.areas.messages.find_part(part) else {
            warn!("the attachment to open is not in the shown transcript");
            return;
        };
        let content_type = bare_content_type(&found.content_type);
        let extension = file_types().database().extension_of(&content_type);
        let name = part_file_name(&found.name, extension.as_deref());
        let folder = images::cache_dir().join("images").join("message-attachments").join(part_key(part));

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            // The error is the text the window shows.
            let written = match engine.messaging().part_bytes(part) {
                Ok(Some(bytes)) => std::fs::create_dir_all(&folder)
                    .and_then(|()| std::fs::write(folder.join(&name), bytes))
                    .map(|()| images::file_url(&folder.join(&name)))
                    .map_err(|e| {
                        warn!(error = %e, "an attachment to open was not written");
                        user_text::something_went_wrong()
                    }),
                Ok(None) => Err(translate("This computer doesn't have the attachment.").to_owned()),
                Err(e) => Err(user_text::feature_error(&e)),
            };
            match written {
                Ok(url) => {
                    let _ = thread
                        .queue(move |bridge| bridge.emit_area_event("messages", "openFile", &json!({ "url": url })));
                }
                Err(text) => fail_later(&thread, Failure::new("Attachment Not Opened", text)),
            }
        });
    }

    /// Saves an attachment where the user chose, off the GUI thread.
    pub(super) fn save_part(self: Pin<&mut Self>, engine: Arc<Engine>, save: SavePart) {
        let Some(part) = parse_part_key(&save.key) else {
            warn!(key = %save.key, "messages.savePart names no part");
            return;
        };
        let Some(path) = images::local_path(&save.url) else {
            warn!(url = %save.url, "messages.savePart names no file");
            return;
        };

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            // The error is the text the window shows.
            let saved = match engine.messaging().part_bytes(part) {
                Ok(Some(bytes)) => std::fs::write(&path, bytes).map_err(|e| {
                    warn!(error = %e, "an attachment was not saved");
                    user_text::something_went_wrong()
                }),
                Ok(None) => Err(translate("This computer doesn't have the attachment.").to_owned()),
                Err(e) => Err(user_text::feature_error(&e)),
            };
            if let Err(text) = saved {
                fail_later(&thread, Failure::new("Couldn't Save the Attachment", text));
            }
        });
    }
}
