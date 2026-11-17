//! Drag and drop between this computer and a mirrored window: the `mirror.drag*` actions, the phone's
//! files dragged out of a window, and the notice when the phone app takes none of the dropped files.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::screen_mirroring::{
    MirrorDragEnter, MirrorDragFile, MirrorPhoneDragItem, ScreenMirroringApi,
};
use serde_json::Value;
use tracing::warn;

use super::request::MirrorRequest;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;
use crate::images;
use crate::l10n::{fill, translate};
use crate::notifier::LocalNotification;

fn drag_files(request: &Value) -> Vec<MirrorDragFile> {
    let urls = request["urls"].as_array().map(Vec::as_slice).unwrap_or_default();
    urls.iter()
        .filter_map(|url| images::local_path(url.as_str()?))
        .map(|path| MirrorDragFile { path, mime_type: None })
        .collect()
}

/// Where the files dragged out of a mirrored app land: in the user's own cache, not the shared `/tmp`.
fn drag_folder(session_id: &str) -> PathBuf {
    images::cache_dir().join("drag").join(session_id.replace(['/', '\\'], "_"))
}

impl EngineBridge {
    /// Runs a `mirror.*` drag action; `None` for any other name.
    pub(super) fn perform_mirror_drag(
        mut self: Pin<&mut Self>,
        api: &Arc<ScreenMirroringApi>,
        name: &str,
        request: &MirrorRequest<'_>,
    ) -> Option<Result<(), FeatureApiError>> {
        let (id, x, y) = (request.id.clone(), request.x, request.y);
        let result = match name {
            "mirror.dragEnter" => {
                let outcome = api.drag_enter(id.clone(), drag_files(&request.json));
                let shown = match &outcome {
                    Ok(MirrorDragEnter::Accepted) => "accepted",
                    Ok(MirrorDragEnter::OverLimit) => "overLimit",
                    // A drag the engine could not take in is refused.
                    Ok(MirrorDragEnter::Refused) | Err(_) => "refused",
                };
                self.as_mut().mirror_drag(QString::from(id.as_str()), QString::from(shown));
                outcome.map(drop)
            }
            "mirror.dragOver" => {
                let copies = api.drag_over(id.clone(), x, y);
                let shown = if copies.as_ref().is_ok_and(|copies| *copies) { "copy" } else { "none" };
                self.as_mut().mirror_drag(QString::from(id.as_str()), QString::from(shown));
                copies.map(drop)
            }
            "mirror.dragLeave" => api.drag_leave(id),
            "mirror.dragDrop" => api.drag_drop(id),
            // A drag over the limits was dropped: the user is told why nothing happened.
            "mirror.dragOverLimit" => {
                self.as_mut().show_failure(Failure::new(
                    "Can't Send These Files",
                    translate("You can send up to 100 files at a time, each up to 500 MB and of a supported type.")
                        .to_owned(),
                ));
                Ok(())
            }
            "mirror.phoneDragEnded" => api.phone_drag_ended(id, request.flag("dropped")),
            "mirror.takePhoneDrag" => api.take_phone_drag(id.clone(), request.flag("held")).map(|items| {
                if let Some(items) = items {
                    self.as_mut().start_phone_drag(api, &id, items);
                }
            }),
            _ => return None,
        };
        Some(result)
    }

    /// Starts a drag of the phone's files out of window `id`: each lands in the drag's own folder once
    /// it arrives from the phone.
    fn start_phone_drag(
        self: Pin<&mut Self>,
        api: &Arc<ScreenMirroringApi>,
        id: &str,
        items: Vec<MirrorPhoneDragItem>,
    ) {
        let folder = drag_folder(id);
        let _ = std::fs::remove_dir_all(&folder);
        if let Err(e) = std::fs::create_dir_all(&folder) {
            warn!(error = %e, "the folder for the phone's drag was not made");
        }

        let mut urls = Vec::new();
        for item in items {
            let name = Path::new(&item.name)
                .file_name()
                .map_or_else(|| item.data_id.clone(), |n| n.to_string_lossy().into_owned());
            let path = folder.join(name);
            urls.push(images::file_url(&path));
            let (api, id) = (api.clone(), id.to_owned());
            self.runtime.spawn(async move {
                let target = path.to_string_lossy().into_owned();
                if let Err(e) = api.write_phone_drag_item(id, item.data_id, target).await {
                    warn!(error = %e, "a file dragged from the phone did not arrive");
                }
            });
        }

        let urls = serde_json::to_string(&urls).unwrap_or_default();
        self.mirror_phone_drag(QString::from(id), QString::from(urls.as_str()));
    }

    /// The phone app of window `session_id` took none of the files dropped on it: a notification names
    /// the app (an app window's), or says "this app".
    pub(super) fn drag_not_accepted(self: Pin<&mut Self>, session_id: &str) {
        let Some(notifier) = self.notifier.clone() else {
            warn!("the phone app took none of the dropped files, and no notification can say so");
            return;
        };
        let Some(engine) = self.engine.clone() else { return };

        let package = engine
            .clone()
            .screen_mirroring()
            .session(session_id.to_owned())
            .ok()
            .flatten()
            .filter(|session| !session.phone_screen)
            .map(|session| session.package_name);
        let app = package.and_then(|package| {
            let apps = engine.phone_apps().apps().unwrap_or_default();
            apps.into_iter().find(|app| app.package_name == package).map(|app| app.name)
        });
        let body = match app {
            Some(app) => fill(translate("%@ can't receive files from this computer."), &[app]),
            None => translate("This app can't receive files from this computer.").to_owned(),
        };

        let (thread, id) = (self.qt_thread(), session_id.to_owned());
        let handler = Arc::new(move |_: &str| {
            let id = id.clone();
            let _ = thread.queue(move |bridge| bridge.mirror_raise(QString::from(id.as_str())));
        });
        let notice = LocalNotification {
            id: &format!("screen_mirroring/dragNotAccepted/{session_id}"),
            title: translate("Phone Link"),
            body: &body,
            actions: &[],
            critical: false,
        };
        if let Err(e) = notifier.post_local(&notice, handler) {
            warn!(error = %e, "the drag-not-accepted notice was not posted");
        }
    }
}
