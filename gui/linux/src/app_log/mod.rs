//! The app's log: the engine's log files, in `$XDG_STATE_HOME/phonelink` (`~/.local/state/phonelink`),
//! installed at launch. The engine's subscriber then records every `tracing` line of the process (the
//! engine's, this app's and its libraries'), writing each field as its redaction policy says;
//! Settings › Advanced shows its size, saves a copy and clears it. When the log cannot be installed,
//! the lines go to standard error instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use phonelink_engine::api::logging::{LogLevel, LogSegment, install_log, log_message};
use tracing::warn;
use tracing_subscriber::EnvFilter;

static INSTALLED: AtomicBool = AtomicBool::new(false);

/// The log's folder: `$XDG_STATE_HOME/phonelink`, else `~/.local/state/phonelink`; `None` without
/// either variable.
fn directory() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .map(|dir| dir.join("phonelink"))
}

/// Lines to standard error, at `info` unless `RUST_LOG` says otherwise.
fn log_to_stderr() {
    // The engine's install may have set the process's subscriber before it failed; that one stays.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .try_init();
}

/// Installs the log; the first thing the app does, so no line is lost.
pub fn install() {
    let Some(dir) = directory() else {
        log_to_stderr();
        warn!("no folder for the log: neither XDG_STATE_HOME nor HOME is set; lines go to standard error");
        return;
    };

    match install_log(dir.to_string_lossy().into_owned()) {
        Ok(()) => INSTALLED.store(true, Ordering::SeqCst),
        Err(e) => {
            log_to_stderr();
            warn!(error = %e, "the log file was not opened; lines go to standard error");
        }
    }
}

/// The log file is installed.
pub fn installed() -> bool {
    INSTALLED.load(Ordering::SeqCst)
}

/// The first line of a launch: the app's version and the system's, so each launch can be told apart
/// in a saved log.
pub fn launched() {
    let system = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .ok()
        .and_then(|text| pretty_name(&text))
        .unwrap_or_else(|| "Linux".to_owned());
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok().filter(|desktop| !desktop.is_empty());
    let text = launch_line(env!("CARGO_PKG_VERSION"), &system, desktop.as_deref());
    log_message(LogLevel::Info, "app".to_owned(), vec![LogSegment { text, is_public: true }]);
}

fn launch_line(version: &str, system: &str, desktop: Option<&str>) -> String {
    match desktop {
        Some(desktop) => format!("app launched: version {version} on {system} ({desktop})"),
        None => format!("app launched: version {version} on {system}"),
    }
}

/// The `PRETTY_NAME` of an `os-release` file.
fn pretty_name(os_release: &str) -> Option<String> {
    os_release
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests;
