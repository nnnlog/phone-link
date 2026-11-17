//! What the Settings window and the sidebar read of the phone: its feature switches, its wallpaper,
//! and the sections the sidebar lists; and the changes that make them read it again.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::calling::CallsAccess;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::wallpaper::WallpaperChange;
use serde_json::{Value, json};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::images;

pub(super) fn settings(engine: &Arc<Engine>) -> Value {
    let hotspot = engine.clone().hotspot().phone_state().ok().flatten();
    let calls = engine.clone().calling().control_state().ok();
    let clipboard_read = engine.clone().clipboard().phone_state();
    let available = clipboard_read.is_ok();
    let clipboard = clipboard_read.ok().flatten();
    let remote = engine.clone().remote_control().phone_state().ok().flatten();
    let files = engine.clone().file_transfer().settings().ok();

    json!({
        "available": available,
        "clipboardKnown": clipboard.is_some(),
        "clipboardCapable": clipboard.as_ref().is_some_and(|c| c.capable),
        "clipboardApplies": clipboard.as_ref().is_some_and(|c| c.applicable),
        "clipboardSupported": clipboard.as_ref().is_some_and(|c| c.supported),
        "clipboard": clipboard.as_ref().is_some_and(|c| c.toggle && c.applicable),
        "remoteControlKnown": remote.is_some(),
        "remoteControlCapable": remote.as_ref().is_some_and(|r| r.capable),
        "remoteControlV2": remote.as_ref().is_some_and(|r| r.v2_capable),
        "remoteControlApplies": remote.as_ref().is_some_and(|r| r.applicable),
        "remoteControl": remote.as_ref().is_some_and(|r| r.toggle && r.applicable),
        "fileSharingShown": files.as_ref().is_some_and(|f| f.share_toggle_shown),
        "fileSharing": files.as_ref().is_some_and(|f| f.share_enabled),
        "canSendFiles": files.as_ref().is_some_and(|f| f.offered_as_send_target),
        "hotspotSupported": hotspot.as_ref().is_some_and(|h| h.supported),
        "hotspotBusy": hotspot.as_ref().is_some_and(|h| h.busy),
        "hotspotJoined": hotspot.as_ref().is_some_and(|h| h.joined),
        "callsOffered": calls.as_ref().is_some_and(|c| c.offered),
        "callsEnabled": calls.as_ref().is_some_and(|c| c.enabled),
    })
}

pub(super) fn wallpaper(engine: &Arc<Engine>) -> Value {
    engine
        .clone()
        .wallpaper()
        .wallpaper()
        .ok()
        .flatten()
        .and_then(|w| images::by_content("wallpapers", &w.image))
        .map_or(Value::Null, Value::from)
}

/// The sections the sidebar lists beside Notifications, Messages and Photos: Calls for a phone that
/// has calling, Contacts for a phone that syncs its contacts, Apps while the phone opens its apps here.
pub(super) fn sections(engine: &Arc<Engine>) -> Value {
    let calls = engine.clone().calling().state().is_ok_and(|state| state.access != CallsAccess::Unsupported);
    let contacts = engine.clone().contacts().state().is_ok_and(|state| state.capable);
    let apps = engine.clone().phone_apps().state().is_ok_and(|state| state.apps_view_visible);
    json!({ "calls": calls, "contacts": contacts, "apps": apps })
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn wallpaper_changed(self: Pin<&mut Self>, _change: WallpaperChange) {
        self.bump(Revision::Wallpaper);
    }

    /// The clipboard's or remote control's settings changed.
    pub(in crate::engine_bridge) fn shell_settings_changed(self: Pin<&mut Self>) {
        self.bump(Revision::Settings);
    }
}
