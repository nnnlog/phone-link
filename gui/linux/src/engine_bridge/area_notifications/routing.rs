//! The notifications part's `query` and `perform` names, each routed to the code that answers it.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::warn;

use super::queries::trace_bytes;
use super::state::PaneRequest;
use crate::engine_bridge::EngineBridge;
use crate::views::notifications::post_notification_link;

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_notifications(
        &self,
        engine: &Arc<Engine>,
        name: &str,
        _arg: &str,
    ) -> Option<Value> {
        let value = match name {
            "notifications.state" => self.notifications_state(engine),
            "notifications.settings" => self.notification_settings(engine),
            "notifications.apps" => self.notification_apps(engine),
            "notifications.postNotificationLink" => Value::from(post_notification_link(trace_bytes())),
            _ if name.starts_with("notifications.") => {
                warn!(action = name, "unknown query");
                Value::Null
            }
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn perform_notifications(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        let Some(action) = name.strip_prefix("notifications.") else { return false };
        let flag = arg == "true";
        match action {
            "reload" => self.reload_all(),
            "clearAll" => self.clear_all(engine),
            "requestAccess" => self.request_access(engine),
            "turnOn" => {
                self.as_mut().turn_on_notifications(&engine);
                self.reload_notification_state();
            }
            "setPcPermission" => {
                let result = engine.notifications().set_pc_permission(flag);
                self.as_mut().setting_changed(result);
                self.reload_notification_state();
            }
            "setThrottling" => {
                let result = engine.notifications().set_throttling_enabled(flag);
                self.as_mut().setting_changed(result);
                self.reload_notification_state();
            }
            "setLowBattery" => {
                let result = engine.device_status().set_low_battery_notifications_enabled(flag);
                self.as_mut().setting_changed(result);
                self.emit_changed();
            }
            "requestPaneAccess" => self.request_from_pane(engine, PaneRequest::Access),
            "requestSensitiveAccess" => self.request_from_pane(engine, PaneRequest::SensitiveContent),
            "openSettingsView" => self.open_settings_view(engine),
            "setAppChoice" => self.set_app_choice(engine, arg),
            "turnOffApp" => self.turn_off_app(engine, arg),
            "undoTurnOff" => self.undo_turn_off(engine, arg),
            "turnOnApp" => self.turn_on_app(engine, arg),
            "muteApp" => self.mute_app(engine, arg),
            "unmuteApp" => self.unmute_app(engine, arg),
            "reply" => self.reply(engine, arg),
            "focusReply" => self.emit_area_event("notifications", "focusReply", &json!({ "key": arg })),
            "restoreKnownApps" => self.restore_known_apps(arg),
            _ => warn!(action = name, "unknown action"),
        }
        true
    }
}
