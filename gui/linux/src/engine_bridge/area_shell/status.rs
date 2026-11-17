//! The phone's status and its controls: Do Not Disturb, the ringer, ringing the phone, the prompts
//! these answer with, and joining or leaving the phone's hotspot.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::device_status::{DeviceStatusChange, RingDisplay, StatusControlOutcome, StatusPrompt};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::hotspot::HotspotError;
use serde_json::json;
use tracing::{info, warn};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision, fail_later};
use crate::l10n::translate;
use crate::user_text;
use crate::views::status::status_json;

fn prompt_kind(prompt: StatusPrompt) -> &'static str {
    match prompt {
        StatusPrompt::DndNotificationsPermission => "dndNotificationsPermission",
        StatusPrompt::DndRuleDisabled => "dndRuleDisabled",
        StatusPrompt::RingerNotificationsPermission => "ringerNotificationsPermission",
        StatusPrompt::DndTurnOffFailed => "dndTurnOffFailed",
        StatusPrompt::StopRinging => "stopRinging",
    }
}

pub(super) fn prompt_of(kind: &str) -> Option<StatusPrompt> {
    match kind {
        "dndNotificationsPermission" => Some(StatusPrompt::DndNotificationsPermission),
        "dndRuleDisabled" => Some(StatusPrompt::DndRuleDisabled),
        "ringerNotificationsPermission" => Some(StatusPrompt::RingerNotificationsPermission),
        "dndTurnOffFailed" => Some(StatusPrompt::DndTurnOffFailed),
        "stopRinging" => Some(StatusPrompt::StopRinging),
        _ => None,
    }
}

/// The hotspot's network to join by hand, when the credentials arrived but joining did not finish:
/// the alert's title and the passphrase it offers to copy.
pub(super) fn manual_join(error: &HotspotError) -> Option<(&'static str, &str)> {
    match error {
        HotspotError::NotVisible { passphrase, .. } => Some(("Your Phone's Hotspot Isn't Visible Yet", passphrase)),
        HotspotError::JoinFailed { passphrase, .. } => Some(("Couldn't Join Your Phone's Hotspot", passphrase)),
        _ => None,
    }
}

impl EngineBridge {
    /// Joins or leaves the phone's hotspot. A join whose network did not show up or could not be joined
    /// offers the network's password to join it by hand.
    pub(super) fn shell_hotspot(self: Pin<&mut Self>, engine: Arc<Engine>, connect: bool) {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let hotspot = engine.hotspot();
            let result = if connect { hotspot.connect().await.map(drop) } else { hotspot.disconnect().await };

            match result {
                Ok(()) => {}
                Err(e) => match manual_join(&e) {
                    Some((title, passphrase)) => {
                        warn!(error = %e, "the phone's hotspot was not joined; it can be joined by hand");
                        let payload = json!({
                            "title": translate(title),
                            "message": user_text::hotspot_error(&e),
                            "passphrase": passphrase,
                        });
                        let _ = thread.queue(move |bridge| bridge.emit_area_event("shell", "hotspotJoin", &payload));
                    }
                    None => {
                        let title =
                            if connect { "Couldn't Join Your Phone's Hotspot" } else { "Couldn't Complete Action" };
                        fail_later(&thread, Failure::new(title, user_text::hotspot_error(&e)));
                    }
                },
            }
            let _ = thread.queue(|bridge| bridge.bump(Revision::Settings));
        });
    }

    pub(in crate::engine_bridge) fn reload_status(mut self: Pin<&mut Self>) {
        if self.phone_connected {
            self.as_mut().rust_mut().areas.shell.connect_failure = None;
        }

        let Some(engine) = self.engine.clone() else { return };
        let json = match engine.device_status().status() {
            Ok(status) => status_json(&status),
            Err(_) => "{}".to_owned(),
        };
        self.as_mut().set_status_json(QString::from(json.as_str()));
    }

    /// Takes a status control's result and shows the prompt it answers with instead, when it does.
    fn status_control(
        mut self: Pin<&mut Self>,
        title: &'static str,
        result: Result<StatusControlOutcome, FeatureApiError>,
    ) {
        match result {
            Ok(StatusControlOutcome::Prompt { prompt }) => {
                self.as_mut().status_prompt(QString::from(prompt_kind(prompt)));
            }
            Ok(StatusControlOutcome::Sent | StatusControlOutcome::NotAvailable) => {}
            Err(FeatureApiError::NotStarted) => info!("a phone control was dropped; the engine has not started"),
            Err(e) => self.as_mut().show_failure(Failure::feature(title, &e)),
        }
        // The pending state shows at once.
        self.reload_status();
    }

    pub fn toggle_do_not_disturb(self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        self.status_control("Couldn't Change Do Not Disturb", engine.device_status().toggle_do_not_disturb());
    }

    pub fn cycle_ringer(self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        self.status_control("Couldn't Change the Ringer", engine.device_status().cycle_ringer_mode());
    }

    /// Rings the phone; while it rings, stops it (the first stop asks for confirmation).
    pub fn find_phone(self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let status = engine.device_status();
        let ringing = status
            .status()
            .ok()
            .and_then(|s| s.ring_my_phone)
            .is_some_and(|ring| matches!(ring.shown, RingDisplay::Ringing { .. }));

        if ringing {
            self.status_control("Couldn't Stop Ringing", status.stop_ringing());
        } else {
            self.status_control("Couldn't Ring Your Phone", status.ring_my_phone());
        }
    }

    pub(in crate::engine_bridge) fn device_status_changed(mut self: Pin<&mut Self>, change: DeviceStatusChange) {
        match change {
            DeviceStatusChange::Changed => {}
            DeviceStatusChange::PromptRequested { prompt } => {
                self.as_mut().status_prompt(QString::from(prompt_kind(prompt)));
            }
            // The prompt on screen stays; a queued one is no longer shown.
            DeviceStatusChange::PromptWithdrawn { prompt } => {
                self.as_mut().emit_area_event("shell", "promptWithdrawn", &json!({ "kind": prompt_kind(prompt) }));
            }
            // The low-battery switch is in the Notifications settings.
            DeviceStatusChange::LowBatteryNotificationActivated => {
                self.as_mut().emit_area_event("shell", "showSettings", &json!({ "pane": "notifications" }));
            }
        }
        self.as_mut().reload_status();
        self.bump(Revision::Settings);
    }
}
