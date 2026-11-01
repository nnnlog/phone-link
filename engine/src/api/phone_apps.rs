//! The phone's apps for the UI ([`crate::features::phone_apps`]).
//!
//! It covers the gates the Apps view reads, the syncing state, the lists (all apps, search,
//! favorites, recent apps), favorites, and app icons — which the notification list shows as well.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::phone_apps::{PhoneApp, PhoneApps, PhoneAppsError, PhoneAppsEvent, PhoneAppsStatus, RecentApp};

use super::{FeatureApiError, core_of, features_of};

impl From<PhoneAppsError> for FeatureApiError {
    fn from(e: PhoneAppsError) -> Self {
        match e {
            PhoneAppsError::NoPhone => FeatureApiError::NoPhone,
            PhoneAppsError::AppGone => FeatureApiError::NotStarted,
            PhoneAppsError::UnknownApp(_) => FeatureApiError::UnknownItem { what: "app".into() },
            PhoneAppsError::InvalidInput(reason) => FeatureApiError::InvalidInput { reason: reason.into() },
            PhoneAppsError::Store(e) => e.into(),
            PhoneAppsError::Edge(reason) => FeatureApiError::Internal { message: reason.into() },
        }
    }
}

/// What the Apps view reads.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneAppsState {
    /// The Apps view shows: the phone shows its screen on the Mac and runs apps for it.
    pub apps_view_visible: bool,
    /// The phone sends its recent apps: the Recent section shows, empty or not.
    pub recent_apps_supported: bool,
    /// The phone hands documents over: clicking a recent task that has a document opens the document
    /// on this Mac (or in the browser) instead of the app on the phone.
    pub app_handoff_enabled: bool,
    /// A launch opens each app in a window of its own (else in the one phone-screen window).
    pub hosted_apps: bool,
    /// The phone's lists are behind and being updated (at most 2 minutes): until this first turns
    /// false the Apps section shows no lists, then it follows every change.
    pub syncing: bool,
    /// The lists are up to date with the phone.
    pub up_to_date: bool,
    /// The app list's last completed sync, Unix ms.
    pub last_sync_ms: Option<i64>,
}

impl From<&PhoneAppsStatus> for PhoneAppsState {
    fn from(s: &PhoneAppsStatus) -> Self {
        Self {
            apps_view_visible: s.apps_view_visible,
            recent_apps_supported: s.recent_apps_supported,
            app_handoff_enabled: s.app_handoff_enabled,
            hosted_apps: s.hosted_apps,
            syncing: s.syncing,
            up_to_date: s.sync.up_to_date(),
            last_sync_ms: s.sync.last_sync_ms,
        }
    }
}

/// One phone app (its icon: `PhoneAppsApi::icon`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneAppItem {
    pub package_name: String,
    pub name: String,
    pub version: String,
    pub has_icon: bool,
    pub favorite: bool,
}

impl From<PhoneApp> for PhoneAppItem {
    fn from(a: PhoneApp) -> Self {
        Self {
            package_name: a.package_name,
            name: a.name,
            version: a.version,
            has_icon: a.has_icon,
            favorite: a.favorite,
        }
    }
}

/// One recent app; its icon is its app's (`PhoneAppsApi::icon` of `package_name`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RecentAppItem {
    pub recent_id: i64,
    pub package_name: String,
    pub name: String,
    /// Launching it opens the app; otherwise it is a task, launched with `task_id`, and it may offer
    /// its document on this Mac (`desktop_uri`) or in the browser (`web_uri`).
    pub main_activity: bool,
    /// A main-activity entry whose app is a favorite; it can be added to or removed from the
    /// favorites. A task entry is never a favorite and offers neither.
    pub favorite: bool,
    pub task_id: i64,
    pub web_uri: Option<String>,
    pub desktop_uri: Option<String>,
    pub has_icon: bool,
}

impl From<RecentApp> for RecentAppItem {
    fn from(r: RecentApp) -> Self {
        Self {
            recent_id: r.recent_id,
            package_name: r.package_name,
            name: r.name,
            main_activity: r.main_activity,
            favorite: r.favorite,
            task_id: r.task_id,
            web_uri: r.web_uri,
            desktop_uri: r.desktop_uri,
            has_icon: r.has_icon,
        }
    }
}

/// A change to the selected phone's apps: re-read the app lists, the recent apps, or the state.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
#[expect(clippy::enum_variant_names, reason = "each change names what changed")]
pub enum PhoneAppsChange {
    /// Apps, icons or favorites changed: re-read the app lists (and drop cached icons).
    AppsChanged,
    /// The recent apps changed.
    RecentChanged,
    /// The state changed: re-read `state` and the lists.
    StateChanged,
}

impl From<&PhoneAppsEvent> for PhoneAppsChange {
    fn from(e: &PhoneAppsEvent) -> Self {
        match e {
            PhoneAppsEvent::AppsChanged { .. } => PhoneAppsChange::AppsChanged,
            PhoneAppsEvent::RecentChanged { .. } => PhoneAppsChange::RecentChanged,
            PhoneAppsEvent::StatusChanged { .. } => PhoneAppsChange::StateChanged,
        }
    }
}

/// The phone-apps feature.
#[derive(uniffi::Object)]
pub struct PhoneAppsApi {
    engine: Arc<Engine>,
}

impl PhoneAppsApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(&self, f: impl FnOnce(&PhoneApps) -> Result<T, PhoneAppsError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.phone_apps.clone();
        Ok(f(&feature)?)
    }
}

#[uniffi::export]
impl PhoneAppsApi {
    /// What the Apps view shows for the selected phone: its sections, launch mode and sync state.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<PhoneAppsState, FeatureApiError> {
        self.run(|f| f.status()).map(|s| (&s).into())
    }

    /// Every app the phone lists (but Link to Windows itself), in the order of the user's language.
    pub fn apps(&self) -> Result<Vec<PhoneAppItem>, FeatureApiError> {
        self.run(|f| f.apps()).map(|v| v.into_iter().map(Into::into).collect())
    }

    /// The apps whose name holds every space-separated term of `query`, ignoring case.
    pub fn search(&self, query: String) -> Result<Vec<PhoneAppItem>, FeatureApiError> {
        self.run(|f| f.search(&query)).map(|v| v.into_iter().map(Into::into).collect())
    }

    /// The favorites, in their order.
    pub fn favorites(&self) -> Result<Vec<PhoneAppItem>, FeatureApiError> {
        self.run(|f| f.favorites()).map(|v| v.into_iter().map(Into::into).collect())
    }

    /// The recent apps (at most 3), most recent first.
    pub fn recent(&self) -> Result<Vec<RecentAppItem>, FeatureApiError> {
        self.run(|f| f.recent()).map(|v| v.into_iter().map(Into::into).collect())
    }

    /// The icon bytes of `package_name`'s app as the phone sent them (PNG); `None` when it has none.
    pub fn icon(&self, package_name: String) -> Result<Option<Vec<u8>>, FeatureApiError> {
        self.run(|f| f.icon(&package_name))
    }

    /// The name the phone sent for `package_name`, as stored, excluded packages included: the title of
    /// a mirroring window showing that app.
    pub fn stored_name(&self, package_name: String) -> Result<Option<String>, FeatureApiError> {
        self.run(|f| f.stored_name(&package_name))
    }

    /// Adds the app `package_name` to the end of the favorites (this Mac only).
    ///
    /// Fails with `UnknownItem` when the phone did not list that app.
    pub fn add_favorite(&self, package_name: String) -> Result<(), FeatureApiError> {
        self.run(|f| f.add_favorite(&package_name))
    }

    /// Removes the app `package_name` from the favorites (this Mac only).
    ///
    /// Fails with `UnknownItem` when the phone did not list that app.
    pub fn remove_favorite(&self, package_name: String) -> Result<(), FeatureApiError> {
        self.run(|f| f.remove_favorite(&package_name))
    }

    /// Puts the favorites in this order (each package once; one left out keeps its place by rank).
    pub fn reorder_favorites(&self, package_names: Vec<String>) -> Result<(), FeatureApiError> {
        self.run(|f| f.reorder_favorites(&package_names))
    }
}
