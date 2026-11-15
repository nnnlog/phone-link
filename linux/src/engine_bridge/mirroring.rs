use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::screen_mirroring::{
    MirrorDragEnter, MirrorDragFile, MirrorFocus, MirrorInputSource, MirrorKeyAction, MirrorMouseButton,
    MirrorNavButton, MirrorTouchPhase, ScreenMirroringChange,
};
use phonelink_screen_mirroring::edge::DisplayGeometry;
use serde_json::Value;

use super::EngineBridge;
use super::features::Revision;
use crate::mirror::presenter;

fn number(request: &Value, key: &str) -> f64 {
    request[key].as_f64().unwrap_or_default()
}

fn flag(request: &Value, key: &str) -> bool {
    request[key].as_bool().unwrap_or_default()
}

pub(super) fn set_display(arg: &str) {
    let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
    let pixels = |key: &str| request[key].as_f64().map_or(0, |v| v.round().max(0.0) as u32);
    presenter().set_geometry(DisplayGeometry {
        usable_width: pixels("usableWidth"),
        usable_height: pixels("usableHeight"),
        dpi: pixels("dpi"),
        width: pixels("width"),
        height: pixels("height"),
    });
}

fn mouse_button(name: &str) -> MirrorMouseButton {
    match name {
        "left" => MirrorMouseButton::Left,
        "middle" => MirrorMouseButton::Middle,
        "right" => MirrorMouseButton::Right,
        _ => MirrorMouseButton::Other,
    }
}

fn touch_phase(name: &str) -> MirrorTouchPhase {
    match name {
        "down" => MirrorTouchPhase::Down,
        "up" => MirrorTouchPhase::Up,
        "cancelled" => MirrorTouchPhase::Cancelled,
        _ => MirrorTouchPhase::Moved,
    }
}

fn drag_files(request: &Value) -> Vec<MirrorDragFile> {
    let urls = request["urls"].as_array().map(Vec::as_slice).unwrap_or_default();
    urls.iter()
        .filter_map(|url| url.as_str()?.strip_prefix("file://"))
        .map(|path| MirrorDragFile { path: percent_decoded(path), mime_type: None })
        .collect()
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex =
            bytes.get(index + 1..index + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn drag_folder(session_id: &str) -> std::path::PathBuf {
    std::env::temp_dir().join("phonelink-drag").join(session_id)
}

impl EngineBridge {
    pub(super) fn mirroring_changed(mut self: Pin<&mut Self>, change: ScreenMirroringChange) {
        match change {
            ScreenMirroringChange::SessionClosed { session_id } => presenter().forget(&session_id),
            ScreenMirroringChange::BringToFront { session_id } => {
                self.as_mut().bump(Revision::Mirroring);
                self.mirror_raise(QString::from(session_id.as_str()));
                return;
            }
            ScreenMirroringChange::ShowMainWindow => {
                self.show_section(QString::default());
                return;
            }
            ScreenMirroringChange::ShowApps => {
                self.show_section(QString::from("apps"));
                return;
            }
            ScreenMirroringChange::NotificationLaunched { opened: false, .. } => {
                eprintln!("the phone could not open the notification");
            }
            _ => {}
        }
        self.bump(Revision::Mirroring);
    }

    pub(super) fn perform_mirror(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) {
        let api = engine.screen_mirroring();
        let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
        let id = request["id"].as_str().unwrap_or(arg).to_owned();
        let (x, y) = (number(&request, "x"), number(&request, "y"));
        let caps_lock = self.mirror_keyboard.caps_lock();
        let result = match name {
            "mirror.openPhoneScreen" => api.open_phone_screen().map(drop),
            "mirror.launch" => api.launch_app(id, None).map(drop),
            "mirror.close" => api.close_session(id),
            "mirror.ready" => api.window_ready(id),
            "mirror.focus" => {
                let focus = match request["focus"].as_str() {
                    Some("in") => MirrorFocus::InFocus,
                    Some("suspended") => MirrorFocus::Suspended,
                    _ => MirrorFocus::NotInFocus,
                };
                api.focus_changed(id, focus, caps_lock)
            }
            "mirror.inputFocus" => {
                let focused = flag(&request, "focused");
                if !focused {
                    self.as_mut().rust_mut().mirror_keyboard.release_all();
                }
                api.input_focus(id, focused, caps_lock)
            }
            "mirror.retry" => api.retry(),
            "mirror.wake" => api.wake(),
            "mirror.resendPermission" => api.resend_permission(),
            "mirror.allowAndRetry" => api.status().and_then(|status| {
                let mut settings = status.settings;
                settings.allow_apps = true;
                api.set_settings(settings)?;
                api.retry()
            }),
            "mirror.reopen" => api.reopen(id),
            "mirror.dismiss" => api.dismiss_error(id),
            "mirror.continueWithoutAudio" => api.continue_without_audio(id),
            "mirror.trueTouch" => api.status().and_then(|status| {
                if flag(&request, "doNotShow") {
                    let mut settings = status.settings;
                    settings.do_not_show_true_touch = true;
                    api.set_settings(settings)?;
                }
                api.continue_true_touch(id)
            }),
            "mirror.moveToMac" => {
                api.answer_move_to_virtual_display(id, flag(&request, "accept"), flag(&request, "doNotAsk"))
            }
            "mirror.confirmLayout" => api.confirm_layout_switch(id, flag(&request, "doNotAsk")),
            "mirror.nearbyRequestPin" => api.nearby_request_pin_code(),
            "mirror.nearbyPin" => api.nearby_send_pin_code(arg.to_owned()),
            "mirror.nearbySkipUnlock" => api.nearby_skip_unlock(),
            "mirror.nearbySkipVerification" => api.nearby_skip_verification(),
            "mirror.nearbyRefreshed" => api.nearby_code_refreshed(),
            "mirror.nav" => {
                let button = match request["button"].as_str() {
                    Some("home") => MirrorNavButton::Home,
                    Some("recents") => MirrorNavButton::Recents,
                    _ => MirrorNavButton::Back,
                };
                api.nav_button(id, button)
            }
            "mirror.mouse" => {
                let _ = match request["button"].as_str() {
                    Some(button) => api.mouse_button(id, mouse_button(button), flag(&request, "down"), x, y),
                    None => api.mouse_move(id, x, y),
                };
                Ok(())
            }
            "mirror.wheel" => {
                let delta = |key: &str| number(&request, key).round() as i32;
                let _ = api.mouse_wheel(id, x, y, delta("dx"), delta("dy"));
                Ok(())
            }
            "mirror.key" => {
                let scan = request["scan"].as_u64().and_then(|v| u32::try_from(v).ok()).unwrap_or_default();
                let down = flag(&request, "down");
                if let Some((key, flags)) = self.as_mut().rust_mut().mirror_keyboard.event(scan, down) {
                    let action = match (down, flag(&request, "repeat")) {
                        (false, _) => MirrorKeyAction::Up,
                        (true, true) => MirrorKeyAction::Repeat,
                        (true, false) => MirrorKeyAction::Down,
                    };
                    let _ = api.key(id, key, action, flags);
                }
                Ok(())
            }
            "mirror.volume" => api.sessions().and_then(|sessions| {
                let package = sessions.into_iter().find(|s| s.id == id).map(|s| s.package_name).unwrap_or_default();
                let volume = number(&request, "volume").round().clamp(0.0, 100.0) as i32;
                api.set_volume(package, volume, flag(&request, "muted"))
            }),
            "mirror.switchLayout" => api.switch_layout(id, number(&request, "layout") as i32),
            "mirror.touch" => {
                let pointer = request["pointer"].as_u64().unwrap_or_default();
                let phase = touch_phase(request["phase"].as_str().unwrap_or_default());
                let _ = api.touch(
                    id,
                    pointer,
                    phase,
                    x,
                    y,
                    number(&request, "pressure"),
                    number(&request, "orientation"),
                    number(&request, "width"),
                    number(&request, "height"),
                );
                Ok(())
            }
            "mirror.pen" => {
                let _ = api.pen(
                    id,
                    x,
                    y,
                    number(&request, "pressure"),
                    number(&request, "rotation"),
                    flag(&request, "inRange"),
                    flag(&request, "inContact"),
                );
                Ok(())
            }
            "mirror.dragEnter" => {
                let outcome = api.drag_enter(id.clone(), drag_files(&request));
                if let Ok(outcome) = &outcome {
                    let shown = match outcome {
                        MirrorDragEnter::Accepted => "accepted",
                        MirrorDragEnter::Refused => "refused",
                        MirrorDragEnter::OverLimit => "overLimit",
                    };
                    self.as_mut().mirror_drag(QString::from(id.as_str()), QString::from(shown));
                }
                outcome.map(drop)
            }
            "mirror.dragOver" => {
                let copies = api.drag_over(id.clone(), x, y).unwrap_or_default();
                self.as_mut()
                    .mirror_drag(QString::from(id.as_str()), QString::from(if copies { "copy" } else { "none" }));
                Ok(())
            }
            "mirror.dragLeave" => api.drag_leave(id),
            "mirror.dragDrop" => api.drag_drop(id),
            "mirror.phoneDragEnded" => api.phone_drag_ended(id, flag(&request, "dropped")),
            "mirror.takePhoneDrag" => {
                if let Ok(Some(items)) = api.take_phone_drag(id.clone(), flag(&request, "held")) {
                    let folder = drag_folder(&id);
                    let _ = std::fs::remove_dir_all(&folder);
                    if let Err(e) = std::fs::create_dir_all(&folder) {
                        eprintln!("the folder for the phone's drag was not made: {e}");
                    }
                    let mut urls = Vec::new();
                    for item in items {
                        let name = std::path::Path::new(&item.name)
                            .file_name()
                            .map_or_else(|| item.data_id.clone(), |n| n.to_string_lossy().into_owned());
                        let path = folder.join(name);
                        urls.push(format!("file://{}", path.display()));
                        let api = api.clone();
                        let id = id.clone();
                        self.runtime.spawn(async move {
                            let target = path.to_string_lossy().into_owned();
                            if let Err(e) = api.write_phone_drag_item(id, item.data_id, target).await {
                                eprintln!("a file dragged from the phone did not arrive: {e}");
                            }
                        });
                    }
                    let urls = serde_json::to_string(&urls).unwrap_or_default();
                    self.as_mut().mirror_phone_drag(QString::from(id.as_str()), QString::from(urls.as_str()));
                }
                Ok(())
            }
            "mirror.resume" | "mirror.audioPermission" | "mirror.enableAudio" | "mirror.keyboardSettings" => {
                let thread = self.qt_thread();
                let action = name.to_owned();
                let open_settings = flag(&request, "openSettings");
                let input_source = if flag(&request, "inputMethod") {
                    MirrorInputSource::InputMethod
                } else {
                    MirrorInputSource::Layout
                };
                self.runtime.spawn(async move {
                    let result = match action.as_str() {
                        "mirror.resume" => api.resume(id).await.map(drop),
                        "mirror.enableAudio" => api.enable_audio(id).await,
                        "mirror.keyboardSettings" => api.request_keyboard_settings(input_source).await,
                        _ => api.request_audio_permission(open_settings).await,
                    };
                    if let Err(e) = result {
                        eprintln!("{action} failed: {e}");
                    }
                    let _ = thread.queue(|bridge| bridge.bump(Revision::Mirroring));
                });
                Ok(())
            }
            other => {
                eprintln!("unknown action {other}");
                Ok(())
            }
        };
        if let Err(e) = result {
            eprintln!("{name} failed: {e}");
        }
    }
}
