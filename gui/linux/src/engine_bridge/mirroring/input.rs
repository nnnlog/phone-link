//! A mirrored window's input: its focus and Caps Lock, the navigation buttons, the mouse and wheel,
//! keys, touch and pen, and a paused app resumed when its window becomes the active one.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::screen_mirroring::{
    MirrorFocus, MirrorKeyAction, MirrorMouseButton, MirrorNavButton, MirrorTouchPhase, MirrorWindowState,
    ScreenMirroringApi,
};
use tracing::{debug, info};

use super::request::MirrorRequest;
use crate::engine_bridge::EngineBridge;

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

impl EngineBridge {
    /// Runs a `mirror.*` input action; `None` for any other name.
    pub(super) fn perform_mirror_input(
        mut self: Pin<&mut Self>,
        api: &Arc<ScreenMirroringApi>,
        name: &str,
        request: &MirrorRequest<'_>,
    ) -> Option<Result<(), FeatureApiError>> {
        let (id, x, y) = (request.id.clone(), request.x, request.y);
        let result = match name {
            "mirror.focus" => {
                let focused = request.json["focus"].as_str() == Some("in");
                let caps_lock = self.as_mut().caps_lock_now();
                let focus = if focused { MirrorFocus::InFocus } else { MirrorFocus::NotInFocus };
                let result = api.focus_changed(id.clone(), focus, caps_lock);
                if focused {
                    self.as_mut().resume_if_paused(api.clone(), id);
                }
                result
            }
            "mirror.inputFocus" => {
                let focused = request.flag("focused");
                if !focused {
                    self.as_mut().rust_mut().mirror_keyboard.release_all();
                }
                let caps_lock = self.as_mut().caps_lock_now();
                api.input_focus(id, focused, caps_lock)
            }
            "mirror.nav" => {
                let button = match request.json["button"].as_str() {
                    Some("home") => MirrorNavButton::Home,
                    Some("recents") => MirrorNavButton::Recents,
                    _ => MirrorNavButton::Back,
                };
                api.nav_button(id, button)
            }
            "mirror.mouse" => match request.json["button"].as_str() {
                Some(button) => api.mouse_button(id, mouse_button(button), request.flag("down"), x, y),
                None => api.mouse_move(id, x, y),
            },
            "mirror.wheel" => {
                let delta = |key: &str| request.number(key).round() as i32;
                api.mouse_wheel(id, x, y, delta("dx"), delta("dy"))
            }
            "mirror.key" => {
                let scan = request.json["scan"].as_u64().and_then(|v| u32::try_from(v).ok()).unwrap_or_default();
                let down = request.flag("down");
                match self.as_mut().rust_mut().mirror_keyboard.event(scan, down) {
                    Some((key, flags)) => {
                        let action = match (down, request.flag("repeat")) {
                            (false, _) => MirrorKeyAction::Up,
                            (true, true) => MirrorKeyAction::Repeat,
                            (true, false) => MirrorKeyAction::Down,
                        };
                        api.key(id, key, action, flags)
                    }
                    None => Ok(()),
                }
            }
            "mirror.touch" => {
                let pointer = request.json["pointer"].as_u64().unwrap_or_default();
                let phase = touch_phase(request.json["phase"].as_str().unwrap_or_default());
                api.touch(
                    id,
                    pointer,
                    phase,
                    x,
                    y,
                    request.number("pressure"),
                    request.number("orientation"),
                    request.number("width"),
                    request.number("height"),
                )
            }
            "mirror.pen" => api.pen(
                id,
                x,
                y,
                request.number("pressure"),
                request.number("rotation"),
                request.flag("inRange"),
                request.flag("inContact"),
            ),
            _ => return None,
        };
        Some(result)
    }

    /// The Caps Lock state now: the keyboards' Caps Lock light when the computer has one, else what the
    /// windows saw of the key.
    fn caps_lock_now(mut self: Pin<&mut Self>) -> bool {
        if let Some(lit) = crate::mirror::caps_lock_lit() {
            self.as_mut().rust_mut().mirror_keyboard.set_caps_lock(lit);
        }
        self.mirror_keyboard.caps_lock()
    }

    /// The window of `id` became the active window: a paused app opens again when the phone can resume
    /// it, and nothing is shown when it cannot.
    fn resume_if_paused(self: Pin<&mut Self>, api: Arc<ScreenMirroringApi>, id: String) {
        let paused = api.session(id.clone()).ok().flatten().is_some_and(|s| s.state == MirrorWindowState::Suspended);
        if !paused {
            return;
        }

        self.runtime.spawn(async move {
            match api.resume(id).await {
                Ok(resumed) => debug!(resumed, "a paused mirroring window was activated"),
                Err(e) => info!(error = %e, "a paused mirroring window was not resumed; it stays paused"),
            }
        });
    }
}
