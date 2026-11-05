//! Phone apps — the phone's launchable apps and its recent apps on the Mac, Android only.
//!
//! | Module | Implements |
//! |---|---|
//! | [`gates`] | the gates over the phone record (recent apps, app handoff, phone-screen apps, remoting, hosted apps, the Apps view), shared with the features that launch phone apps |
//! | [`wire`] | the `phoneApps` messages: keys, parsing and the malformed-content rules, replies, the desktop-URI rule |
//! | [`store`] | the per-phone app table, recent table, sequence pairs and sync time |
//! | [`present`] | the lists the UI shows: all apps, search, favorites, recent apps |
//! | [`edge`] | the OS edge: which URIs an installed application opens, the order of names in the user's language |
//! | `sync` | the `phoneApps` processor: metadata, phone-app items, recent items, completion |
//! | `connect` | the contribution to the connect message |
//! | this module | the "syncing" state, favorites, the record-derived status, the UI-facing API, and the app names and icons other features read ([`super::PhoneAppDirectory`]) |
//!
//! Both lists are pushed by the phone: the PC answers a metadata message with the ids it lacks and
//! stores the item messages that follow; it never asks for them. Launching an app is not a message
//! of this feature: every launch is handed to the screen-mirroring session.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod edge;
pub mod present;
pub mod store;
pub mod wire;

mod connect;
mod sync;

use phonelink_core::config::features::PhoneAppsConfig;
use phonelink_features::shared::launch_gates;
use phonelink_features::shared::ongoing::OngoingFailure;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use phonelink_app::{AppEvent, AppLayer, ConnectProvider, PhoneRecord};
use phonelink_core::config::AppConfig;
use phonelink_core::edge::Clock;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError};
use phonelink_features::{FeatureEnv, FeatureError, PhoneAppDirectory, PhoneAppInfo};

pub use edge::{PhoneAppsHost, PlainAppsHost};
use phonelink_core::sync::MutexExt;
pub use present::RecentView;
pub use store::SyncState;

fn validate_config(config: &PhoneAppsConfig) -> Result<(), FeatureError> {
    if config.recent_apps_max == 0 {
        return Err(FeatureError::Config("the recent list must show at least one entry".into()));
    }
    Ok(())
}

/// What the UI is told to re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::enum_variant_names, reason = "each event names what changed")]
pub enum PhoneAppsEvent {
    /// Apps, their icons or favorites changed: re-read the app lists.
    AppsChanged { phone: DcgClient },
    /// The recent entries changed: re-read [`PhoneApps::recent`].
    RecentChanged { phone: DcgClient },
    /// The gates or the syncing state changed: re-read [`PhoneApps::status`] and the lists.
    StatusChanged { phone: DcgClient },
}

/// Why a UI call could not run.
#[derive(Debug, thiserror::Error)]
pub enum PhoneAppsError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("the App layer is gone")]
    AppGone,
    #[error("no app of package {0:?} is stored")]
    UnknownApp(String),
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    /// The OS edge broke its contract.
    #[error("edge: {0}")]
    Edge(&'static str),
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// One app of a list (its icon: [`PhoneApps::icon`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneApp {
    pub package_name: String,
    /// The shown name (a line break as a space).
    pub name: String,
    pub version: String,
    pub has_icon: bool,
    pub favorite: bool,
}

impl PhoneApp {
    fn of(row: &store::AppRow) -> Self {
        Self {
            package_name: row.package_name.clone(),
            name: present::shown_name(&row.name),
            version: row.version.clone(),
            has_icon: row.has_icon,
            favorite: row.favorite_rank >= 0,
        }
    }
}

/// One recent app (its icon is its app's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentApp {
    pub recent_id: i64,
    pub package_name: String,
    pub name: String,
    /// Launching it opens the app; otherwise it is a task, launched with its task id.
    pub main_activity: bool,
    /// A main-activity entry whose app is a favorite (a task entry never is).
    pub favorite: bool,
    pub task_id: i64,
    pub web_uri: Option<String>,
    pub desktop_uri: Option<String>,
    pub has_icon: bool,
}

impl From<RecentView> for RecentApp {
    fn from(v: RecentView) -> Self {
        Self {
            recent_id: v.recent_id,
            package_name: v.package_name,
            name: v.name,
            main_activity: v.main_activity,
            favorite: v.favorite,
            task_id: v.task_id,
            web_uri: v.web_uri,
            desktop_uri: v.desktop_uri,
            has_icon: v.app.has_icon,
        }
    }
}

/// The feature's state for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneAppsStatus {
    pub phone: DcgClient,
    pub phone_apps_capable: bool,
    pub recent_apps_supported: bool,
    pub app_handoff_enabled: bool,
    pub phone_screen_apps: bool,
    pub remoting_supported: bool,
    pub hosted_apps: bool,
    /// The Apps view shows.
    pub apps_view_visible: bool,
    /// The "syncing" state.
    pub syncing: bool,
    pub sync: SyncState,
}

/// The record-derived inputs of the status, to announce their changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordView {
    phone: DcgClient,
    gates: [bool; 7],
}

fn gates_of(record: &PhoneRecord, app: &AppConfig) -> [bool; 7] {
    [
        launch_gates::phone_apps_capable(record),
        launch_gates::recent_apps_supported(record),
        launch_gates::app_handoff_enabled(record),
        launch_gates::phone_screen_apps(record, app),
        launch_gates::remoting_supported(record),
        launch_gates::hosted_apps(record, app),
        launch_gates::apps_view_visible(record, app),
    ]
}

/// The "syncing" state: set when the up-to-date state becomes false (and when first read not up to
/// date), for at most the configured time; cleared when up to date.
#[derive(Debug, Default)]
struct Indicator {
    syncing: bool,
    generation: u64,
    up_to_date: Option<bool>,
}

/// One phone's open store and syncing state.
pub(crate) struct PhoneState {
    pub(crate) phone: DcgClient,
    pub(crate) db: Arc<FeatureDb>,
    indicator: Mutex<Indicator>,
}

pub(crate) struct Inner {
    pub(crate) config: PhoneAppsConfig,
    pub(crate) app_config: AppConfig,
    pub(crate) app: Weak<AppLayer>,
    clock: Arc<dyn Clock>,
    pub(crate) host: Arc<dyn PhoneAppsHost>,
    stores: PhoneStores,
    phones: Mutex<HashMap<DcgClient, Arc<PhoneState>>>,
    last_view: Mutex<Option<RecordView>>,
    events: broadcast::Sender<PhoneAppsEvent>,
    runtime: Handle,
    shutdown: CancellationToken,
    /// The app lookups for other features (one per notification shown) cannot read the store.
    directory_failing: OngoingFailure,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl Inner {
    pub(crate) fn emit(&self, event: PhoneAppsEvent) {
        // No receiver is not an error.
        let _ = self.events.send(event);
    }

    pub(crate) fn now_ms(&self) -> i64 {
        phonelink_core::time::unix_ms(self.clock.now())
    }

    /// `phone`'s state, its store opened (and migrated) on first use.
    pub(crate) fn phone_state(self: &Arc<Self>, phone: &DcgClient) -> Result<Arc<PhoneState>, StoreError> {
        let (ps, created) = {
            let mut phones = self.phones.lock_or_panic();
            match phones.get(phone) {
                Some(ps) => (ps.clone(), false),
                None => {
                    let ps = Arc::new(PhoneState {
                        phone: phone.clone(),
                        db: self.stores.get(phone)?,
                        indicator: Mutex::new(Indicator::default()),
                    });
                    phones.insert(phone.clone(), ps.clone());
                    (ps, true)
                }
            }
        };

        if created {
            self.refresh_sync_state(&ps);
        }
        Ok(ps)
    }

    /// Re-reads the sequence pairs; when the up-to-date state changed (or is read the first time)
    /// the "syncing" state shows while not up to date, for at most the configured time. The pairs are
    /// read while the indicator is held, so a refresh racing another writer decides on the pairs as
    /// they are when its turn comes.
    pub(crate) fn refresh_sync_state(self: &Arc<Self>, ps: &Arc<PhoneState>) {
        let (generation, up) = {
            let mut ind = ps.indicator.lock_or_panic();
            let up = match store::sync_state(&ps.db) {
                Ok(s) => s.up_to_date(),
                Err(e) => {
                    error!(phone = %ps.phone, error = %e, "phone apps store not read; the syncing state is unchanged");
                    return;
                }
            };

            if ind.up_to_date == Some(up) {
                return;
            }

            ind.up_to_date = Some(up);
            ind.generation += 1;
            ind.syncing = !up;
            (ind.generation, up)
        };

        if !up {
            let (weak, ps2, max) = (Arc::downgrade(self), ps.clone(), self.config.syncing_indicator_max);
            self.runtime.spawn(async move {
                tokio::time::sleep(max).await;
                let Some(inner) = weak.upgrade() else { return };

                let ended = {
                    let mut ind = ps2.indicator.lock_or_panic();
                    let ends = ind.generation == generation && ind.syncing;
                    if ends {
                        ind.syncing = false;
                    }
                    ends
                };

                if ended {
                    inner.emit(PhoneAppsEvent::StatusChanged { phone: ps2.phone.clone() });
                }
            });
        }

        self.emit(PhoneAppsEvent::StatusChanged { phone: ps.phone.clone() });
    }

    fn app_layer(&self) -> Result<Arc<AppLayer>, PhoneAppsError> {
        self.app.upgrade().ok_or(PhoneAppsError::AppGone)
    }

    /// The selected phone, its state and its record.
    fn selected(self: &Arc<Self>) -> Result<(Arc<PhoneState>, PhoneRecord), PhoneAppsError> {
        let app = self.app_layer()?;
        let record = app.phone_record().ok_or(PhoneAppsError::NoPhone)?;
        let ps = self.phone_state(&record.phone())?;
        Ok((ps, record))
    }

    /// *All apps* of `ps`, in the order of the user's language.
    fn all_apps(&self, ps: &PhoneState) -> Result<Vec<store::AppRow>, PhoneAppsError> {
        let rows = present::listed(store::apps(&ps.db)?);
        if rows.is_empty() {
            return Ok(rows);
        }

        let names = rows.iter().map(|r| present::shown_name(&r.name)).collect();
        let order = self.host.name_order(names);
        present::reorder(rows, &order).map_err(PhoneAppsError::Edge)
    }

    fn status_of(&self, ps: &PhoneState, record: &PhoneRecord) -> Result<PhoneAppsStatus, PhoneAppsError> {
        let [capable, recent, handoff, screen, remoting, hosted, visible] = gates_of(record, &self.app_config);
        let sync = store::sync_state(&ps.db)?;

        Ok(PhoneAppsStatus {
            phone: ps.phone.clone(),
            phone_apps_capable: capable,
            recent_apps_supported: recent,
            app_handoff_enabled: handoff,
            phone_screen_apps: screen,
            remoting_supported: remoting,
            hosted_apps: hosted,
            apps_view_visible: visible,
            syncing: ps.indicator.lock_or_panic().syncing,
            sync,
        })
    }

    fn spawn_app_listener(self: &Arc<Self>, mut rx: broadcast::Receiver<AppEvent>) {
        let (weak, shutdown) = (Arc::downgrade(self), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    e = rx.recv() => e,
                };
                let Some(inner) = weak.upgrade() else { break };

                match event {
                    Ok(AppEvent::PhoneRecordChanged { .. } | AppEvent::PermissionsChanged { .. }) => {
                        inner.on_record_changed()
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "phone apps missed App events; re-reading the phone record");
                        inner.on_record_changed();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Announces a change of the record-derived gates; a newly selected phone's store is opened.
    fn on_record_changed(self: &Arc<Self>) {
        let Some(record) = self.app.upgrade().and_then(|a| a.phone_record()) else { return };
        let view = RecordView { phone: record.phone(), gates: gates_of(&record, &self.app_config) };
        let previous = self.last_view.lock_or_panic().replace(view.clone());
        if previous.as_ref() == Some(&view) {
            return;
        }

        if let Err(e) = self.phone_state(&view.phone) {
            error!(phone = %view.phone, error = %e, "phone apps store not opened");
        }
        self.emit(PhoneAppsEvent::StatusChanged { phone: view.phone });
    }
}

/// The app names and icons other features show (the notifications list and banners).
struct Directory {
    inner: Weak<Inner>,
}

impl PhoneAppDirectory for Directory {
    /// A store that cannot be read shows the app without its name and icon; that is logged once
    /// until a lookup reads the store again.
    fn app(&self, phone: &DcgClient, package: &str) -> Option<PhoneAppInfo> {
        let inner = self.inner.upgrade()?;
        let read = inner.phone_state(phone).and_then(|ps| {
            let Some(row) = store::app_by_package(&ps.db, package)? else { return Ok(None) };
            let icon = store::icon(&ps.db, package)?;
            Ok(Some((row, icon)))
        });

        match read {
            Ok(found) => {
                inner.directory_failing.clear();
                found.map(|(row, icon)| PhoneAppInfo { name: present::shown_name(&row.name), icon })
            }
            Err(e) => {
                if inner.directory_failing.starts() {
                    error!(%phone, error = %e, "phone apps store not read; other features show apps without names");
                }
                None
            }
        }
    }
}

/// The phone-apps feature handle: install it once; its methods are the UI's view of the lists and the
/// favorites. Calls act on the selected phone.
pub struct PhoneApps {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for PhoneApps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhoneApps").finish_non_exhaustive()
    }
}

impl PhoneApps {
    /// Registers the `phoneApps` processor factory and the `PhoneApps` connect provider and opens the
    /// selected phone's store. `host` answers which URIs an installed application opens and orders
    /// names in the user's language. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: PhoneAppsConfig,
        host: Arc<dyn PhoneAppsHost>,
    ) -> Result<Arc<Self>, FeatureError> {
        validate_config(&config)?;
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("phone apps must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new(Inner {
            config,
            app_config: env.config.app.clone(),
            app: Arc::downgrade(&env.app),
            clock: env.edges.clock.clone(),
            host,
            stores: PhoneStores::new(env.stores.clone(), store::FEATURE, store::MIGRATIONS),
            phones: Mutex::new(HashMap::new()),
            last_view: Mutex::new(None),
            events: broadcast::channel(256).0,
            runtime,
            shutdown: CancellationToken::new(),
            directory_failing: OngoingFailure::default(),
        });

        env.app.register_processor_factory(&[wire::CONTENT_TYPE], Arc::new(sync::Factory { inner: inner.clone() }))?;
        env.app.register_connect_contributor(
            ConnectProvider::PhoneApps,
            Arc::new(connect::Contributor { inner: inner.clone() }),
        )?;

        inner.spawn_app_listener(env.app.subscribe_events());
        if let Some(record) = env.app.phone_record() {
            inner.phone_state(&record.phone())?;
            *inner.last_view.lock_or_panic() =
                Some(RecordView { phone: record.phone(), gates: gates_of(&record, &inner.app_config) });
        }
        Ok(Arc::new(Self { inner }))
    }

    /// Events that tell the UI what to re-read.
    pub fn subscribe(&self) -> broadcast::Receiver<PhoneAppsEvent> {
        self.inner.events.subscribe()
    }

    /// The app names and icons of the stored lists, for other features.
    pub fn directory(&self) -> Arc<dyn PhoneAppDirectory> {
        Arc::new(Directory { inner: Arc::downgrade(&self.inner) })
    }

    /// The selected phone's gates and syncing state.
    pub fn status(&self) -> Result<PhoneAppsStatus, PhoneAppsError> {
        let (ps, record) = self.inner.selected()?;
        self.inner.status_of(&ps, &record)
    }

    /// *All apps*: every stored app but the Link to Windows apps, in the order of the user's language.
    pub fn apps(&self) -> Result<Vec<PhoneApp>, PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        Ok(self.inner.all_apps(&ps)?.iter().map(PhoneApp::of).collect())
    }

    /// The apps of *All apps* whose name holds every space-separated term of `query`, ignoring case.
    pub fn search(&self, query: &str) -> Result<Vec<PhoneApp>, PhoneAppsError> {
        Ok(self.apps()?.into_iter().filter(|a| present::matches_search(&a.name, query)).collect())
    }

    /// The favorites, in their order.
    pub fn favorites(&self) -> Result<Vec<PhoneApp>, PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        Ok(present::favorites(&self.inner.all_apps(&ps)?).iter().map(PhoneApp::of).collect())
    }

    /// The recent apps (at most 3), most recent first; none while the phone does not support them.
    pub fn recent(&self) -> Result<Vec<RecentApp>, PhoneAppsError> {
        let (ps, record) = self.inner.selected()?;
        if !launch_gates::recent_apps_supported(&record) {
            return Ok(Vec::new());
        }

        let all = self.inner.all_apps(&ps)?;
        let rows = store::recent_with_apps(&ps.db)?;
        Ok(present::recent(&all, &rows, self.inner.config.recent_apps_max).into_iter().map(Into::into).collect())
    }

    /// The stored icon bytes of `package`'s app (`None`: it has none).
    pub fn icon(&self, package: &str) -> Result<Option<Vec<u8>>, PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        Ok(store::icon(&ps.db, package)?)
    }

    /// The name the phone sent for `package`, as stored (an excluded package's too): a mirroring
    /// window's title.
    pub fn stored_name(&self, package: &str) -> Result<Option<String>, PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        Ok(store::app_by_package(&ps.db, package)?.map(|r| r.name))
    }

    /// Adds `package` to the favorites, last (its rank is the current time).
    pub fn add_favorite(&self, package: &str) -> Result<(), PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        let rank = phonelink_core::time::ticks_from_unix_ms(self.inner.now_ms());
        if !store::set_favorite_rank(&ps.db, package, rank)? {
            return Err(PhoneAppsError::UnknownApp(package.to_string()));
        }

        debug!(phone = %ps.phone, "favorite added");
        self.inner.emit(PhoneAppsEvent::AppsChanged { phone: ps.phone.clone() });
        Ok(())
    }

    /// Removes `package` from the favorites.
    pub fn remove_favorite(&self, package: &str) -> Result<(), PhoneAppsError> {
        let (ps, _) = self.inner.selected()?;
        if !store::set_favorite_rank(&ps.db, package, -1)? {
            return Err(PhoneAppsError::UnknownApp(package.to_string()));
        }

        self.inner.emit(PhoneAppsEvent::AppsChanged { phone: ps.phone.clone() });
        Ok(())
    }

    /// Puts the favorites in `packages`' order: each gets the rank `now + its position`. Every package
    /// must be stored and named once; a package left out keeps its rank.
    pub fn reorder_favorites(&self, packages: &[String]) -> Result<(), PhoneAppsError> {
        let mut seen = std::collections::HashSet::new();
        if !packages.iter().all(|p| seen.insert(p.as_str())) {
            return Err(PhoneAppsError::InvalidInput("a package is named twice"));
        }

        let (ps, _) = self.inner.selected()?;
        let start = phonelink_core::time::ticks_from_unix_ms(self.inner.now_ms());
        store::set_favorite_ranks(&ps.db, packages, start)?.map_err(PhoneAppsError::UnknownApp)?;
        self.inner.emit(PhoneAppsEvent::AppsChanged { phone: ps.phone.clone() });
        Ok(())
    }
}
