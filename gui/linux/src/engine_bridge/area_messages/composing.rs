//! Writing the shown draft: its text, and the images picked, dropped or pasted into it.

use std::path::Path;
use std::pin::Pin;

use base64::Engine as _;
use cxx_qt::Threading;
use serde_json::json;
use tracing::warn;

use super::arguments::DroppedImage;
use super::draft::{DraftImage, MAX_IMAGES};
use super::draft_images::{DraftImageError, clipboard_images, data_image, read_image_file};
use super::refresh::{DETAIL, DRAFT, LIST};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::l10n::{fill_plural, translate};
use crate::views::messages::{ComposerTarget, clamp_utf16};
use crate::{images, user_text};

impl EngineBridge {
    pub(super) fn set_draft_text(mut self: Pin<&mut Self>, text: &str) {
        let state = &self.areas.messages;
        let Some(target) = state.composer_target() else { return };
        let clamped = clamp_utf16(text, state.composer().max_text_length);
        let lists_text = target == ComposerTarget::NewMessage && state.new_recipients.is_empty();
        let had_content = state.new_message_has_content();

        let state = self.as_mut().msg();
        let mut draft = state.drafts.remove(&target).unwrap_or_default();
        draft.text = clamped.clone();
        if !draft.is_empty() {
            state.drafts.insert(target, draft);
        }

        if clamped != text {
            self.as_mut().mark(DRAFT);
        }
        if lists_text || had_content != self.areas.messages.new_message_has_content() {
            self.mark(LIST | DETAIL);
        }
    }

    /// Adds images to the shown draft: images only, at most [`MAX_IMAGES`] per message.
    fn add_images(mut self: Pin<&mut Self>, items: Vec<DraftImage>) {
        let state = &self.areas.messages;
        let Some(target) = state.composer_target() else { return };
        if items.is_empty() {
            return;
        }
        let composer = state.composer();
        if !composer.enabled || !composer.can_attach {
            let text = translate("This conversation can't take attachments.").to_owned();
            return self.show_failure(Failure::new("Images Not Added", text));
        }

        let count = items.len();
        let pictures: Vec<DraftImage> = items.into_iter().filter(DraftImage::is_image).collect();
        let state = self.as_mut().msg();
        let mut draft = state.drafts.remove(&target).unwrap_or_default();
        let room = MAX_IMAGES.saturating_sub(draft.images.len());
        let offered = pictures.len();
        let took = offered.min(room);
        for mut image in pictures.into_iter().take(room) {
            state.next_image += 1;
            image.id = state.next_image;
            draft.images.push(image);
        }
        if !draft.is_empty() {
            state.drafts.insert(target, draft);
        }

        if offered < count {
            let text = translate("Only images can be added to a message.").to_owned();
            self.as_mut().show_failure(Failure::new("Images Not Added", text));
        } else if took < offered {
            let text = fill_plural("A message can have up to %lld images.", MAX_IMAGES, &[MAX_IMAGES.to_string()]);
            self.as_mut().show_failure(Failure::new("Images Not Added", text));
        }
        self.mark(LIST | DRAFT);
    }

    /// Reads picked or dropped files off the GUI thread, then adds them to the shown draft.
    pub(super) fn add_files(self: Pin<&mut Self>, urls: Vec<String>) {
        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let read: Result<Vec<DraftImage>, DraftImageError> = urls
                .iter()
                .map(|url| {
                    let path = images::local_path(url).ok_or_else(|| DraftImageError::NotAFile { url: url.clone() })?;
                    read_image_file(Path::new(&path))
                })
                .collect();
            match read {
                Ok(items) => {
                    let _ = thread.queue(move |bridge| bridge.add_images(items));
                }
                Err(e) => {
                    warn!(error = %e, "the picked or dropped images were not read");
                    fail_later(&thread, Failure::new("Images Not Added", user_text::something_went_wrong()));
                }
            }
        });
    }

    /// Image data dropped on the conversation, as base64.
    pub(super) fn add_image_data(self: Pin<&mut Self>, data: DroppedImage) {
        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let image = base64::engine::general_purpose::STANDARD
                .decode(data.data.as_bytes())
                .map_err(DraftImageError::Undecodable)
                .and_then(|bytes| data_image(bytes, data.content_type));
            match image {
                Ok(image) => {
                    let _ = thread.queue(move |bridge| bridge.add_images(vec![image]));
                }
                Err(e) => {
                    warn!(error = %e, "the dropped image was not read");
                    fail_later(&thread, Failure::new("Images Not Added", user_text::something_went_wrong()));
                }
            }
        });
    }

    /// Paste in the composer: the clipboard's images join the draft.
    ///
    /// They are copied image files, else image data when the clipboard holds no text; otherwise the
    /// window pastes text (the `pasteText` event).
    pub(super) fn paste(mut self: Pin<&mut Self>) {
        if self.areas.messages.pasteboard.is_none() {
            match crate::clipboard::system_pasteboard() {
                Ok(pasteboard) => self.as_mut().msg().pasteboard = Some(pasteboard),
                Err(e) => {
                    warn!(error = %e, "the clipboard is not readable for pasted images");
                    return self.emit_area_event("messages", "pasteText", &json!({}));
                }
            }
        }

        let Some(pasteboard) = self.areas.messages.pasteboard.clone() else { return };
        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || match clipboard_images(pasteboard.as_ref()) {
            Ok(Some(items)) => {
                let _ = thread.queue(move |bridge| bridge.add_images(items));
            }
            Ok(None) => {
                let _ = thread.queue(|bridge| bridge.emit_area_event("messages", "pasteText", &json!({})));
            }
            Err(e) => {
                warn!(error = %e, "the clipboard's images were not read");
                fail_later(&thread, Failure::new("Images Not Added", user_text::something_went_wrong()));
            }
        });
    }

    pub(super) fn remove_image(mut self: Pin<&mut Self>, id: u64) {
        let Some(target) = self.areas.messages.composer_target() else { return };
        let state = self.as_mut().msg();
        let Some(draft) = state.drafts.get_mut(&target) else { return };
        draft.images.retain(|image| image.id != id);
        if draft.is_empty() {
            state.drafts.remove(&target);
        }
        self.mark(LIST | DRAFT);
    }
}
