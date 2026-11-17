//! What the QML reads of the photos: the grid's items, what the section shows, a cell's picture, the
//! viewer's photo, and a photo's context menu.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::{Photo, PhotoDeleteState, PhotosState};
use serde_json::{Value, json};
use tracing::warn;

use super::action_file::mime_of;
use super::{files, quiet};
use crate::apps_host::{self, NameCollator, OpenWithApp, desktop};
use crate::engine_bridge::EngineBridge;
use crate::images;

fn delete_state(state: PhotoDeleteState) -> &'static str {
    match state {
        PhotoDeleteState::None => "none",
        PhotoDeleteState::Deleting => "deleting",
        PhotoDeleteState::Deleted => "deleted",
    }
}

/// The photo shows an image: a thumbnail or the full image is cached (what makes it actionable).
fn shows_image(photo: &Photo) -> bool {
    photo.has_thumbnail || photo.has_full
}

/// Delete is offered for the photo: the phone offers deleting and has it enabled, the photo shows an
/// image, and no delete of it is under way.
pub(super) fn can_delete(state: Option<&PhotosState>, photo: &Photo) -> bool {
    state.is_some_and(|state| state.delete_available)
        && shows_image(photo)
        && photo.delete_state == PhotoDeleteState::None
}

pub(super) fn photo_key(photo: &Photo) -> String {
    format!("{}-{}", photo.id, photo.modified_ms)
}

pub(super) fn photo_json(photo: &Photo) -> Value {
    json!({
        "id": photo.id,
        "name": photo.name,
        "takenMs": photo.taken_ms,
        "modifiedMs": photo.modified_ms,
        "width": photo.width,
        "height": photo.height,
        "hasThumbnail": photo.has_thumbnail,
        "hasFull": photo.has_full,
        "deleteState": delete_state(photo.delete_state),
    })
}

pub(super) fn gallery(engine: &Arc<Engine>) -> Vec<Photo> {
    engine.clone().photos().gallery().unwrap_or_default()
}

/// What the Photos section shows, in the order the states take precedence.
pub(super) fn content(state: Option<&PhotosState>, count: usize) -> &'static str {
    match state {
        None => "nothing",
        Some(state) if !state.enabled => "unavailable",
        Some(state) if !state.consent => "turnedOff",
        Some(state) if !state.photos_permission => "needsPhoneAccess",
        Some(_) if count == 0 => "empty",
        Some(_) => "gallery",
    }
}

pub(super) fn photos_state(engine: &Arc<Engine>) -> Value {
    let state = match engine.clone().photos().state() {
        Ok(state) => Some(state),
        Err(e) => {
            if !quiet(&e) {
                warn!(error = %e, "the photos state was not read");
            }
            None
        }
    };
    let count = if state.is_some() { gallery(engine).len() } else { 0 };

    let Some(state) = state else { return json!({ "content": content(None, 0) }) };
    json!({
        "content": content(Some(&state), count),
        "enabled": state.enabled,
        "consent": state.consent,
        "fullAccessReminder": state.full_access_reminder,
        "deletionAllowed": state.deletion_allowed,
        "deleteAvailable": state.delete_available,
        "newPhotoNotifications": state.new_photo_notifications,
        "syncing": state.syncing,
    })
}

/// The picture of a grid cell: the thumbnail, or without one the full image (the view scales it down).
pub(super) fn thumbnail(engine: &Arc<Engine>, arg: &str) -> Option<String> {
    let (id, modified) = arg.split_once(':')?;
    let (id, modified) = (id.parse::<i64>().ok()?, modified.parse::<i64>().ok()?);

    let api = engine.clone().photos();
    let key = format!("{id}-{modified}");
    images::cached("photo-thumbnails", &key, || api.thumbnail(id).ok().flatten())
        .or_else(|| images::cached("photos", &key, || api.full_image(id).ok().flatten()))
}

/// What the viewer shows of photo `id`: its file name, and its full image once cached, until then
/// its thumbnail.
pub(super) fn viewer(engine: &Arc<Engine>, arg: &str) -> Value {
    let Ok(id) = arg.parse::<i64>() else { return Value::Null };
    let Some(photo) = gallery(engine).into_iter().find(|photo| photo.id == id) else { return Value::Null };

    let api = engine.clone().photos();
    let title = api.save_as_name(id).ok().and_then(|name| files::file_name(&name).ok()).unwrap_or_default();
    let key = photo_key(&photo);
    let full = photo.has_full.then(|| images::cached("photos", &key, || api.full_image(id).ok().flatten())).flatten();
    let thumbnail = photo
        .has_thumbnail
        .then(|| images::cached("photo-thumbnails", &key, || api.thumbnail(id).ok().flatten()))
        .flatten();

    json!({ "id": id, "title": title, "takenMs": photo.taken_ms, "full": full, "thumbnail": thumbnail })
}

impl EngineBridge {
    /// The context menu of photo `id`: the photos it acts on, those of them that can be deleted, and
    /// the applications Open With lists for it (none when it acts on several, or the photo's name is
    /// no photo's or video's).
    pub(super) fn photos_menu(&self, engine: &Arc<Engine>, arg: &str) -> Value {
        let Ok(id) = arg.parse::<i64>() else { return Value::Null };
        let photos = gallery(engine);
        let order: Vec<i64> = photos.iter().map(|photo| photo.id).collect();
        let targets = self.areas.photos.selection.targets(&order, id);

        let state = engine.clone().photos().state().ok();
        let deletable: Vec<i64> = photos
            .iter()
            .filter(|photo| targets.contains(&photo.id) && can_delete(state.as_ref(), photo))
            .map(|photo| photo.id)
            .collect();

        let open_with: Vec<Value> = if targets.len() == 1 {
            self.open_with_apps(engine, id)
                .into_iter()
                .map(|app| json!({ "id": app.id, "name": app.name, "isDefault": app.is_default }))
                .collect()
        } else {
            Vec::new()
        };

        json!({ "targets": targets, "deletable": deletable, "openWith": open_with })
    }

    fn open_with_apps(&self, engine: &Arc<Engine>, id: i64) -> Vec<OpenWithApp> {
        let Ok(name) = engine.clone().photos().save_as_name(id) else { return Vec::new() };
        let Some(mime) = files::file_name(&name).ok().and_then(|file_name| mime_of(&file_name)) else {
            return Vec::new();
        };
        if !desktop::is_media(&mime) {
            return Vec::new();
        }

        let mut known = self.areas.photos.open_with.borrow_mut();
        known
            .entry(mime.clone())
            .or_insert_with(|| apps_host::open_with_apps(&mime, &NameCollator::for_session()))
            .clone()
    }
}
