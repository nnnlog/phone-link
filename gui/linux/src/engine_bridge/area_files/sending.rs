//! Sending files to the phone.
//!
//! Files come from the Send Files window, which shows the progress and the outcome, or are handed to the
//! app from outside its windows; the window also asks the phone for the files permission.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::file_transfer::{FilePermissionResult, FileSendOrigin};
use serde_json::json;
use tracing::warn;

use super::send_outcome::{Outcome, log_send, outcome_of};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::l10n::translated;
use crate::{images, user_text};

/// The local paths of a JSON list of `file://` URLs, as QML writes them.
///
/// Other URLs are left out, as a drop of a web link is not a file.
pub(super) fn file_paths(arg: &str) -> Vec<String> {
    let urls: Vec<String> = match serde_json::from_str(arg) {
        Ok(urls) => urls,
        Err(e) => {
            warn!(%arg, error = %e, "the files to send are not a JSON list of URLs");
            return Vec::new();
        }
    };
    urls.iter()
        .filter_map(|url| {
            let path = images::local_path(url);
            if path.is_none() {
                warn!(%url, "a URL to send is not a file; it is not sent");
            }
            path
        })
        .collect()
}

impl EngineBridge {
    /// Sends the files of the JSON list of URLs `urls` from the Send Files window.
    ///
    /// The window shows the progress and the outcome; nothing is sent while a window send runs.
    pub(super) fn send_from_window(mut self: Pin<&mut Self>, engine: Arc<Engine>, urls: &str) {
        let paths = file_paths(urls);
        if paths.is_empty() || self.areas.files.window_send {
            return;
        }

        let mut rust = self.as_mut().rust_mut();
        rust.areas.files.outcome = None;
        rust.areas.files.window_send = true;
        self.as_mut().bump(Revision::Transfers);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let outcome = match engine.file_transfer().send(paths, FileSendOrigin::Window).await {
                Ok(result) => {
                    log_send(&result);
                    outcome_of(&result)
                }
                Err(e) => {
                    warn!(error = %e, "the file send did not start");
                    Outcome::failed(user_text::feature_error(&e))
                }
            };
            let _ = thread.queue(move |mut bridge| {
                let mut rust = bridge.as_mut().rust_mut();
                rust.areas.files.window_send = false;
                rust.areas.files.outcome = Some(outcome);
                bridge.bump(Revision::Transfers);
            });
        });
    }

    /// Sends files handed to the app from outside its windows.
    ///
    /// The engine's notifications report the progress and the outcome. While the phone is not offered
    /// as a target, the Send Files window says why.
    pub(super) fn send_from_outside(self: Pin<&mut Self>, engine: Arc<Engine>, urls: &str) {
        let paths = file_paths(urls);
        if paths.is_empty() {
            return;
        }
        if !engine.clone().file_transfer().settings().is_ok_and(|s| s.offered_as_send_target) {
            self.emit_area_event("files", "showSendWindow", &json!({}));
            return;
        }

        self.runtime.spawn(async move {
            match engine.file_transfer().send(paths, FileSendOrigin::ShareMenu).await {
                Ok(result) => log_send(&result),
                Err(e) => warn!(error = %e, "the file send did not start"),
            }
        });
    }

    /// The Send Files window's request for the files permission (the phone prompts).
    pub(super) fn request_send_permission(self: Pin<&mut Self>, engine: Arc<Engine>) {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.file_transfer().request_permission().await;
            let _ = thread.queue(move |mut bridge| {
                let failure = match result {
                    Ok(FilePermissionResult::Granted) => None,
                    Ok(FilePermissionResult::Required) => {
                        bridge.as_mut().rust_mut().areas.files.outcome = Some(Outcome {
                            title: translated("Check Your Phone"),
                            message: translated(
                                "Allow Link to Windows to receive files on your phone, then send again.",
                            ),
                            succeeded: false,
                            needs_permission: false,
                        });
                        bridge.as_mut().bump(Revision::Transfers);
                        return;
                    }
                    Ok(FilePermissionResult::Failed { reason }) => {
                        warn!(%reason, "the file-transfer permission was not requested");
                        Some(translated(
                            "Your phone didn't answer. Make sure it's nearby and connected, then try again.",
                        ))
                    }
                    Err(e) => {
                        warn!(error = %e, "the file-transfer permission was not requested");
                        Some(user_text::feature_error(&e))
                    }
                };

                match failure {
                    Some(message) => bridge.as_mut().emit_area_event(
                        "files",
                        "sendFailure",
                        &json!({ "title": translated("Couldn't Ask Your Phone"), "message": message }),
                    ),
                    None => {
                        bridge.as_mut().rust_mut().areas.files.outcome = None;
                        bridge.as_mut().bump(Revision::Transfers);
                    }
                }
            });
        });
    }
}
