//! Save As… and saving into a folder: a photo's full image, fetched while the photo shows busy, written
//! where the user chose.

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::PhotoAction;
use serde_json::json;
use tracing::error;

use super::action_file::{Fetching, action_file, failure_title, fetch};
use super::files;
use super::work::blocking;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::l10n::{fill, fill_plural, translate};
use crate::user_text;

impl EngineBridge {
    /// Save As…: the full image starts on its way and the save panel opens with the photo's name.
    pub(super) fn start_save(mut self: Pin<&mut Self>, engine: Arc<Engine>, id: i64) {
        let file = match action_file(&engine, id, PhotoAction::SaveAs) {
            Ok(file) => file,
            Err(message) => return self.show_failure(Failure::new(failure_title(PhotoAction::SaveAs), message)),
        };

        let task = self.as_mut().fetch_marked(engine, id, PhotoAction::SaveAs);
        self.as_mut().rust_mut().areas.photos.pending_save = Some((id, task));
        self.emit_area_event("photos", "saveDialog", &json!({ "id": id, "name": file.file_name, "mime": file.mime }));
    }

    /// Fetches the full image of `id` for `action`, the photo busy until it arrives.
    pub(super) fn fetch_marked(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        id: i64,
        action: PhotoAction,
    ) -> Fetching {
        self.as_mut().mark_busy(&[id], true);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = fetch(engine, id, action).await;
            let _ = thread.queue(move |bridge| bridge.mark_busy(&[id], false));
            result
        })
    }

    /// The save panel chose `path`: the image is written there once it is here.
    pub(super) fn save_as(mut self: Pin<&mut Self>, engine: Arc<Engine>, id: i64, path: PathBuf) {
        let task = match self.as_mut().rust_mut().areas.photos.pending_save.take() {
            Some((pending, task)) if pending == id => task,
            _ => self.as_mut().fetch_marked(engine, id, PhotoAction::SaveAs),
        };

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let saved = async {
                let data = task.await.map_err(|e| {
                    error!(id, error = %e, "the fetch of a photo to save stopped");
                    user_text::something_went_wrong()
                })??;
                blocking(move || files::write_atomically(&path, &data)).await
            }
            .await;

            if let Err(message) = saved {
                error!(id, error = %message, "a photo was not saved");
                fail_later(&thread, Failure::new(failure_title(PhotoAction::SaveAs), message));
            }
        });
    }

    /// Save N Photos… and Save All Photos…: each photo's full image is fetched and written into
    /// `folder` in turn under its own name, numbered when the folder already holds it. Every photo is
    /// tried; one alert at the end counts those that weren't saved.
    pub(super) fn save_to_folder(mut self: Pin<&mut Self>, engine: Arc<Engine>, ids: Vec<i64>, folder: PathBuf) {
        self.as_mut().mark_busy(&ids, true);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let mut failures: Vec<String> = Vec::new();
            for id in &ids {
                let id = *id;
                let saved = async {
                    let file = action_file(&engine, id, PhotoAction::SaveAs)?;
                    let data = fetch(engine.clone(), id, PhotoAction::SaveAs).await?;
                    let folder = folder.clone();
                    blocking(move || files::write_numbered(&data, &file.name, &folder).map(drop)).await
                }
                .await;
                if let Err(message) = saved {
                    error!(id, error = %message, "a photo was not saved");
                    failures.push(message);
                }
                let _ = thread.queue(move |bridge| bridge.mark_busy(&[id], false));
            }

            let Some(reason) = failures.first().cloned() else { return };
            let saved = ids.len() - failures.len();
            let title = if failures.len() == 1 {
                translate(failure_title(PhotoAction::SaveAs)).to_owned()
            } else {
                fill_plural("Couldn't Save %lld Photos", failures.len(), &[failures.len().to_string()])
            };
            let message = if saved == 0 {
                reason
            } else {
                fill(
                    translate("%lld of %lld photos were saved. %@"),
                    &[saved.to_string(), ids.len().to_string(), reason],
                )
            };

            let _ = thread
                .queue(move |bridge| bridge.failure(QString::from(title.as_str()), QString::from(message.as_str())));
        });
    }
}
