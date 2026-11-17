//! What the list and detail columns and the settings pane read besides the list, and the trace id of
//! the link that asks for help with posting notifications.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::screen_mirroring::MirrorAppsFeatureType;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::phone_id;
use super::state::PaneRequest;
use crate::engine_bridge::EngineBridge;
use crate::views::notifications::{AppRow, app_rows_json};

/// Whether a notification can open on the phone in a window here: it can while the phone shows its
/// screen and its apps on this computer.
fn phone_screen_offered(engine: &Arc<Engine>) -> bool {
    engine.clone().screen_mirroring().status().is_ok_and(|status| {
        status.remoting_supported
            && status.phone_screen_apps
            && status.apps_feature_type != MirrorAppsFeatureType::Disabled
    })
}

/// 16 random bytes for a trace id, from the standard library's randomly keyed hasher over the time.
pub(super) fn trace_bytes() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_nanos());
    let half = |salt: u8| {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(nanos);
        hasher.write_u8(salt);
        hasher.finish().to_le_bytes()
    };

    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&half(0));
    bytes[8..].copy_from_slice(&half(1));
    bytes
}

impl EngineBridge {
    /// What the list and detail columns show besides the list.
    pub(super) fn notifications_state(&self, engine: &Arc<Engine>) -> Value {
        let area = &self.areas.notifications;
        let state = area.state.as_ref();
        json!({
            "content": area.content().name(),
            "failure": area.failure(),
            "pcPermission": state.is_some_and(|s| s.pc_permission),
            // Only a phone that reports its app list keeps per-app settings.
            "appCommands": state.is_some_and(|s| s.app_list_supported),
            "mutedAllUntilMs": state.and_then(|s| s.muted_all_until_ms),
            // Clear All leaves pinned notifications.
            "canClearAll": !area.clearing && area.list.iter().any(|n| !n.pinned),
            "requestingAccess": area.requesting_access,
            "openOnPhone": phone_screen_offered(engine),
        })
    }

    /// The Notifications settings pane's values; `available` is false until the engine answers.
    pub(super) fn notification_settings(&self, engine: &Arc<Engine>) -> Value {
        let area = &self.areas.notifications;
        let Some(state) = area.state.as_ref() else { return json!({ "available": false }) };
        let low_battery = match engine.clone().device_status().low_battery_notifications_enabled() {
            Ok(enabled) => Some(enabled),
            Err(e) => {
                info!(error = %e, "the low-battery setting was not read");
                None
            }
        };

        json!({
            "available": true,
            "pcPermission": state.pc_permission,
            "throttling": state.throttling_enabled,
            "lowBattery": low_battery,
            "appListSupported": state.app_list_supported,
            "capable": state.capable,
            "lowRamDevice": state.low_ram_device,
            "devicePermission": state.device_permission,
            "sensitivePromptOffered": state.sensitive_prompt_offered,
            "sensitivePermission": state.sensitive_permission,
            "requestingAccess": area.pane_requests.contains(&PaneRequest::Access),
            "requestingSensitive": area.pane_requests.contains(&PaneRequest::SensitiveContent),
        })
    }

    /// The settings pane's apps: every app the selected phone's list has shown, with its setting; a row
    /// whose change waits for the phone shows the user's choice. `null` when a setting could not be
    /// read (the pane keeps the rows it shows).
    pub(super) fn notification_apps(&self, engine: &Arc<Engine>) -> Value {
        let area = &self.areas.notifications;
        let Some(phone) = phone_id(engine) else { return Value::Null };
        let Some(known) = area.known_apps.get(&phone) else { return json!([]) };

        let api = engine.clone().notifications();
        let mut rows = Vec::with_capacity(known.len());
        for (package, name) in known {
            let setting = match api.app_setting(package.clone()) {
                Ok(setting) => setting,
                Err(e) => {
                    warn!(error = %e, "app notification settings were not read");
                    return Value::Null;
                }
            };
            let chosen = area.pending_apps.get(package).copied();
            rows.push(AppRow {
                package: package.clone(),
                name: name.clone(),
                flags: chosen.unwrap_or(setting.flags),
                muted_until_ms: setting.muted_until_ms,
                pending: chosen.is_some(),
            });
        }

        app_rows_json(rows)
    }
}
