//! The apps the settings pane lists: every app the selected phone's list has shown, remembered by the
//! window between runs and handed to it as the `knownApps` event.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use serde_json::json;
use tracing::warn;

use super::phone_id;
use crate::engine_bridge::EngineBridge;
use crate::views::notifications::{merge_known_apps, remember_apps};

impl EngineBridge {
    pub(super) fn remember_listed_apps(mut self: Pin<&mut Self>, engine: &Arc<Engine>) {
        let Some(phone) = phone_id(engine) else { return };
        let area = self.as_mut().notifications_mut();
        let changed = remember_apps(area.known_apps.entry(phone).or_default(), &area.list);
        if changed && area.known_apps_restored {
            self.emit_known_apps();
        }
    }

    /// The apps the window kept between runs join those this run has seen.
    pub(super) fn restore_known_apps(mut self: Pin<&mut Self>, arg: &str) {
        let stored = serde_json::from_str::<BTreeMap<String, BTreeMap<String, String>>>(arg).unwrap_or_else(|e| {
            warn!(error = %e, "the remembered notification apps are not readable; they start over");
            BTreeMap::new()
        });

        let area = self.as_mut().notifications_mut();
        area.known_apps = merge_known_apps(&area.known_apps, stored);
        area.known_apps_restored = true;
        self.emit_known_apps();
    }

    fn emit_known_apps(self: Pin<&mut Self>) {
        let payload = json!(self.areas.notifications.known_apps);
        self.emit_area_event("notifications", "knownApps", &payload);
    }
}
