//! The photos and apps part's `query` and `perform` names, each routed to the code that answers it.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use serde_json::Value;
use tracing::{info, warn};

use super::apps_section::{app_icon, apps_list};
use super::files;
use super::gallery::{gallery, photo_json, photos_state, thumbnail, viewer};
use super::opening::OpenWith;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::images;

/// The photo ids `arg` lists as JSON; `None`, logged, when it is no such list.
fn ids_of(arg: &str) -> Option<Vec<i64>> {
    serde_json::from_str::<Vec<i64>>(arg).inspect_err(|e| warn!(error = %e, "a list of photo ids was expected")).ok()
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_photos(&self, engine: &Arc<Engine>, name: &str, arg: &str) -> Option<Value> {
        let value = match name {
            "photos" => Value::Array(gallery(engine).iter().map(photo_json).collect()),
            "photos.state" => photos_state(engine),
            "photos.ui" => self.areas.photos.ui_json(),
            "photos.thumbnail" => thumbnail(engine, arg).map_or(Value::Null, Value::from),
            "photos.viewer" => viewer(engine, arg),
            "photos.menu" => self.photos_menu(engine, arg),
            "photos.dragFile" => self.photos_drag_file(engine, arg),
            "apps" => apps_list(engine),
            "appIcon" => app_icon(engine, self.areas.photos.icon_generation, arg).map_or(Value::Null, Value::from),
            "apps.view" => self.apps_view(engine),
            "apps.search" => self.apps_search(engine, arg),
            "apps.storedName" => {
                engine.clone().phone_apps().stored_name(arg.to_owned()).ok().flatten().map_or(Value::Null, Value::from)
            }
            "snipping.settings" => Self::snipping_settings(engine),
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn perform_photos(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        if name.starts_with("apps.") {
            self.perform_apps(engine, name, arg);
            return true;
        }
        if name.starts_with("snipping.") {
            self.perform_snipping(engine, name, arg);
            return true;
        }
        if !name.starts_with("photos.") {
            return false;
        }

        let handled = self.as_mut().photos_perform_view(&engine, name, arg)
            || self.as_mut().photos_perform_file_action(engine.clone(), name, arg);
        let done: Result<(), Failure> = if handled {
            Ok(())
        } else {
            match self.as_mut().photos_perform_phone_request(engine, name, arg) {
                Some(done) => done,
                None => {
                    warn!(action = name, "unknown action");
                    Ok(())
                }
            }
        };

        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        if matches!(
            name,
            "photos.turnOn" | "photos.setConsent" | "photos.setNotifications" | "photos.setDeletionAllowed"
        ) {
            self.as_mut().bump(Revision::Photos);
            self.bump(Revision::Settings);
        }
        true
    }

    /// Runs `name` when it is an action of the grid, the selection or the viewer; whether it is one.
    fn photos_perform_view(mut self: Pin<&mut Self>, engine: &Arc<Engine>, name: &str, arg: &str) -> bool {
        match name {
            "photos.layout" => match serde_json::from_str::<Vec<i64>>(arg) {
                Ok(visible) if !visible.is_empty() => {
                    if let Err(e) = engine.clone().photos().grid_layout(visible) {
                        info!(error = %e, "the photo grid layout was not passed on");
                    }
                }
                Ok(_) => {}
                Err(e) => warn!(error = %e, "photos.layout takes a JSON list of photo ids"),
            },
            "photos.shown" | "photos.hidden" => match arg.parse::<i64>() {
                Ok(id) => self.as_mut().cell_shown(engine, id, name == "photos.shown"),
                Err(e) => warn!(action = name, error = %e, "the action takes a photo id"),
            },
            "photos.left" => self.as_mut().gallery_left(engine),
            "photos.entered" => self.as_mut().gallery_entered(engine),
            "photos.select" | "photos.selectAll" | "photos.move" | "photos.contextMenu" => {
                self.as_mut().change_selection(engine, name, arg);
            }
            "photos.openViewer" => match arg.parse::<i64>() {
                Ok(id) => self.as_mut().open_viewer(engine, id),
                Err(e) => warn!(error = %e, "photos.openViewer takes a photo id"),
            },
            "photos.toggleViewer" => match (self.areas.photos.viewer, self.areas.photos.selection.focus()) {
                (Some(_), _) => self.as_mut().close_viewer(engine),
                (None, Some(focus)) => self.as_mut().open_viewer(engine, focus),
                (None, None) => {}
            },
            "photos.closeViewer" => self.as_mut().close_viewer(engine),
            _ => return false,
        }
        true
    }

    /// Runs `name` when it is a file action (copy, open, save, drag out) or the removal of their files
    /// at quit; whether it is one.
    fn photos_perform_file_action(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) -> bool {
        match name {
            "photos.copy" => {
                if let Some(ids) = ids_of(arg) {
                    self.as_mut().copy_photos(engine, ids);
                }
            }
            "photos.open" => {
                for id in ids_of(arg).unwrap_or_default() {
                    self.as_mut().open_photo(engine.clone(), id, OpenWith::Default);
                }
            }
            "photos.openWith" => {
                let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
                match (request["id"].as_i64(), request["app"].as_str()) {
                    (Some(id), Some(app)) => self.as_mut().open_photo(engine, id, OpenWith::App(app.to_owned())),
                    _ => warn!(%arg, "photos.openWith takes an id and an app"),
                }
            }
            "photos.openWithOther" => match arg.parse::<i64>() {
                Ok(id) => self.as_mut().open_photo(engine, id, OpenWith::Chooser),
                Err(e) => warn!(error = %e, "photos.openWithOther takes a photo id"),
            },
            "photos.startSave" => match arg.parse::<i64>() {
                Ok(id) => self.as_mut().start_save(engine, id),
                Err(e) => warn!(error = %e, "photos.startSave takes a photo id"),
            },
            "photos.saveAs" => {
                let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
                match (request["id"].as_i64(), request["path"].as_str().and_then(images::local_path)) {
                    (Some(id), Some(path)) => self.as_mut().save_as(engine, id, PathBuf::from(path)),
                    _ => warn!(%arg, "photos.saveAs takes an id and a local file URL"),
                }
            }
            "photos.cancelSave" => {
                self.as_mut().rust_mut().areas.photos.pending_save = None;
            }
            "photos.saveToFolder" => {
                let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
                let ids: Option<Vec<i64>> =
                    request["ids"].as_array().map(|ids| ids.iter().filter_map(Value::as_i64).collect());
                match (ids, request["folder"].as_str().and_then(images::local_path)) {
                    (Some(ids), Some(folder)) if !ids.is_empty() => {
                        self.as_mut().save_to_folder(engine, ids, PathBuf::from(folder));
                    }
                    _ => warn!(%arg, "photos.saveToFolder takes ids and a local folder URL"),
                }
            }
            "photos.prepareDrag" => match arg.parse::<i64>() {
                Ok(id) => self.as_mut().prepare_drag(engine, id),
                Err(e) => warn!(error = %e, "photos.prepareDrag takes a photo id"),
            },
            "photos.quit" => files::remove_all(),
            _ => return false,
        }
        true
    }

    /// Runs `name` when it is a delete, a photo permission request or a Photos setting: `None` when it is
    /// none of them, else what came of it.
    fn photos_perform_phone_request(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> Option<Result<(), Failure>> {
        let flag = arg == "true";
        let setting = |e: FeatureApiError| Failure::setting(&e);
        let done = match name {
            "photos.delete" => {
                if let Some(ids) = ids_of(arg) {
                    self.as_mut().delete_photos(engine, ids);
                }
                Ok(())
            }
            "photos.confirmDelete" => {
                if let Some(ids) = ids_of(arg) {
                    self.as_mut().confirm_delete(engine, ids);
                }
                Ok(())
            }
            "photos.allowDeleting" | "photos.requestAccess" | "photos.requestFullAccess" => {
                self.as_mut().request_photos_access(engine, name);
                Ok(())
            }
            "photos.dismissReminder" => engine
                .photos()
                .dismiss_full_access_reminder()
                .map_err(|e| Failure::feature("Couldn't Hide the Reminder", &e)),
            "photos.turnOn" => {
                engine.photos().set_consent(true).map_err(|e| Failure::feature("Couldn't Turn On Photos", &e))
            }
            "photos.setConsent" => engine.photos().set_consent(flag).map_err(setting),
            "photos.setNotifications" => engine.photos().set_new_photo_notifications(flag).map_err(setting),
            "photos.setDeletionAllowed" => engine.photos().set_deletion_allowed(flag).map_err(setting),
            _ => return None,
        };
        Some(done)
    }
}
