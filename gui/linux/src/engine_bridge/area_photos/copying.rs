//! Copy: the photos' files on the clipboard, with the image itself for one photo that decodes.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_clipboard::image::ClipboardImageCodec;
use phonelink_clipboard::pasteboard::{ClipboardPasteboard, ClipboardPasteboardData, uti};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::PhotoAction;
use phonelink_engine::edge::EdgeError;
use tracing::{debug, error, info};

use super::action_file::{ActionFile, action_file, failure_title, fetch};
use super::files;
use super::work::{WorkStopped, blocking};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::images;
use crate::l10n::translate;

/// Why the clipboard took none of the photos.
#[derive(Debug, thiserror::Error)]
enum ClipboardWriteError {
    #[error(transparent)]
    Pasteboard(#[from] EdgeError),
    #[error(transparent)]
    Stopped(#[from] WorkStopped),
}

impl EngineBridge {
    /// Copy: the clipboard gets the photos' files (and the image itself for one photo that decodes) once
    /// every image is here; nothing when one of them fails (after its alert), or when something else
    /// was copied meanwhile.
    pub(super) fn copy_photos(mut self: Pin<&mut Self>, engine: Arc<Engine>, ids: Vec<i64>) {
        let title = failure_title(PhotoAction::Copy);
        let mut wanted = Vec::new();
        for id in &ids {
            match action_file(&engine, *id, PhotoAction::Copy) {
                Ok(file) => wanted.push(file),
                Err(message) => return self.show_failure(Failure::new(title, message)),
            }
        }
        if wanted.is_empty() {
            return;
        }

        let pasteboard = match self.areas.photos.pasteboard.clone() {
            Some(pasteboard) => pasteboard,
            None => match crate::clipboard::system_pasteboard() {
                Ok(pasteboard) => {
                    self.as_mut().rust_mut().areas.photos.pasteboard = Some(pasteboard.clone());
                    pasteboard
                }
                Err(e) => {
                    error!(error = %e, "no clipboard to copy photos to");
                    let message = translate("The clipboard didn't accept the photo.").to_owned();
                    return self.show_failure(Failure::new(title, message));
                }
            },
        };
        let count = pasteboard.change_count().ok();

        self.as_mut().mark_busy(&ids, true);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = copy(engine, wanted, pasteboard, count).await;
            if let Err(message) = result {
                fail_later(&thread, Failure::new(title, message));
            }
            let _ = thread.queue(move |bridge| bridge.mark_busy(&ids, false));
        });
    }
}

/// The copy itself: every image fetched and written, then the clipboard written unless it changed
/// meanwhile.
async fn copy(
    engine: Arc<Engine>,
    wanted: Vec<ActionFile>,
    pasteboard: Arc<dyn ClipboardPasteboard>,
    count: Option<i64>,
) -> Result<(), String> {
    let changed = |pasteboard: &Arc<dyn ClipboardPasteboard>| pasteboard.change_count().ok() != count;
    let single = wanted.len() == 1;
    let mut urls = Vec::new();
    let mut image = None;
    for file in wanted {
        let data = fetch(engine.clone(), file.id, PhotoAction::Copy).await?;
        if changed(&pasteboard) {
            debug!("the photo copy is dropped; the clipboard changed meanwhile");
            return Ok(());
        }

        let is_image = file.mime.as_deref().is_some_and(|mime| mime.starts_with("image/"));
        let (id, file_name) = (file.id, file.file_name);
        let (path, png) = blocking(move || -> Result<_, String> {
            let path = files::write_photo(id, &file_name, &data)?;
            // Applications paste images as PNG; a format the image crate does not read (HEIF) goes as
            // its file alone.
            let png = (single && is_image)
                .then(|| {
                    crate::clipboard::ImageCrateCodec
                        .encode_png(data)
                        .inspect_err(|e| info!(id, error = %e, "a photo is copied as a file only"))
                        .ok()
                })
                .flatten();
            Ok((path, png))
        })
        .await?;
        urls.push(images::file_url(&path));
        image = png;
    }
    if changed(&pasteboard) {
        debug!("the photo copy is dropped; the clipboard changed meanwhile");
        return Ok(());
    }

    let mut representations = vec![ClipboardPasteboardData {
        uti: uti::FILE_URL.to_owned(),
        data: format!("{}\r\n", urls.join("\r\n")).into_bytes(),
    }];
    if let Some(png) = image {
        representations.push(ClipboardPasteboardData { uti: uti::PNG.to_owned(), data: png });
    }

    let written = blocking(move || pasteboard.write(representations).map_err(ClipboardWriteError::from)).await;
    match written {
        Ok(write) if write.failure.is_none() => Ok(()),
        Ok(write) => {
            error!(failure = ?write.failure, "the clipboard took the photo only in part");
            Err(translate("The clipboard didn't accept the photo.").to_owned())
        }
        Err(e) => {
            error!(error = %e, "the photo was not copied");
            Err(translate("The clipboard didn't accept the photo.").to_owned())
        }
    }
}
