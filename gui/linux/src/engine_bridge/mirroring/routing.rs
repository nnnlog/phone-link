//! The routing of `mirror.*` names: the queries, the actions by their kind, and the display values
//! the windows have read.

use std::pin::Pin;
use std::sync::Arc;

use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use serde_json::Value;
use tracing::{debug, warn};

use super::request::MirrorRequest;
use super::session_actions::session_action;
use crate::engine_bridge::features::Revision;
use crate::engine_bridge::{EngineBridge, qobject};
use crate::mirror::{presenter, sizing, views};

/// Gives mirroring the primary screen's values: its available area in device pixels less a framed
/// window's title bar, its dpi and its size.
///
/// The presenter gets `primary_display_geometry` as its reader; the phone's configuration calls it on
/// the engine's thread, where it returns what was last read on the GUI thread. This function, which the
/// windows call on the GUI thread as they open and once they are framed, does that read; from then on
/// desktop.h reads the screen again whenever the primary screen changes. Without a screen the last
/// values stay.
pub(in crate::engine_bridge) fn set_display() {
    presenter().read_screen_with(qobject::primary_display_geometry);
    // The read on the GUI thread, which the reader's calls on other threads return.
    let read = qobject::primary_display_geometry();
    match crate::mirror::display_geometry(&read) {
        Some(geometry) => {
            debug!(width = geometry.usable_width, height = geometry.usable_height, "the primary screen was read")
        }
        None => warn!("no primary screen; mirroring keeps the last display values"),
    }
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_mirror(&self, engine: &Arc<Engine>, name: &str, arg: &str) -> Option<Value> {
        match name {
            "mirror.status" => Some(views::status(engine)),
            "mirror.sessions" => Some(views::sessions(engine)),
            "mirror.session" => Some(views::one_session(engine, arg)),
            "mirror.frame" => {
                let request: Value = serde_json::from_str(arg).unwrap_or(Value::Null);
                Some(sizing::answer(&request).unwrap_or_else(|e| {
                    warn!(error = %e, "a mirroring window's frame was not computed");
                    Value::Null
                }))
            }
            _ => None,
        }
    }

    /// Runs a `mirror.*` action; false for any other name.
    pub(in crate::engine_bridge) fn perform_mirror(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        if !name.starts_with("mirror.") {
            return false;
        }

        let api = engine.screen_mirroring();
        let request = MirrorRequest::parse(arg);
        let handled = session_action(&api, name, &request)
            .or_else(|| self.as_mut().perform_mirror_input(&api, name, &request))
            .or_else(|| self.as_mut().perform_mirror_drag(&api, name, &request))
            .or_else(|| self.as_mut().perform_mirror_deferred(&api, name, &request));
        let result = match handled {
            Some(result) => result,
            None => Err(FeatureApiError::InvalidInput { reason: format!("unknown action {name}") }),
        };

        if let Err(e) = result {
            self.as_mut().report_mirror_failure(name, &e);
        }
        // The settings read again show what the phone keeps, changed or not.
        if name == "mirror.setting" {
            self.bump(Revision::Mirroring);
        }
        true
    }
}
