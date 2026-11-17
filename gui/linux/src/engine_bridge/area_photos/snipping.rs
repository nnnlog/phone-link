//! New-photo notifications: their settings and the permission they need, and a click on one, which
//! opens the photo in the default viewer (the photos feature's own notifications and the phone's).

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::PhotoAction;
use phonelink_engine::api::remote_snipping::{RemoteSnippingChange, RemoteSnippingPermission};
use serde_json::{Value, json};
use tracing::{error, info, warn};

use super::action_file::action_file;
use super::files::{self, OpenError};
use super::quiet;
use super::work::{WorkStopped, blocking};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision, fail_later};
use crate::l10n::translate;

/// Why a clicked new photo did not open.
#[derive(Debug, thiserror::Error)]
enum NewPhotoOpenError {
    /// The engine's full image of the photo was not read.
    #[error(transparent)]
    FullImage(#[from] FeatureApiError),
    #[error("the engine has no full image of it")]
    NoFullImage,
    #[error("its bytes are no image")]
    NotAnImage,
    /// The photo's file was not written; the text is what the window would show.
    #[error("{0}")]
    NotWritten(String),
    #[error(transparent)]
    NotOpened(#[from] OpenError),
    #[error(transparent)]
    Stopped(#[from] WorkStopped),
}

impl EngineBridge {
    pub(super) fn snipping_settings(engine: &Arc<Engine>) -> Value {
        match engine.clone().remote_snipping().settings() {
            Ok(settings) => json!({
                "applicable": settings.applicable,
                "enabled": settings.enabled,
                "needsPermission": settings.needs_permission,
                "requestingPermission": settings.requesting_permission,
                "coversPhotosNotifications": settings.covers_photos_notifications,
            }),
            Err(e) => {
                if !quiet(&e) && !matches!(e, FeatureApiError::NotAvailable { .. }) {
                    warn!(error = %e, "the new-photo notification settings were not read");
                }
                Value::Null
            }
        }
    }

    pub(super) fn perform_snipping(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) {
        match name {
            "snipping.setEnabled" => {
                if let Err(e) = engine.remote_snipping().set_enabled(arg == "true") {
                    self.as_mut().show_failure(Failure::setting(&e));
                }
                self.bump(Revision::Settings);
            }
            "snipping.requestPermission" => {
                let thread = self.qt_thread();
                self.runtime.spawn(async move {
                    match engine.remote_snipping().request_permission().await {
                        Ok(RemoteSnippingPermission::Failed { reason }) => {
                            error!(%reason, "the new-photo notification permission was not requested");
                            let message = translate(
                                "Your phone didn't answer. Make sure it's nearby and connected, then try again.",
                            );
                            fail_later(&thread, Failure::new("Couldn't Ask Your Phone", message.to_owned()));
                        }
                        Ok(_) => {}
                        Err(e) => {
                            error!(error = %e, "the new-photo notification permission was not requested");
                            fail_later(&thread, Failure::feature("Couldn't Ask Your Phone", &e));
                        }
                    }
                    let _ = thread.queue(|bridge| bridge.bump(Revision::Settings));
                });
            }
            other => warn!(action = other, "unknown action"),
        }
    }

    pub(in crate::engine_bridge) fn snipping_changed(self: Pin<&mut Self>, change: RemoteSnippingChange) {
        match change {
            RemoteSnippingChange::SettingsChanged => self.bump(Revision::Settings),
            RemoteSnippingChange::OpenImage { media_id, bytes } => {
                // A click hands the photo (or its thumbnail, when the photo did not come) to the default
                // application for its image type; only bytes that are an image are opened.
                self.runtime.spawn(async move {
                    let opened = blocking(move || {
                        match files::write_new_photo(media_id, &bytes).map_err(NewPhotoOpenError::NotWritten)? {
                            Some(path) => {
                                files::open_externally(&path.to_string_lossy()).map_err(NewPhotoOpenError::from)
                            }
                            None => Err(NewPhotoOpenError::NotAnImage),
                        }
                    })
                    .await;
                    if let Err(e) = opened {
                        warn!(media_id, error = %e, "the clicked new photo was not opened");
                    }
                });
            }
        }
    }

    /// A new-photo notification was clicked and the engine stored the full image: it opens in the
    /// default viewer when its name is a photo's or video's. Not started from the window, so a failure
    /// is only logged.
    pub(super) fn open_from_notification(self: Pin<&mut Self>, id: i64) {
        let Some(engine) = self.engine.clone() else { return };
        let file = match action_file(&engine, id, PhotoAction::Open) {
            Ok(file) => file,
            Err(e) => return info!(id, error = %e, "the clicked new photo is not opened"),
        };

        self.runtime.spawn(async move {
            let opened = blocking(move || -> Result<(), NewPhotoOpenError> {
                let data = engine.photos().full_image(id)?;
                let Some(data) = data else { return Err(NewPhotoOpenError::NoFullImage) };
                let path = files::write_photo(id, &file.file_name, &data).map_err(NewPhotoOpenError::NotWritten)?;
                Ok(files::open_externally(&path.to_string_lossy())?)
            })
            .await;
            if let Err(e) = opened {
                warn!(id, error = %e, "the clicked new photo was not opened");
            }
        });
    }
}
