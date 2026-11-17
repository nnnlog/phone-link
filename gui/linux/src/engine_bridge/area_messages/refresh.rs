//! Marking views to read the section again.
//!
//! Their JSON is rebuilt once the current work is done, the pictures they lack are read off the GUI
//! thread, and the views are told.

use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use phonelink_engine::api::messaging::MessagePartRef;
use serde_json::json;
use tracing::info;

use super::reload::record;
use super::view_json::{PHOTOS, PICTURES, Wanted};
use crate::engine_bridge::EngineBridge;
use crate::images;
use crate::views::messages::conversations_json;

/// Which views read the section again.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Dirty {
    list: bool,
    detail: bool,
    draft: bool,
    transcript: bool,
}

pub(super) const LIST: Dirty = Dirty { list: true, detail: false, draft: false, transcript: false };
pub(super) const DETAIL: Dirty = Dirty { list: false, detail: true, draft: false, transcript: false };
pub(super) const DRAFT: Dirty = Dirty { list: false, detail: true, draft: true, transcript: false };
pub(super) const TRANSCRIPT: Dirty = Dirty { list: false, detail: true, draft: false, transcript: true };
pub(super) const ALL: Dirty = Dirty { list: true, detail: true, draft: true, transcript: true };

impl std::ops::BitOr for Dirty {
    type Output = Dirty;
    fn bitor(self, other: Dirty) -> Dirty {
        Dirty {
            list: self.list || other.list,
            detail: self.detail || other.detail,
            draft: self.draft || other.draft,
            transcript: self.transcript || other.transcript,
        }
    }
}

impl EngineBridge {
    /// Marks views to read again; they are rebuilt and told once the current work is done.
    pub(super) fn mark(mut self: Pin<&mut Self>, dirty: Dirty) {
        let state = self.as_mut().msg();
        state.dirty = state.dirty | dirty;
        if state.flush_queued {
            return;
        }
        state.flush_queued = true;
        let _ = self.qt_thread().queue(|bridge| bridge.flush_messages());
    }

    /// Rebuilds the marked views' JSON, starts reading the pictures they lack, and tells the views.
    pub(super) fn flush_messages(mut self: Pin<&mut Self>) {
        let dirty = std::mem::take(&mut self.as_mut().msg().dirty);
        self.as_mut().msg().flush_queued = false;

        let mut wanted = Wanted::default();
        if dirty.list {
            let state = &self.areas.messages;
            let list = state.list_json(&mut wanted);
            let compat =
                if state.gate().is_some() { "[]".to_owned() } else { conversations_json(&state.conversations) };
            self.as_mut().msg().list_json = list;
            self.as_mut().set_conversations_json(QString::from(compat.as_str()));
        }
        if dirty.detail {
            let detail = self.areas.messages.detail_json(&mut wanted);
            self.as_mut().msg().detail_json = detail;
        }
        if dirty.draft {
            let draft = self.areas.messages.draft_json();
            self.as_mut().msg().draft_json = draft;
        }
        if dirty.transcript {
            let transcript = self.areas.messages.transcript_json(&mut wanted).to_string();
            self.as_mut().set_messages_json(QString::from(transcript.as_str()));
        }
        self.as_mut().read_pictures(wanted);

        for (marked, name) in [(dirty.list, "list"), (dirty.detail, "detail"), (dirty.draft, "draft")] {
            if marked {
                self.as_mut().emit_area_event("messages", name, &json!({}));
            }
        }
    }

    /// Reads contact photos and message pictures into the image cache off the GUI thread.
    ///
    /// The views that wanted them read again once they are there.
    fn read_pictures(mut self: Pin<&mut Self>, wanted: Wanted) {
        let Some(engine) = self.engine.clone() else { return };
        let state = self.as_mut().msg();
        let photos: Vec<i64> = wanted.photos.into_iter().filter(|id| state.photos_reading.insert(*id)).collect();
        let pictures: Vec<(String, MessagePartRef)> =
            wanted.pictures.into_iter().filter(|(key, _)| state.pictures_reading.insert(key.clone())).collect();
        if photos.is_empty() && pictures.is_empty() {
            return;
        }

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let mut without_photo = Vec::new();
            for id in &photos {
                let read = engine.clone().contacts().thumbnail(*id);
                if let Err(e) = &read {
                    record(e, "a contact photo was not read");
                }
                if images::cached(PHOTOS, &id.to_string(), || read.ok().flatten()).is_none() {
                    without_photo.push(*id);
                }
            }
            let mut missing = Vec::new();
            for (key, part) in &pictures {
                let read = engine.clone().messaging().part_bytes(*part);
                if let Err(e) = &read {
                    record(e, "a message picture was not read");
                }
                if images::cached(PICTURES, key, || read.ok().flatten()).is_none() {
                    info!(%key, "a message picture has no bytes on this computer");
                    missing.push(key.clone());
                }
            }

            let _ = thread.queue(move |mut bridge| {
                let state = bridge.as_mut().msg();
                for id in &photos {
                    state.photos_reading.remove(id);
                }
                state.without_photo.extend(without_photo);
                for (key, _) in &pictures {
                    state.pictures_reading.remove(key);
                }
                state.pictures_missing.extend(missing);
                let photos_read = if photos.is_empty() { Dirty::default() } else { LIST | DETAIL };
                let pictures_read = if pictures.is_empty() { Dirty::default() } else { TRANSCRIPT };
                bridge.mark(photos_read | pictures_read);
            });
        });
    }
}
