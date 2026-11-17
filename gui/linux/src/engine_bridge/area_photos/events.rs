//! The photos feature's changes: the gallery read again (the selection and the photos' files follow
//! it), its state, a full image stored, a delete finished, and a clicked new-photo notification.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;

use cxx_qt::CxxQtType;
use phonelink_engine::api::photos::{Photo, PhotosChange};
use serde_json::json;
use tracing::warn;

use super::files;
use super::gallery::{gallery, photo_key};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::images;
use crate::l10n::translate;

impl EngineBridge {
    pub(in crate::engine_bridge) fn photos_changed(mut self: Pin<&mut Self>, change: PhotosChange) {
        match change {
            PhotosChange::GalleryChanged => {
                let Some(engine) = self.engine.clone() else { return };
                let photos = gallery(&engine);
                self.as_mut().gallery_reread(&photos);
                let focus = self.areas.photos.selection.focus();
                if self.areas.photos.viewer.is_some() && focus.is_none() {
                    self.as_mut().close_viewer(&engine);
                }
                self.as_mut().photos_ui_changed();
                self.bump(Revision::Photos);
            }
            PhotosChange::StateChanged => {
                self.as_mut().bump(Revision::Photos);
                self.bump(Revision::Settings);
            }
            PhotosChange::FullImageStored { id } => self.emit_area_event("photos", "fullImage", &json!({ "id": id })),
            PhotosChange::DeleteFinished { result } => {
                if result.failed {
                    let message = translate("The photo is still on your phone.").to_owned();
                    self.show_failure(Failure::new("Couldn't Delete the Photo", message));
                }
            }
            PhotosChange::OpenImage { id } => self.open_from_notification(id),
        }
    }

    /// The gallery was read again: the selection keeps the photos still listed, and the files of the
    /// photos that left or changed go.
    fn gallery_reread(mut self: Pin<&mut Self>, photos: &[Photo]) {
        let listed: HashSet<i64> = photos.iter().map(|photo| photo.id).collect();
        let versions: HashMap<i64, i64> = photos.iter().map(|photo| (photo.id, photo.modified_ms)).collect();

        let state = &mut self.as_mut().rust_mut().areas.photos;
        state.selection.retain(&listed);
        state.visible.retain(|id| listed.contains(id));
        for (id, modified) in &state.versions {
            if versions.get(id) != Some(modified) {
                files::remove_photo(*id);
            }
        }
        state.versions = versions;

        prune_cache("photo-thumbnails", photos.iter().filter(|photo| photo.has_thumbnail).map(photo_key).collect());
        prune_cache("photos", photos.iter().filter(|photo| photo.has_full).map(photo_key).collect());
    }
}

/// Removes the cached images of `kind` whose key is not in `keep` (photos that left or changed).
fn prune_cache(kind: &str, keep: HashSet<String>) {
    // `images::cached` keeps an image of `kind` at `<cache>/images/<kind>/<key>`.
    let dir = images::cache_dir().join("images").join(kind);
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        if !keep.contains(entry.file_name().to_string_lossy().as_ref())
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            warn!(path = %entry.path().display(), error = %e, "a cached photo image was not removed");
        }
    }
}
