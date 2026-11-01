//! DIAGNOSTIC (temporary): drives mirroring sessions from scripts, so a session can be opened and fed
//! input without anyone at the window.
//!
//! With `PHONELINK_MIRROR_PROBE` naming a file, the file is read
//! every 500 ms and each new content runs as a script (the session id carries over between scripts).
//! Steps are separated by `;`:
//!
//! | Step | Does |
//! |---|---|
//! | `wait:<ms>` | sleeps |
//! | `open` / `launch:<package>` | opens the phone screen / an app; later steps act on it |
//! | `ready` | waits (≤ 90 s) for the window to be launched with video, then marks it ready and focused |
//! | `focus` | tells the session its window has focus |
//! | `skip_unlock` | answers the nearby prompt with "continue" |
//! | `auto_unlock:1` / `auto_unlock:0` | sets "continue without asking to skip unlocking" |
//! | `move:x,y` | moves the mouse (0–1 over the video) |
//! | `down:x,y` / `up:x,y` | presses / releases the left mouse button there |
//! | `tdown:x,y` / `tmove:x,y` / `tup:x,y` | one finger |
//! | `btn:<l\|m\|r>,<1\|0>,x,y` | presses (1) or releases (0) a mouse button there |
//! | `pen:<1\|0>,x,y` | the pen touches (1) or lifts (0) there |
//! | `key:<mac key code>` | a key press and release |
//! | `wheel:<dy>` / `hwheel:<dx>` | the vertical / horizontal wheel, in Windows units |
//! | `state` | logs the session |
//! | `close` | closes the session |
//!
//! Every step and its outcome is logged at `info` with the target `mirror_probe`.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::{FocusableState, KeyAction, MouseButton, ScreenMirroring, TouchPhase};

pub const VARIABLE: &str = "PHONELINK_MIRROR_PROBE";

/// Watches the file [`VARIABLE`] names, when it is set.
pub fn spawn_from_env(feature: Arc<ScreenMirroring>) {
    let Ok(path) = std::env::var(VARIABLE) else { return };
    info!(target: "mirror_probe", %path, "probe watching");
    let weak = Arc::downgrade(&feature);

    tokio::spawn(async move {
        let mut last = std::fs::read_to_string(&path).unwrap_or_default();
        let mut id = String::new();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let Some(feature) = weak.upgrade() else { return };
            let now = std::fs::read_to_string(&path).unwrap_or_default();
            if now != last && !now.trim().is_empty() {
                last = now.clone();
                info!(target: "mirror_probe", script = %now.trim(), "probe running");
                run(&feature, &now, &mut id).await;
            }
        }
    });
}

fn done(r: Result<(), super::MirroringError>) -> Result<String, String> {
    r.map(|_| String::new()).map_err(|e| e.to_string())
}

fn point(arg: &str) -> Option<(f64, f64)> {
    let (x, y) = arg.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

async fn run(feature: &ScreenMirroring, script: &str, id: &mut String) {
    for step in script.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, arg) = step.split_once(':').unwrap_or((step, ""));
        let outcome = run_step(feature, name, arg, id).await;
        match outcome {
            Ok(detail) => info!(target: "mirror_probe", raw_step = step, %detail, "probe step done"),
            Err(e) => warn!(target: "mirror_probe", raw_step = step, error = %e, "probe step failed"),
        }
    }
    info!(target: "mirror_probe", "probe finished");
}

/// Runs one step `name` with its argument `arg`; the detail to log, or why it failed.
async fn run_step(feature: &ScreenMirroring, name: &str, arg: &str, id: &mut String) -> Result<String, String> {
    match name {
        "wait" => {
            tokio::time::sleep(Duration::from_millis(arg.parse().unwrap_or(1000))).await;
            Ok(String::new())
        }
        "open" => feature.open_phone_screen().map(|s| s.clone_into(id)).map(|_| id.clone()).map_err(|e| e.to_string()),
        "launch" => {
            feature.launch_app(arg, None).map(|s| s.clone_into(id)).map(|_| id.clone()).map_err(|e| e.to_string())
        }
        "ready" => wait_ready(feature, id).await,
        "focus" => done(feature.focus_changed(id, FocusableState::InFocus, false)),
        "skip_unlock" => done(feature.nearby_skip_unlock()),
        "auto_unlock" => set_auto_unlock(feature, arg),
        "move" | "down" | "up" | "tdown" | "tmove" | "tup" => pointer_step(feature, name, arg, id),
        "btn" | "pen" => button_or_pen_step(feature, name, arg, id),
        "key" => match arg.parse::<u16>() {
            Ok(code) => {
                done(feature.key(id, code, KeyAction::Down, 0).and_then(|_| feature.key(id, code, KeyAction::Up, 0)))
            }
            Err(_) => Err(format!("bad key code {arg:?}")),
        },
        "wheel" => done(feature.mouse_wheel(id, 0.5, 0.5, 0, arg.parse().unwrap_or(-120))),
        "hwheel" => done(feature.mouse_wheel(id, 0.5, 0.5, arg.parse().unwrap_or(120), 0)),
        "state" => Ok(format!("{:?}", feature.session(id))),
        "close" => done(feature.close_session(id)),
        other => Err(format!("unknown step {other:?}")),
    }
}

/// The `ready` step: waits for the window to be launched with video, marking it ready, then focuses it.
async fn wait_ready(feature: &ScreenMirroring, id: &str) -> Result<String, String> {
    let mut ok = false;
    for _ in 0..180 {
        if let Some(s) = feature.session(id) {
            let _ = feature.window_ready(id);
            if s.launched && s.video_ready {
                ok = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let _ = feature.focus_changed(id, FocusableState::InFocus, false);
    if ok { Ok("video ready".into()) } else { Err("not ready in 90 s".into()) }
}

/// The `auto_unlock` step: sets "continue without asking to skip unlocking" from `arg`.
fn set_auto_unlock(feature: &ScreenMirroring, arg: &str) -> Result<String, String> {
    match feature.status() {
        Ok(status) => {
            let mut settings = status.settings;
            settings.do_not_ask_skip_unlock = arg == "1";
            done(feature.set_settings(settings))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// The mouse-move, left-button and one-finger steps, at the point `arg`.
fn pointer_step(feature: &ScreenMirroring, name: &str, arg: &str, id: &str) -> Result<String, String> {
    match point(arg) {
        None => Err(format!("bad point {arg:?}")),
        Some((x, y)) => done(match name {
            "move" => feature.mouse_move(id, x, y),
            "down" => feature.mouse_button(id, MouseButton::Left, true, x, y),
            "up" => feature.mouse_button(id, MouseButton::Left, false, x, y),
            other => {
                let phase = match other {
                    "tdown" => TouchPhase::Down,
                    "tmove" => TouchPhase::Moved,
                    _ => TouchPhase::Up,
                };
                feature.touch(id, 1, phase, x, y, 0.5, 0.0, 4.0, 4.0)
            }
        }),
    }
}

/// The `btn` and `pen` steps, from their comma-separated arguments `arg`.
fn button_or_pen_step(feature: &ScreenMirroring, name: &str, arg: &str, id: &str) -> Result<String, String> {
    let parts: Vec<&str> = arg.split(',').collect();
    let num = |i: usize| parts.get(i).and_then(|v| v.trim().parse::<f64>().ok());
    match (name, parts.as_slice()) {
        ("btn", [b, d, ..]) if parts.len() == 4 => {
            let button = match *b {
                "m" => MouseButton::Middle,
                "r" => MouseButton::Right,
                _ => MouseButton::Left,
            };
            match (num(2), num(3)) {
                (Some(x), Some(y)) => done(feature.mouse_button(id, button, *d == "1", x, y)),
                _ => Err(format!("bad point {arg:?}")),
            }
        }
        ("pen", [d, ..]) if parts.len() == 3 => match (num(1), num(2)) {
            (Some(x), Some(y)) => done(feature.pen(id, x, y, 0.5, 0.0, true, *d == "1")),
            _ => Err(format!("bad point {arg:?}")),
        },
        _ => Err(format!("bad arguments {arg:?}")),
    }
}
