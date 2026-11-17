//! What the photos and apps part keeps between calls, and the `ui` payload the QML reads of it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use phonelink_clipboard::pasteboard::ClipboardPasteboard;
use serde_json::{Value, json};

use super::action_file::Fetching;
use super::apps;
use super::selection::Selection;
use crate::apps_host::OpenWithApp;

/// What the photos and apps part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    pub(super) selection: Selection,
    /// The photo the viewer shows (the focus) while it is open.
    pub(super) viewer: Option<i64>,
    /// The grid's cells on screen: reported again when the gallery is entered after it was left.
    pub(super) visible: HashSet<i64>,
    pub(super) gallery_left: bool,
    /// How many times the gallery was left: an open that arrives after the user left is dropped.
    pub(super) departures: Arc<AtomicU64>,
    /// File actions, saves and delete pre-checks waiting on the phone, per photo.
    pub(super) busy: HashMap<i64, u32>,
    /// Photo permission requests under way; their buttons wait.
    pub(super) requests: u32,
    /// The full image of the photo whose save panel is open, on its way.
    pub(super) pending_save: Option<(i64, Fetching)>,
    /// Each listed photo's version (its modified time), to drop the files of photos that changed or left.
    pub(super) versions: HashMap<i64, i64>,
    /// The clipboard photos are copied to, connected at the first copy.
    pub(super) pasteboard: Option<Arc<dyn ClipboardPasteboard>>,
    /// The Open With applications per MIME type, while the gallery stays on screen.
    pub(super) open_with: RefCell<HashMap<String, Vec<OpenWithApp>>>,
    /// The cached icon files' generation: icons the phone changed get new file names, so the views
    /// load them again.
    pub(super) icon_generation: u64,
    pub(super) apps: RefCell<apps::Memory>,
}

impl State {
    pub(super) fn ui_json(&self) -> Value {
        let mut selected: Vec<i64> = self.selection.ids().collect();
        selected.sort_unstable();

        let mut busy: Vec<i64> = self.busy.keys().copied().collect();
        busy.sort_unstable();

        json!({
            "selected": selected,
            "focus": self.selection.focus(),
            "viewer": self.viewer,
            "busy": busy,
            "requestPending": self.requests > 0,
        })
    }
}
