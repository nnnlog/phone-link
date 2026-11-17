//! The `mirror.*` actions that wait for the phone: they run on the engine's runtime, and their failure
//! is shown once the phone answers.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::screen_mirroring::ScreenMirroringApi;

use super::request::MirrorRequest;
use crate::engine_bridge::EngineBridge;
use crate::mirror::keyboard_input_source;

impl EngineBridge {
    /// Starts a `mirror.*` action that waits for the phone; `None` for any other name.
    pub(super) fn perform_mirror_deferred(
        self: Pin<&mut Self>,
        api: &Arc<ScreenMirroringApi>,
        name: &str,
        request: &MirrorRequest<'_>,
    ) -> Option<Result<(), FeatureApiError>> {
        match name {
            "mirror.launchNotification"
            | "mirror.audioPermission"
            | "mirror.enableAudio"
            | "mirror.keyboardSettings" => {
                self.perform_mirror_later(api.clone(), name, request);
                Some(Ok(()))
            }
            _ => None,
        }
    }

    /// Runs a `mirror.*` action that waits for the phone, on the runtime.
    fn perform_mirror_later(
        self: Pin<&mut Self>,
        api: Arc<ScreenMirroringApi>,
        name: &str,
        request: &MirrorRequest<'_>,
    ) {
        let thread = self.qt_thread();
        let action = name.to_owned();
        let key = request.arg.to_owned();
        let open_settings = request.flag("openSettings");
        let id = request.id.clone();

        self.runtime.spawn(async move {
            let result = match action.as_str() {
                "mirror.launchNotification" => api.launch_notification(key, -1).await,
                "mirror.enableAudio" => api.enable_audio(id).await,
                // The settings of the input source in use when the button is pressed.
                "mirror.keyboardSettings" => api.request_keyboard_settings(keyboard_input_source().await).await,
                _ => api.request_audio_permission(open_settings).await,
            };
            let _ = thread.queue(move |bridge| {
                if let Err(e) = result {
                    bridge.report_mirror_failure(&action, &e);
                }
            });
        });
    }
}
