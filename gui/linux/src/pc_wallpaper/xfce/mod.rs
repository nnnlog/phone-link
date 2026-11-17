//! Xfce's picture: xfdesktop's `last-image` of the primary monitor in the workspace that shows, from
//! xfconf.

use std::collections::HashMap;
use std::process::Command;

use phonelink_engine::edge::EdgeError;
use x11rb::connection::Connection as _;
use x11rb::errors::{ConnectError, ConnectionError, ReplyError};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

use super::Setting;
use crate::edges::unexpected;

#[cfg(test)]
mod tests;

/// Why the current workspace was not read from the window manager.
#[derive(Debug, thiserror::Error)]
enum WorkspaceError {
    #[error("{0}")]
    Connect(#[from] ConnectError),
    #[error("no X11 screen")]
    NoScreen,
    #[error("{0}")]
    Connection(#[from] ConnectionError),
    #[error("{0}")]
    Reply(#[from] ReplyError),
    #[error("the window manager names no workspace")]
    NoWorkspace,
}

/// An xfconf channel's properties and values, from `xfconf-query -c <channel> -lv`.
fn xfconf(channel: &str) -> Result<HashMap<String, String>, EdgeError> {
    let output = Command::new("xfconf-query")
        .args(["-c", channel, "-l", "-v"])
        .output()
        .map_err(|e| unexpected(format!("xfconf-query did not run: {e}")))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(unexpected(format!("xfconf-query -c {channel}: {}", error.trim())));
    }
    Ok(xfconf_properties(&String::from_utf8_lossy(&output.stdout)))
}

/// `xfconf-query -lv` text: each line a property, spaces, and its value.
fn xfconf_properties(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (property, value) = line.split_once(char::is_whitespace)?;
            property.starts_with('/').then(|| (property.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// The workspace whose picture shows: the one picture of every workspace in single-workspace mode
/// (xfdesktop's default), else the current workspace, which the window manager tells.
fn xfce_workspace(desktop: &HashMap<String, String>) -> Result<u32, EdgeError> {
    let single = desktop.get("/backdrop/single-workspace-mode").is_none_or(|mode| mode == "true");
    if single {
        return Ok(desktop.get("/backdrop/single-workspace-number").and_then(|n| n.parse().ok()).unwrap_or(0));
    }
    current_workspace().map_err(|e| unexpected(format!("the current workspace is unknown: {e}")))
}

/// `_NET_CURRENT_DESKTOP` of the X11 root window.
fn current_workspace() -> Result<u32, WorkspaceError> {
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots.get(screen).ok_or(WorkspaceError::NoScreen)?.root;
    let atom = connection.intern_atom(true, b"_NET_CURRENT_DESKTOP")?.reply()?.atom;
    let reply = connection.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1)?.reply()?;
    reply.value32().and_then(|mut values| values.next()).ok_or(WorkspaceError::NoWorkspace)
}

/// The monitors with a picture of `workspace` in xfdesktop's properties, by name.
fn xfce_monitors(desktop: &HashMap<String, String>, workspace: u32) -> Vec<String> {
    let suffix = format!("/workspace{workspace}/last-image");
    let mut monitors: Vec<String> = desktop
        .keys()
        .filter_map(|property| property.strip_prefix("/backdrop/screen0/monitor")?.strip_suffix(&suffix))
        .map(str::to_owned)
        .collect();
    monitors.sort();
    monitors
}

/// The primary output in the display settings' active profile (`/<profile>/<output>/Primary`).
fn xfce_primary(displays: &HashMap<String, String>) -> Option<String> {
    let profile = displays.get("/ActiveProfile").map_or("Default", String::as_str);
    let prefix = format!("/{profile}/");
    displays.iter().find_map(|(property, value)| {
        let output = property.strip_prefix(&prefix)?.strip_suffix("/Primary")?;
        (value == "true" && !output.contains('/')).then(|| output.to_owned())
    })
}

/// Xfce's picture of the primary monitor in the workspace that shows: its `last-image`, none while its
/// `image-style` is 0 (no picture).
pub(super) fn xfce_setting() -> Result<Setting, EdgeError> {
    let desktop = xfconf("xfce4-desktop")?;
    let workspace = xfce_workspace(&desktop)?;
    let monitors = xfce_monitors(&desktop, workspace);
    let monitor = match monitors.as_slice() {
        [] => return Ok(Setting::NoPicture),
        [only] => only.clone(),
        several => {
            let primary = xfce_primary(&xfconf("displays")?)
                .ok_or_else(|| unexpected("Xfce has pictures for several monitors and none is the primary one"))?;
            several
                .iter()
                .find(|monitor| **monitor == primary)
                .cloned()
                .ok_or_else(|| unexpected(format!("Xfce has no picture for the primary monitor {primary}")))?
        }
    };

    let base = format!("/backdrop/screen0/monitor{monitor}/workspace{workspace}");
    if desktop.get(&format!("{base}/image-style")).is_some_and(|style| style == "0") {
        return Ok(Setting::NoPicture);
    }
    Ok(Setting::from_path(desktop.get(&format!("{base}/last-image")).map_or("", String::as_str)))
}
