//! An app's commands from a row's menu: turning its notifications off (with Undo) and back on, and
//! muting its desktop notifications.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::notifications::NotificationMuteDuration;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::failures::{app_outcome_later, error_text};
use super::phone_id;
use super::state::TurnedOff;
use crate::engine_bridge::EngineBridge;

/// The app an action names, from its JSON argument `{"package": …, "name": …, …}`.
pub(super) struct AppRequest {
    pub(super) package: String,
    pub(super) name: String,
    pub(super) request: Value,
}

pub(super) fn app_request(action: &str, arg: &str) -> Option<AppRequest> {
    let request: Value = match serde_json::from_str(arg) {
        Ok(request) => request,
        Err(e) => {
            warn!(action, error = %e, "the argument is not JSON");
            return None;
        }
    };

    match (request["package"].as_str(), request["name"].as_str()) {
        (Some(package), Some(name)) => Some(AppRequest { package: package.to_owned(), name: name.to_owned(), request }),
        _ => {
            warn!(action, "the argument names no app");
            None
        }
    }
}

pub(super) fn mute_duration(name: &str) -> Option<NotificationMuteDuration> {
    match name {
        "oneHour" => Some(NotificationMuteDuration::OneHour),
        "eightHours" => Some(NotificationMuteDuration::EightHours),
        "oneDay" => Some(NotificationMuteDuration::OneDay),
        _ => None,
    }
}

impl EngineBridge {
    /// Turns the app's notifications off on the phone and its desktop notifications off here, and
    /// offers Undo, which brings back what the app had.
    pub(super) fn turn_off_app(mut self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(app) = app_request("notifications.turnOffApp", arg) else { return };
        let title = "Couldn't Turn Off Notifications from %@";
        let previous = match engine.clone().notifications().app_setting(app.package.clone()) {
            Ok(setting) => setting.flags,
            Err(e) => {
                if let Some(text) = error_text(&e) {
                    self.show_app_failure(title, &app.name, text);
                }
                return;
            }
        };

        if let Some(phone) = phone_id(&engine) {
            self.as_mut().notifications_mut().turned_off =
                Some(TurnedOff { phone, package: app.package.clone(), name: app.name.clone(), previous });
            self.as_mut().emit_area_event(
                "notifications",
                "appTurnedOff",
                &json!({ "package": app.package, "name": app.name }),
            );
        }

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.notifications().toggle_app_from_list(app.package, false).await;
            app_outcome_later(&thread, title, app.name, result);
        });
    }

    /// Undo of the last turn-off (`arg`: its package): re-applies the flags the app had.
    pub(super) fn undo_turn_off(mut self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(record) = self.as_mut().notifications_mut().turned_off.take() else {
            warn!("Undo with no app turned off; dropped");
            return;
        };
        if record.package != arg {
            warn!("Undo names another app than the one last turned off; dropped");
            return;
        }
        if phone_id(&engine).as_deref() != Some(record.phone.as_str()) {
            info!("Undo of an app's turn-off dropped; another phone is selected");
            return;
        }

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.notifications().undo_app_turn_off(record.package, record.previous).await;
            app_outcome_later(&thread, "Couldn't Turn On Notifications from %@", record.name, result);
        });
    }

    /// Turns the app's notifications back on (a pinned notification of an app that is off).
    pub(super) fn turn_on_app(self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(app) = app_request("notifications.turnOnApp", arg) else { return };
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.notifications().toggle_app_from_list(app.package, true).await;
            app_outcome_later(&thread, "Couldn't Turn On Notifications from %@", app.name, result);
        });
    }

    /// Mutes the app's desktop notifications for an hour, eight hours or a day (this computer only).
    pub(super) fn mute_app(self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(app) = app_request("notifications.muteApp", arg) else { return };
        let Some(duration) = app.request["duration"].as_str().and_then(mute_duration) else {
            warn!("notifications.muteApp: unknown duration");
            return;
        };

        if let Err(e) = engine.notifications().mute_app(app.package, duration)
            && let Some(text) = error_text(&e)
        {
            self.show_app_failure("Couldn't Mute %@", &app.name, text);
        }
    }

    pub(super) fn unmute_app(self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(app) = app_request("notifications.unmuteApp", arg) else { return };
        if let Err(e) = engine.notifications().unmute_app(app.package)
            && let Some(text) = error_text(&e)
        {
            self.show_app_failure("Couldn't Unmute %@", &app.name, text);
        }
    }
}
