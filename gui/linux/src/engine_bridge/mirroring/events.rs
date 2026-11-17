//! What screen mirroring's events change in the window: a session's window read again or raised, the
//! main window shown, a notification the phone did not open, and a drop the phone app refused.

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use phonelink_engine::api::screen_mirroring::ScreenMirroringChange;
use serde_json::json;
use tracing::info;

use crate::audio;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::{fill, translate};
use crate::mirror::presenter;

impl EngineBridge {
    pub(in crate::engine_bridge) fn mirroring_changed(mut self: Pin<&mut Self>, change: ScreenMirroringChange) {
        self.as_mut().watch_audio_outputs();
        match change {
            // Only the window of that session reads it again.
            ScreenMirroringChange::SessionChanged { session_id } => {
                self.emit_area_event("mirror", "sessionChanged", &json!({ "id": session_id }));
                return;
            }
            ScreenMirroringChange::SessionClosed { session_id } => presenter().forget(&session_id),
            ScreenMirroringChange::BringToFront { session_id } => {
                self.as_mut().bump(Revision::Mirroring);
                self.mirror_raise(QString::from(session_id.as_str()));
                return;
            }
            ScreenMirroringChange::ShowMainWindow => {
                self.show_section(QString::default());
                return;
            }
            ScreenMirroringChange::ShowApps => {
                self.show_section(QString::from("apps"));
                return;
            }
            ScreenMirroringChange::NotificationLaunched { opened, key } => {
                if !opened {
                    info!(key, "the phone didn't open the notification");
                    let phone = self.phone_display_name();
                    let message = fill(translate("%@ couldn't open the notification."), &[phone]);
                    self.show_failure(Failure::new("Couldn't Open on Your Phone", message));
                }
                return;
            }
            ScreenMirroringChange::DragNotAccepted { session_id } => {
                self.drag_not_accepted(&session_id);
                return;
            }
            ScreenMirroringChange::DragEnded { .. } => return,
            ScreenMirroringChange::Changed | ScreenMirroringChange::SessionOpened { .. } => {}
        }
        self.bump(Revision::Mirroring);
    }

    /// The phone's name for the windows' texts, "Your phone" while none is known.
    fn phone_display_name(&self) -> String {
        let name = self.phone_name.to_string();
        if name.is_empty() { translate("Your phone").to_owned() } else { name }
    }

    /// Tells the engine when the computer's audio outputs change, from the first mirroring event on:
    /// before it no app's audio plays here, so no output change matters.
    fn watch_audio_outputs(mut self: Pin<&mut Self>) {
        if self.areas.mirror.outputs_watched {
            return;
        }
        let Some(engine) = self.engine.clone() else { return };

        self.as_mut().rust_mut().areas.mirror.outputs_watched = true;
        audio::on_outputs_changed(move || {
            if let Err(e) = engine.clone().screen_mirroring().audio_output_changed() {
                info!(error = %e, "the audio output change was not passed on");
            }
        });
    }
}
