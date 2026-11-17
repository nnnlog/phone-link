//! What the notifications part keeps between calls, and the bridge's access to it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;

use cxx_qt::CxxQtType;
use phonelink_engine::api::notifications::{
    AppNotificationFlags, AppNotificationSetting, NotificationsState, PhoneNotification,
};

use crate::engine_bridge::EngineBridge;
use crate::views::notifications::Content;

/// A request the settings pane sends to the phone; its row waits while one is under way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum PaneRequest {
    Access,
    SensitiveContent,
}

/// The app last turned off from a row's menu, which Undo turns back on with the flags it had. It
/// names one phone's app, so Undo applies only while that phone is selected.
pub(super) struct TurnedOff {
    pub(super) phone: String,
    pub(super) package: String,
    pub(super) name: String,
    pub(super) previous: AppNotificationFlags,
}

/// What the notifications part keeps between calls.
#[derive(Default)]
pub(in crate::engine_bridge) struct State {
    /// The engine's state and list as last read, and why they could not be read, until a read succeeds.
    pub(super) state: Option<NotificationsState>,
    pub(super) list: Vec<PhoneNotification>,
    pub(super) state_failure: Option<String>,
    pub(super) list_failure: Option<String>,
    /// The listed apps' settings, by package: a row's menu offers turning the app on or off and muting
    /// it from these.
    pub(super) app_settings: HashMap<String, AppNotificationSetting>,
    /// Actions and replies queued from this window that have not finished, by (key, reply). A finished
    /// one queued elsewhere (a desktop notification's, whose failure the engine announces itself, or one
    /// from an earlier run) is not the user's action here.
    pub(super) pending_actions: HashMap<(String, bool), u32>,
    pub(super) clearing: bool,
    /// The list column's own access request runs; once granted, it turns this computer's switch on.
    pub(super) requesting_access: bool,
    pub(super) pane_requests: HashSet<PaneRequest>,
    /// Apps whose change in the settings pane waits for the phone, with the flags the user chose.
    pub(super) pending_apps: HashMap<String, AppNotificationFlags>,
    /// Every app the list has shown, by phone id, package → name: an app set to Off leaves the list,
    /// yet stays in the settings pane to be turned on again.
    pub(super) known_apps: BTreeMap<String, BTreeMap<String, String>>,
    /// The window gave back the apps it keeps between runs; until then nothing is handed to it, so a
    /// partial set never replaces them.
    pub(super) known_apps_restored: bool,
    pub(super) turned_off: Option<TurnedOff>,
}

impl State {
    pub(super) fn failure(&self) -> Option<&str> {
        self.state_failure.as_deref().or(self.list_failure.as_deref())
    }

    pub(super) fn content(&self) -> Content {
        Content::of(self.state.as_ref(), self.failure().is_some(), self.list.is_empty())
    }
}

impl EngineBridge {
    pub(super) fn notifications_mut(self: Pin<&mut Self>) -> &mut State {
        &mut self.rust_mut().get_mut().areas.notifications
    }
}
