//! The kind of the computer's current keyboard input source — a keyboard layout or an input method —
//! as the input method framework that runs says: Fcitx 5's current input method (`keyboard-*` is a
//! layout), else `IBus`'s global engine (`xkb:*` is a layout). With no framework running it is a layout;
//! a framework that runs and does not say is logged, and a layout is assumed.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use phonelink_engine::api::screen_mirroring::MirrorInputSource;
use tracing::{debug, error, warn};
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, Proxy};
use zbus::names::BusName;

const FCITX5: &str = "org.fcitx.Fcitx5";
/// The process `IBus` runs as.
const IBUS_DAEMON: &str = "ibus-daemon";
/// How long the frameworks have to answer.
const ANSWER_WITHIN: Duration = Duration::from_secs(3);

/// The input method frameworks asked, in this order: Fcitx 5 is only running when the user chose it,
/// while some desktops run `IBus` whatever the user types with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Framework {
    Fcitx5,
    IBus,
}

/// What a framework said.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    NotRunning,
    /// The name of its current input source.
    Source(String),
    /// It runs and did not say: why.
    Silent(String),
}

/// The kind of the input source `framework` names `name`.
fn kind(framework: Framework, name: &str) -> MirrorInputSource {
    let layout = match framework {
        Framework::Fcitx5 => name.starts_with("keyboard-"),
        Framework::IBus => name.starts_with("xkb:"),
    };
    if layout { MirrorInputSource::Layout } else { MirrorInputSource::InputMethod }
}

/// Fcitx 5's current input method, when it is on the session bus.
fn ask_fcitx5() -> Answer {
    let bus = match Connection::session() {
        Ok(bus) => bus,
        Err(e) => {
            debug!(error = %e, "no session bus; Fcitx 5 is not asked");
            return Answer::NotRunning;
        }
    };

    let running = BusName::try_from(FCITX5)
        .map_err(zbus::Error::from)
        .and_then(|name| DBusProxy::new(&bus)?.name_has_owner(name).map_err(zbus::Error::from));
    match running {
        Ok(true) => {}
        Ok(false) => return Answer::NotRunning,
        Err(e) => {
            debug!(error = %e, "the session bus did not say whether Fcitx 5 runs; it is not asked");
            return Answer::NotRunning;
        }
    }

    let current = Proxy::new(&bus, FCITX5, "/controller", "org.fcitx.Fcitx.Controller1")
        .and_then(|controller| controller.call::<_, _, String>("CurrentInputMethod", &()));
    match current {
        Ok(name) if !name.is_empty() => Answer::Source(name),
        Ok(_) => Answer::Silent("Fcitx 5 named no current input method".to_owned()),
        Err(e) => Answer::Silent(format!("Fcitx 5: {e}")),
    }
}

/// Whether one of this user's processes under `proc` (laid out like `/proc`) runs `command`.
fn runs(proc: &Path, command: &str, uid: u32) -> bool {
    let Ok(entries) = fs::read_dir(proc) else { return false };
    entries.flatten().any(|entry| {
        let process = entry.file_name().to_str().is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()));
        process
            && entry.metadata().is_ok_and(|metadata| metadata.uid() == uid)
            && fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim_end() == command)
    })
}

/// `IBus`'s global engine, when this user's `IBus` daemon runs.
fn ask_ibus() -> Answer {
    let own = match fs::metadata("/proc/self") {
        Ok(metadata) => metadata.uid(),
        Err(e) => return Answer::Silent(format!("this process's user is not known: {e}")),
    };
    if !runs(Path::new("/proc"), IBUS_DAEMON, own) {
        return Answer::NotRunning;
    }

    let output = match Command::new("ibus").arg("engine").env("LC_ALL", "C").output() {
        Ok(output) => output,
        Err(e) => return Answer::Silent(format!("ibus did not run: {e}")),
    };
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || name.is_empty() {
        return Answer::Silent(format!("ibus engine: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Answer::Source(name)
}

/// The kind of the current input source; blocks on D-Bus and on `ibus`.
fn read() -> MirrorInputSource {
    for framework in [Framework::Fcitx5, Framework::IBus] {
        let answer = match framework {
            Framework::Fcitx5 => ask_fcitx5(),
            Framework::IBus => ask_ibus(),
        };
        match answer {
            Answer::NotRunning => {}
            Answer::Source(name) => {
                let source = kind(framework, &name);
                debug!(framework_kind = ?framework, input_source_kind = ?source, "the input method framework said its input source");
                return source;
            }
            Answer::Silent(why) => {
                warn!(framework_kind = ?framework, error = %why, "the input method framework did not say its input source; a keyboard layout is assumed");
                return MirrorInputSource::Layout;
            }
        }
    }
    debug!("no input method framework runs; the input source is a keyboard layout");
    MirrorInputSource::Layout
}

/// The kind of the computer's current keyboard input source, which the phone's keyboard settings follow.
pub async fn keyboard_input_source() -> MirrorInputSource {
    match tokio::time::timeout(ANSWER_WITHIN, tokio::task::spawn_blocking(read)).await {
        Ok(Ok(source)) => source,
        Ok(Err(e)) => {
            error!(error = %e, "the input source was not read; a keyboard layout is assumed");
            MirrorInputSource::Layout
        }
        Err(_) => {
            warn!("the input method framework did not answer in time; a keyboard layout is assumed");
            MirrorInputSource::Layout
        }
    }
}

#[cfg(test)]
mod tests;
