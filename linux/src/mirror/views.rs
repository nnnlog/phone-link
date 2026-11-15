use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::screen_mirroring::{MirrorError, MirrorSessionView, MirrorSize};
use serde_json::{Value, json};

fn size(size: Option<MirrorSize>) -> Value {
    size.filter(|s| s.width > 0 && s.height > 0)
        .map_or(Value::Null, |s| json!({ "width": s.width, "height": s.height }))
}

fn error(error: Option<MirrorError>) -> Value {
    error.map_or(Value::Null, |e| json!({ "kind": format!("{:?}", e.kind), "action": format!("{:?}", e.action) }))
}

fn session(view: MirrorSessionView) -> Value {
    json!({
        "id": view.id,
        "package": view.package_name,
        "phoneScreen": view.phone_screen,
        "state": format!("{:?}", view.state),
        "error": error(view.error),
        "secureLocked": view.secure_locked,
        "unlockFromMac": view.unlock_from_mac,
        "restartTaskTransfer": view.restart_task_transfer,
        "restartLayoutSwitch": view.restart_layout_switch,
        "videoReady": view.video_ready,
        "videoSize": size(view.video_size),
        "deviceSize": size(view.device_resolution),
        "initialDeviceSize": size(view.initial_device_resolution),
        "rotation": view.rotation,
        "navigationButtons": view.navigation_buttons,
        "moveToMac": view.move_to_virtual_display,
        "screenLayout": view.screen_layout,
        "volume": view.volume,
        "muted": view.muted,
        "audioButton": format!("{:?}", view.audio_button),
    })
}

pub fn sessions(engine: &Arc<Engine>) -> Value {
    let list = engine.clone().screen_mirroring().sessions().unwrap_or_default();
    Value::Array(list.into_iter().map(session).collect())
}

pub fn status(engine: &Arc<Engine>) -> Value {
    let Ok(status) = engine.clone().screen_mirroring().status() else { return Value::Null };
    json!({
        "supported": status.remoting_supported,
        "phoneScreen": status.phone_screen_apps,
        "apps": format!("{:?}", status.apps_feature_type),
        "input": status.input_supported,
        "audio": status.audio_supported,
        "adaptiveLayout": status.adaptive_layout,
        "phoneState": format!("{:?}", status.phone_state),
        "phoneError": error(status.phone_error),
        "allowApps": status.settings.allow_apps,
        "nearby": status.nearby.map(|n| json!({
            "token": n.expiry_token,
            "tokenExpired": n.token_expired,
            "pinWrong": n.pin_code_wrong,
            "skipUnlock": n.skip_unlock,
            "skipVerification": n.skip_verification,
        })),
    })
}
