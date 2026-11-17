//! Delete and the photo permission requests: the phone checks the photos before one confirmation and
//! deletes those the user confirmed, and is asked for photo access or for deleting.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::{Photo, PhotoDeleteCheck};
use serde_json::json;
use tracing::{debug, error};

use super::gallery::{can_delete, gallery};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::user_text;

pub(super) fn delete_failure_title(count: usize) -> &'static str {
    if count == 1 { "Couldn't Delete the Photo" } else { "Couldn't Delete the Photos" }
}

impl EngineBridge {
    /// Delete…: each photo that can be deleted is checked with the phone first (which may ask for the
    /// permission silently), then one confirmation counts those still in the gallery; the first that
    /// lacks the permission asks to allow deleting instead.
    pub(super) fn delete_photos(mut self: Pin<&mut Self>, engine: Arc<Engine>, ids: Vec<i64>) {
        let state = engine.clone().photos().state().ok();
        let targets: Vec<i64> = gallery(&engine)
            .iter()
            .filter(|photo| ids.contains(&photo.id) && can_delete(state.as_ref(), photo))
            .map(|photo| photo.id)
            .collect();
        if targets.is_empty() {
            return;
        }

        self.as_mut().mark_busy(&targets, true);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let mut check = Ok(PhotoDeleteCheck::Confirm);
            for id in &targets {
                check = engine.clone().photos().prepare_delete(*id).await;
                if !matches!(check, Ok(PhotoDeleteCheck::Confirm)) {
                    break;
                }
            }

            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().mark_busy(&targets, false);
                match check {
                    Ok(PhotoDeleteCheck::Confirm) => {
                        let photos = gallery(&engine);
                        let remaining: Vec<&Photo> =
                            photos.iter().filter(|photo| targets.contains(&photo.id)).collect();
                        if remaining.len() < targets.len() {
                            debug!(
                                photo_count = targets.len() - remaining.len(),
                                "photos left the gallery before the delete confirmation"
                            );
                        }

                        let Some(first) = remaining.first() else { return };
                        let name = if remaining.len() == 1 { first.name.clone() } else { String::new() };
                        let ids: Vec<i64> = remaining.iter().map(|photo| photo.id).collect();
                        bridge.emit_area_event(
                            "photos",
                            "prompt",
                            &json!({ "kind": "confirmDelete", "ids": ids, "name": name }),
                        );
                    }
                    Ok(PhotoDeleteCheck::MissingPermission) => {
                        bridge.emit_area_event("photos", "prompt", &json!({ "kind": "allowDeleting" }));
                    }
                    Err(e) => {
                        error!(error = %e, "the photo delete was not prepared");
                        bridge.show_failure(Failure::feature(delete_failure_title(targets.len()), &e));
                    }
                }
            });
        });
    }

    /// The user confirmed: the phone deletes the photos one after another. A failure it reports arrives
    /// as `DeleteFinished`; a call that fails stops the rest.
    pub(super) fn confirm_delete(self: Pin<&mut Self>, engine: Arc<Engine>, ids: Vec<i64>) {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            for id in &ids {
                if let Err(e) = engine.clone().photos().delete(*id).await {
                    error!(id, error = %e, "a photo was not deleted");
                    fail_later(&thread, Failure::feature(delete_failure_title(ids.len()), &e));
                    return;
                }
            }
        });
    }

    /// "Send Request to Phone" (photo access), "Allow Full Access" and "Allow on Phone" (deleting): the
    /// state change the request causes arrives as an event. A photo-access request answered with
    /// limited access is no failure.
    pub(super) fn request_photos_access(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str) {
        let (title, limited_access_answers): (&'static str, bool) = match name {
            "photos.requestAccess" => ("Couldn't Get Photo Access", true),
            "photos.requestFullAccess" => ("Couldn't Get Full Photo Access", true),
            _ => ("Couldn't Allow Deleting Photos", false),
        };
        let name = name.to_owned();

        self.as_mut().rust_mut().areas.photos.requests += 1;
        self.as_mut().photos_ui_changed();

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let api = engine.photos();
            let outcome = match name.as_str() {
                "photos.requestAccess" => api.request_photo_access().await,
                "photos.requestFullAccess" => api.request_full_access().await,
                _ => api.request_delete_permission().await,
            };

            let failure = match outcome {
                Ok(RequestOutcome::NeedsPermission) if limited_access_answers => None,
                Ok(outcome) => user_text::outcome_failure(&outcome).map(|message| {
                    error!(action = name.as_str(), reason = ?outcome, "the request to the phone did not succeed");
                    Failure::new(title, message)
                }),
                Err(e) => {
                    error!(action = name.as_str(), error = %e, "the request to the phone failed");
                    Some(Failure::feature(title, &e))
                }
            };
            if let Some(failure) = failure {
                fail_later(&thread, failure);
            }

            let _ = thread.queue(|mut bridge| {
                bridge.as_mut().rust_mut().areas.photos.requests -= 1;
                bridge.photos_ui_changed();
            });
        });
    }
}
