//! The `mirror.*` actions on the phone's screen, its apps' windows, the connection and the Phone
//! Screen settings: opening, closing and resuming windows, the phone's prompts, nearby sharing's
//! codes, the volume and the layout.

use std::sync::Arc;

use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::screen_mirroring::ScreenMirroringApi;

use super::request::MirrorRequest;
use crate::mirror::views;

fn unknown_session() -> FeatureApiError {
    FeatureApiError::UnknownItem { what: "mirroring session".into() }
}

/// Runs a `mirror.*` action on a session, the connection or the settings; `None` for any other name.
pub(super) fn session_action(
    api: &Arc<ScreenMirroringApi>,
    name: &str,
    request: &MirrorRequest<'_>,
) -> Option<Result<(), FeatureApiError>> {
    let (arg, id) = (request.arg, request.id.clone());
    let result = match name {
        "mirror.openPhoneScreen" => api.open_phone_screen().map(drop),
        // A phone app by its package name, or `{"package", "taskId"}` for a recent task of one.
        "mirror.launch" => match request.json["package"].as_str() {
            Some(package) => api.launch_app(package.to_owned(), request.json["taskId"].as_i64()).map(drop),
            None => api.launch_app(arg.to_owned(), None).map(drop),
        },
        "mirror.close" => api.close_session(id),
        "mirror.ready" => api.window_ready(id),
        "mirror.retry" => api.retry(),
        "mirror.wake" => api.wake(),
        "mirror.resendPermission" => api.resend_permission(),
        "mirror.allowAndRetry" => api.status().and_then(|status| {
            let mut settings = status.settings;
            settings.allow_apps = true;
            api.set_settings(settings)?;
            api.retry()
        }),
        "mirror.setting" => match request.json["key"].as_str().zip(request.json["value"].as_bool()) {
            Some((key, value)) => api.status().and_then(|status| {
                let settings = views::with_setting(status.settings, key, value)
                    .map_err(|e| FeatureApiError::InvalidInput { reason: e.to_string() })?;
                api.set_settings(settings)
            }),
            None => Err(FeatureApiError::InvalidInput { reason: format!("no setting and value in {arg}") }),
        },
        // Resume launches a paused window's app again. Continuing past the touch confirmation does
        // that, and skips the phone's launch checks.
        "mirror.resume" => api.continue_true_touch(id),
        "mirror.reopen" => api.reopen(id),
        "mirror.dismiss" => api.dismiss_error(id),
        "mirror.continueWithoutAudio" => api.continue_without_audio(id),
        "mirror.trueTouch" => api.status().and_then(|status| {
            if request.flag("doNotShow") {
                let mut settings = status.settings;
                settings.do_not_show_true_touch = true;
                api.set_settings(settings)?;
            }
            api.continue_true_touch(id)
        }),
        "mirror.moveToComputer" => {
            api.answer_move_to_virtual_display(id, request.flag("accept"), request.flag("doNotAsk"))
        }
        "mirror.confirmLayout" => api.confirm_layout_switch(id, request.flag("doNotAsk")),
        "mirror.nearbyRequestPin" => api.nearby_request_pin_code(),
        "mirror.nearbyPin" => {
            let pin = arg.trim();
            if pin.is_empty() { Ok(()) } else { api.nearby_send_pin_code(pin.to_owned()) }
        }
        "mirror.nearbySkipUnlock" => api.nearby_skip_unlock(),
        "mirror.nearbySkipVerification" => api.nearby_skip_verification(),
        "mirror.nearbyRefreshed" => api.nearby_code_refreshed(),
        "mirror.volume" => api.session(id).and_then(|session| {
            let session = session.ok_or_else(unknown_session)?;
            let volume = request.number("volume").round().clamp(0.0, 100.0) as i32;
            api.set_volume(session.package_name, volume, request.flag("muted"))
        }),
        "mirror.switchLayout" => api.switch_layout(id, request.number("layout") as i32),
        _ => return None,
    };
    Some(result)
}
