//! The actions on the list and its notifications: Clear All, turning notifications on and asking the
//! phone for access from the list column, replying, and the invokables that pin, dismiss, press a
//! notification's button or pause every notification.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::warn;

use super::failures::error_text;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::user_text;

impl EngineBridge {
    pub(super) fn clear_all(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.notifications.clearing {
            return;
        }
        self.as_mut().notifications_mut().clearing = true;
        self.as_mut().emit_changed();

        let thread = self.qt_thread();
        // The phone clears them too; they leave the list only when it confirms.
        self.runtime.spawn(async move {
            let result = engine.notifications().clear_all().await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().notifications_mut().clearing = false;
                let text = match result {
                    Ok(outcome) => outcome.as_ref().and_then(user_text::outcome_failure),
                    Err(e) => error_text(&e),
                };
                if let Some(text) = text {
                    bridge.as_mut().show_failure(Failure::new("Couldn't Clear Notifications", text));
                }
                bridge.emit_changed();
            });
        });
    }

    pub(super) fn turn_on_notifications(self: Pin<&mut Self>, engine: &Arc<Engine>) {
        if let Err(e) = engine.clone().notifications().set_pc_permission(true) {
            self.report(&e, "Couldn't Turn On Notifications");
        }
    }

    /// Asks the phone for notification access from the list column; the phone shows the request.
    pub(super) fn request_access(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.notifications.requesting_access {
            return;
        }
        self.as_mut().notifications_mut().requesting_access = true;
        self.as_mut().emit_changed();

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.clone().notifications().request_access().await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().notifications_mut().requesting_access = false;
                match result {
                    Ok(outcome) => {
                        // A low-memory phone answers "not supported"; the list column then says so.
                        if outcome != RequestOutcome::DeviceNotSupported
                            && let Some(text) = user_text::outcome_failure(&outcome)
                        {
                            bridge.as_mut().show_failure(Failure::new("Couldn't Get Notification Access", text));
                        }
                        // This request also turns this computer's switch on once the phone grants access,
                        // so the list follows without a second click.
                        if matches!(outcome, RequestOutcome::Success | RequestOutcome::Accepted) {
                            bridge.as_mut().turn_on_notifications(&engine);
                        }
                    }
                    Err(e) => bridge.as_mut().report(&e, "Couldn't Get Notification Access"),
                }
                bridge.reload_notification_state();
            });
        });
    }

    pub(super) fn reply(mut self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
        let index = request["index"].as_i64().and_then(|index| i32::try_from(index).ok());
        let (Some(key), Some(index), Some(text)) = (request["key"].as_str(), index, request["text"].as_str()) else {
            warn!("notifications.reply: the argument is not a reply");
            return;
        };

        if self.as_mut().queue_reply(&engine, key, index, text) {
            self.emit_area_event("notifications", "replyQueued", &json!({ "key": key }));
        }
    }

    /// Queues `text` as the notification's reply; whether it was queued (blank text never is).
    fn queue_reply(mut self: Pin<&mut Self>, engine: &Arc<Engine>, key: &str, index: i32, text: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }

        match engine.clone().notifications().reply(key.to_owned(), index, text.to_owned()) {
            Ok(()) => {
                *self.as_mut().notifications_mut().pending_actions.entry((key.to_owned(), true)).or_default() += 1;
                true
            }
            Err(e) => {
                self.report(&e, "Couldn't Send Reply");
                false
            }
        }
    }

    pub fn set_notification_pinned(mut self: Pin<&mut Self>, key: &QString, pinned: bool) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.notifications().set_pinned(key.to_string(), pinned) {
            let title = if pinned { "Couldn't Pin Notification" } else { "Couldn't Unpin Notification" };
            self.as_mut().report(&e, title);
        }
    }

    /// Pauses every desktop notification from the phone for 24 hours, or ends the pause.
    pub fn mute_notifications(mut self: Pin<&mut Self>, muted: bool) {
        let Some(engine) = self.engine.clone() else { return };
        let result = engine.notifications().set_mute_all(muted);
        self.as_mut().setting_changed(result);
        self.reload_notification_state();
    }

    pub fn dismiss_notification(&self, key: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        let key = key.to_string();
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            match engine.notifications().dismiss(key).await {
                Ok(outcome) => {
                    if let Some(text) = user_text::outcome_failure(&outcome) {
                        fail_later(&thread, Failure::new("Couldn't Dismiss on Your Phone", text));
                    }
                }
                Err(e) => {
                    if let Some(text) = error_text(&e) {
                        fail_later(&thread, Failure::new("Couldn't Dismiss Notification", text));
                    }
                }
            }
        });
    }

    /// Presses one of the notification's buttons; its outcome arrives later.
    pub fn press_notification_action(mut self: Pin<&mut Self>, key: &QString, index: i32) {
        let Some(engine) = self.engine.clone() else { return };
        let key = key.to_string();
        match engine.notifications().press_action(key.clone(), index) {
            Ok(()) => *self.as_mut().notifications_mut().pending_actions.entry((key, false)).or_default() += 1,
            Err(e) => self.as_mut().report(&e, "Couldn't Complete Action"),
        }
    }
}
