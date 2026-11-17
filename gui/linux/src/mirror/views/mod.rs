//! Screen mirroring as the window reads it, as JSON: the phone's status and the Phone Screen settings,
//! the sessions, and a setting changed by its name.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::screen_mirroring::{
    MirrorAppsFeatureType, MirrorError, MirrorSessionView, MirrorSettings, MirrorSize, MirroringStatusView,
};
use serde_json::{Value, json};

fn size(size: Option<MirrorSize>) -> Value {
    size.filter(|s| s.width > 0 && s.height > 0)
        .map_or(Value::Null, |s| json!({ "width": s.width, "height": s.height }))
}

fn error(error: Option<MirrorError>) -> Value {
    error.map_or(Value::Null, |e| json!({ "kind": format!("{:?}", e.kind), "action": format!("{:?}", e.action) }))
}

pub fn session(view: MirrorSessionView) -> Value {
    json!({
        "id": view.id,
        "package": view.package_name,
        "phoneScreen": view.phone_screen,
        "state": format!("{:?}", view.state),
        "error": error(view.error),
        "secureLocked": view.secure_locked,
        "unlockFromComputer": view.unlock_from_mac,
        "restartTaskTransfer": view.restart_task_transfer,
        "restartLayoutSwitch": view.restart_layout_switch,
        "videoReady": view.video_ready,
        "videoSize": size(view.video_size),
        "deviceSize": size(view.device_resolution),
        "initialDeviceSize": size(view.initial_device_resolution),
        "rotation": view.rotation,
        "navigationButtons": view.navigation_buttons,
        "moveToComputer": view.move_to_virtual_display,
        "screenLayout": view.screen_layout,
        "lastScreenLayout": view.last_screen_layout,
        "volume": view.volume,
        "muted": view.muted,
        "silent": view.silent,
        "audioCaptureAllowed": view.audio_capture_allowed,
        "audioButton": format!("{:?}", view.audio_button),
    })
}

pub fn sessions(engine: &Arc<Engine>) -> Value {
    let list = engine.clone().screen_mirroring().sessions().unwrap_or_default();
    Value::Array(list.into_iter().map(session).collect())
}

/// The window `id`'s session; null when no open window has that id.
pub fn one_session(engine: &Arc<Engine>, id: &str) -> Value {
    match engine.clone().screen_mirroring().session(id.to_owned()) {
        Ok(view) => view.map_or(Value::Null, session),
        Err(e) => {
            tracing::debug!(session = %id, error = %e, "a mirroring session was not read");
            Value::Null
        }
    }
}

/// The settings by the names the Phone Screen pane changes them by (`mirror.setting`).
fn settings(s: &MirrorSettings) -> Value {
    json!({
        "allowApps": s.allow_apps,
        "blackScreen": s.black_screen,
        "audioStreaming": s.audio_streaming,
        "alwaysExpanded": s.always_expanded,
        "doNotAskRestart": s.do_not_ask_restart,
        "doNotAskTaskTransfer": s.do_not_ask_task_transfer,
        "doNotShowTrueTouch": s.do_not_show_true_touch,
        "doNotAskSkipUnlock": s.do_not_ask_skip_unlock,
        "doNotAskSkipVerification": s.do_not_ask_skip_verification,
        "doNotAskSwitchLayout": s.do_not_ask_switch_layout,
    })
}

/// Why a setting was not changed by its name.
#[derive(Debug, thiserror::Error)]
pub enum SettingError {
    #[error("{key} is no mirroring setting")]
    NotASetting { key: String },
}

/// `settings` with the one named `key` set to `value`; an error for a name that is no setting.
pub fn with_setting(mut settings: MirrorSettings, key: &str, value: bool) -> Result<MirrorSettings, SettingError> {
    let field = match key {
        "allowApps" => &mut settings.allow_apps,
        "blackScreen" => &mut settings.black_screen,
        "audioStreaming" => &mut settings.audio_streaming,
        "alwaysExpanded" => &mut settings.always_expanded,
        "doNotAskRestart" => &mut settings.do_not_ask_restart,
        "doNotAskTaskTransfer" => &mut settings.do_not_ask_task_transfer,
        "doNotShowTrueTouch" => &mut settings.do_not_show_true_touch,
        "doNotAskSkipUnlock" => &mut settings.do_not_ask_skip_unlock,
        "doNotAskSkipVerification" => &mut settings.do_not_ask_skip_verification,
        "doNotAskSwitchLayout" => &mut settings.do_not_ask_switch_layout,
        other => return Err(SettingError::NotASetting { key: other.to_owned() }),
    };
    *field = value;
    Ok(settings)
}

fn status_json(status: MirroringStatusView) -> Value {
    json!({
        "supported": status.remoting_supported,
        "phoneScreen": status.phone_screen_apps,
        "apps": format!("{:?}", status.apps_feature_type),
        // The phone screen and its apps open on this computer.
        "offered": status.remoting_supported
            && status.phone_screen_apps
            && status.apps_feature_type != MirrorAppsFeatureType::Disabled,
        "input": status.input_supported,
        "audio": status.audio_supported,
        "adaptiveLayout": status.adaptive_layout,
        "phoneState": format!("{:?}", status.phone_state),
        "phoneError": error(status.phone_error),
        "allowApps": status.settings.allow_apps,
        "settings": settings(&status.settings),
        "nearby": status.nearby.map(|n| json!({
            "token": n.expiry_token,
            "tokenExpired": n.token_expired,
            "pinWrong": n.pin_code_wrong,
            "skipUnlock": n.skip_unlock,
            "skipVerification": n.skip_verification,
        })),
    })
}

pub fn status(engine: &Arc<Engine>) -> Value {
    let Ok(status) = engine.clone().screen_mirroring().status() else { return Value::Null };
    status_json(status)
}

#[cfg(test)]
mod tests;
