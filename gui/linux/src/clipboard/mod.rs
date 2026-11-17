//! The system clipboard for the engine's clipboard sharing — read and written through a Wayland
//! data-control protocol where the compositor offers one, else through the X11 selection — and the
//! image codec the clipboard feature converts pictures with.

mod image_codec;
mod mimes;
mod wayland;
mod x11;

use std::sync::Arc;
use std::time::Duration;

pub use image_codec::ImageCrateCodec;
use phonelink_clipboard::pasteboard::ClipboardPasteboard;
use phonelink_engine::edge::EdgeError;
use wayland::{Ext, WaylandError, WaylandPasteboard, Wlr};
use x11::{X11Error, X11Pasteboard};

/// How long the clipboard's owner may take to answer a read, and to send each part of a long one.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Why no system clipboard was reached.
#[derive(Debug, thiserror::Error)]
pub enum SystemPasteboardError {
    /// Neither `WAYLAND_DISPLAY` nor `DISPLAY` names a display.
    #[error("no display")]
    NoDisplay,
    /// Every clipboard of the displays failed to connect, each for its reason.
    #[error("{}", joined(.0))]
    NotConnected(Vec<ConnectFailure>),
}

/// Why one clipboard did not connect.
#[derive(Debug, thiserror::Error)]
pub enum ConnectFailure {
    #[error("Wayland: {0}")]
    Wayland(#[source] WaylandError),
    #[error("{0}")]
    X11(#[source] X11Error),
}

/// The failures one after another, separated by `; `.
fn joined(failures: &[ConnectFailure]) -> String {
    failures.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

/// Wayland's data-control protocols (ext, else wlr) reach the clipboard without focus. A compositor with
/// neither, such as GNOME's Mutter, and an X11 session use the X11 selection (through `XWayland` on Wayland).
pub fn system_pasteboard() -> Result<Arc<dyn ClipboardPasteboard>, SystemPasteboardError> {
    let mut failures = Vec::new();

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match WaylandPasteboard::<Ext>::connect() {
            Ok(wayland) => return Ok(Arc::new(wayland)),
            Err(e) => failures.push(ConnectFailure::Wayland(e)),
        }
        match WaylandPasteboard::<Wlr>::connect() {
            Ok(wayland) => return Ok(Arc::new(wayland)),
            Err(e) => failures.push(ConnectFailure::Wayland(e)),
        }
    }

    if std::env::var_os("DISPLAY").is_some() {
        match X11Pasteboard::connect() {
            Ok(x11) => return Ok(Arc::new(x11)),
            Err(e) => failures.push(ConnectFailure::X11(e)),
        }
    }

    Err(if failures.is_empty() {
        SystemPasteboardError::NoDisplay
    } else {
        SystemPasteboardError::NotConnected(failures)
    })
}

/// A failure of the clipboard, as the engine's error.
fn edge_err(e: impl std::fmt::Display) -> EdgeError {
    EdgeError::Unexpected { message: format!("clipboard: {e}") }
}
