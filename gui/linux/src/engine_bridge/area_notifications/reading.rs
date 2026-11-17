//! Reading the notifications state and list from the engine, with the listed apps' settings, and
//! handing the window what it shows of them.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::pin::Pin;

use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::notifications::{AppNotificationSetting, NotificationsApi, PhoneNotification};
use serde_json::json;
use tracing::warn;

use crate::engine_bridge::EngineBridge;
use crate::user_text;
use crate::views::notifications::{Content, notifications_json};

/// The settings of the apps `list` shows, by package.
fn app_settings(api: &NotificationsApi, list: &[PhoneNotification]) -> HashMap<String, AppNotificationSetting> {
    let mut settings = HashMap::new();
    let mut failure = None;
    let packages: BTreeSet<&str> = list.iter().map(|n| n.package_name.as_str()).collect();
    for package in packages {
        match api.app_setting(package.to_owned()) {
            Ok(setting) => {
                settings.insert(package.to_owned(), setting);
            }
            Err(FeatureApiError::NotStarted | FeatureApiError::NoPhone) => break,
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
    }

    // One line for the list: the apps' reads fail together.
    if let Some(e) = failure {
        warn!(error = %e, "app notification settings were not read");
    }
    settings
}

impl EngineBridge {
    /// Reads the state and the list again (Try Again).
    pub(super) fn reload_all(mut self: Pin<&mut Self>) {
        self.as_mut().read_state();
        self.as_mut().read_list();
        self.publish();
    }

    pub(in crate::engine_bridge) fn reload_notifications(mut self: Pin<&mut Self>) {
        self.as_mut().read_list();
        self.publish();
    }

    pub(in crate::engine_bridge) fn reload_notification_state(mut self: Pin<&mut Self>) {
        self.as_mut().read_state();
        self.publish();
    }

    fn read_state(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let result = engine.notifications().state();

        let area = self.as_mut().notifications_mut();
        match result {
            Ok(state) => {
                area.state = Some(state);
                area.state_failure = None;
            }
            // Before the engine starts (or without a phone) there is nothing to show; that is not a failure.
            Err(FeatureApiError::NotStarted | FeatureApiError::NoPhone) => {
                area.state = None;
                area.state_failure = None;
            }
            Err(e) => {
                warn!(error = %e, "the notifications state was not read");
                area.state_failure = Some(user_text::feature_error(&e));
            }
        }
    }

    fn read_list(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let api = engine.clone().notifications();
        let list = match api.list() {
            Ok(list) => list,
            Err(FeatureApiError::NotStarted | FeatureApiError::NoPhone) => {
                let area = self.as_mut().notifications_mut();
                area.list.clear();
                area.app_settings.clear();
                area.pending_actions.clear();
                area.list_failure = None;
                return;
            }
            Err(e) => {
                warn!(error = %e, "the notification list was not read");
                self.as_mut().notifications_mut().list_failure = Some(user_text::feature_error(&e));
                return;
            }
        };
        let settings = app_settings(&api, &list);

        let area = self.as_mut().notifications_mut();
        {
            // Actions queued for a notification that left the list are forgotten: the engine drops them
            // unsent.
            let listed: HashSet<&str> = list.iter().map(|n| n.key.as_str()).collect();
            area.pending_actions.retain(|(key, _), _| listed.contains(key.as_str()));
        }
        area.list = list;
        area.app_settings = settings;
        area.list_failure = None;

        self.remember_listed_apps(&engine);
    }

    /// Hands the window what it shows: the list (only while the list column shows it, so the sidebar
    /// counts only listed notifications), whether every notification is paused, and the `changed` event.
    fn publish(mut self: Pin<&mut Self>) {
        let area = &self.areas.notifications;
        let json = match area.content() {
            Content::List => notifications_json(&area.list, &area.app_settings),
            _ => "[]".to_owned(),
        };
        let paused = area.state.as_ref().is_some_and(|s| s.muted_all_until_ms.is_some());

        self.as_mut().set_notifications_json(QString::from(json.as_str()));
        self.as_mut().set_notifications_muted(paused);
        self.emit_changed();
    }

    pub(super) fn emit_changed(self: Pin<&mut Self>) {
        self.emit_area_event("notifications", "changed", &json!({}));
    }
}
