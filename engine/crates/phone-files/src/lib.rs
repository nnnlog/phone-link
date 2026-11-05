//! Phone files, a Cross Device Experience Host feature on the CDEH identity's connection: the phone's
//! shared storage as a folder tree in the Mac's file manager.
//!
//! The Mac keeps the phone's file metadata (the golden state, kept current by sequence-numbered
//! syncs) and fetches content and thumbnails only when the file manager needs them; new files and
//! folders, renames, moves and deletes go back to the phone.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the requests and their replies, the tag-4 publication, channel targets, the upload manifest |
//! | [`path`] | where an item sits in the tree, its name, the remote path sent back |
//! | [`store`] | the per-phone state and configuration, the golden state, synthetic folders, the change journal, thumbnails |
//! | [`channel`] | the per-operation NamedStreams channels, their failures, the breaker |
//! | `sync` | metadata syncs (full, incremental, batched) and how a request is sent (channel or `/DeviceResourceManager`) |
//! | `thumbnails` | single thumbnails and the prefetch |
//! | `content` | downloads and uploads |
//! | `ops` | create folder, rename and move, delete (single and batched) |
//! | `tree` | the tree as the file manager reads it: items, children, changes |
//! | this module | the options, enablement and start, tags 4 and 3, the context handle, the config sync, the permission request |

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod channel;
pub mod path;
pub mod store;
pub mod wire;

mod content;
mod ops;
mod sync;
mod thumbnails;
mod tree;

use phonelink_core::config::features::PhoneFilesConfig;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::UNIX_EPOCH;

use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use phonelink_core::seams::SessionEvent;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::shared::files_permission::PermissionResult;
use phonelink_features::{FeatureEnv, FeatureError, PhoneSwitchChanged};
use phonelink_proto::phone_files::v1::FilesConfigurationPayload;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use phonelink_transport::platform::{
    ContextHandle, DeviceFilter, Drm, PlatformRoutes, Publication, PublicationObserver, SessionValidator, Subscription,
};

pub use channel::{FilesChannels, TransferError};

mod permission;
mod publications;
mod requests;
mod start;
#[cfg(feature = "test-support")]
pub use channel::{Breaker, FilesChannel};
pub use content::{FileTimes, UploadError};
#[cfg(feature = "test-support")]
pub use ops::OpFailure;
pub use ops::{DeleteError, OpError};
pub use store::{FEATURE, PhoneData};
#[cfg(feature = "test-support")]
pub use tree::ROOT;
pub use tree::{Item, ItemChanges};
pub use wire::{Status, Trigger};

use channel::{Breaker as PhoneBreaker, BreakerChange};
use path::Location;
use phonelink_core::sync::MutexExt;
use store::Store;

/// What the UI and the file manager hear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhoneFilesEvent {
    /// What [`PhoneFiles::status`] shows for `phone` may have changed.
    StatusChanged { phone: DcgClient },
    /// `phone`'s tree appeared (`running`) or went away with everything in it.
    Running { phone: DcgClient, running: bool },
    /// `phone`'s tree changed: ask for the changes.
    ItemsChanged { phone: DcgClient },
    /// Every delete of a batch succeeded: tell the user deleted files go to the phone's recycle bin
    /// (unless they chose not to be told again).
    RecycleBinNotice { phone: DcgClient },
}

/// Where the latest metadata sync stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    Idle,
    Preparing,
    Syncing,
    Succeeded,
    /// The phone could not be reached.
    ConnectionFailed,
    Failed(String),
}

/// The feature's state for one phone, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneFilesStatus {
    /// The phone serves phone files.
    pub capable: bool,
    /// The user's switch.
    pub enabled: bool,
    /// The tree exists.
    pub running: bool,
    pub permission_granted: bool,
    /// The phone is connected.
    pub available: bool,
    pub sync: SyncState,
    pub initial_sync_done: bool,
    pub storage_free: i64,
    pub storage_total: i64,
    /// The phone's bytes per gigabyte, for showing sizes as it does.
    pub gigabyte_bytes: i64,
    /// The location of the phone's recycle bin in the tree, when it has one.
    pub recycle_bin: Option<Location>,
    /// Link to Windows is turned off on the phone.
    pub ltw_off: bool,
    /// The user chose not to be told where deleted files go.
    pub recycle_notice_off: bool,
}

/// Where the user asked for the permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOrigin {
    /// The prompt shown when the feature needs the permission.
    Dialog,
    /// The status shown in the settings.
    StatusUi,
}

/// An operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum PhoneFilesError {
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    #[error("phone files is not running for this phone")]
    NotRunning,
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// A phone's map and store key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The per-phone locks that serialize one kind of operation.
#[derive(Default)]
struct PhoneLocks {
    /// One metadata sync at a time, in arrival order.
    sync: tokio::sync::Mutex<()>,
    download: tokio::sync::Mutex<()>,
    upload: tokio::sync::Mutex<()>,
    thumbnail: tokio::sync::Mutex<()>,
    prefetch: tokio::sync::Mutex<()>,
    /// One delete batch on the wire at a time.
    delete: tokio::sync::Mutex<()>,
}

/// The in-memory state of one phone.
struct PhoneRuntime {
    /// The context handle of the phone's latest tag-4 publication.
    handle: Option<ContextHandle>,
    /// Counts the new handles stored: a handle request watches it for one arriving.
    handle_arrivals: watch::Sender<u64>,
    /// A configuration was published since the phone last became unavailable.
    config_up_to_date: bool,
    /// The tree exists.
    running: bool,
    breaker: PhoneBreaker,
    /// Link to Windows is turned off on the phone, as its last answers said.
    ltw_off: bool,
    sync: SyncState,
    /// Cancelled when the phone stops being available: the pending syncs end. A child of `run_cancel`.
    sync_cancel: CancellationToken,
    /// Cancelled when the feature stops for the phone: every operation ends.
    run_cancel: CancellationToken,
    /// Items being downloaded.
    downloads: HashSet<i64>,
    locks: Arc<PhoneLocks>,
    deletes: ops::DeleteQueue,
}

impl PhoneRuntime {
    fn new(config: &PhoneFilesConfig) -> Self {
        let run_cancel = CancellationToken::new();
        Self {
            handle: None,
            handle_arrivals: watch::channel(0).0,
            config_up_to_date: false,
            running: false,
            breaker: PhoneBreaker::new(config.breaker_threshold, config.breaker_open),
            ltw_off: false,
            sync: SyncState::Idle,
            sync_cancel: run_cancel.child_token(),
            run_cancel,
            downloads: HashSet::new(),
            locks: Arc::default(),
            deletes: ops::DeleteQueue::default(),
        }
    }
}

/// Why the feature looks at a phone's state again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    /// The phone joined the account, or the feature started.
    Registered,
    /// The user's switch changed.
    Switch,
    /// The per-phone switch of the CDEH features changed.
    PhoneSwitch,
    /// A configuration was published.
    Config,
    /// The permission was granted.
    Permission,
}

/// The feature's shared state; tasks and callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    config: PhoneFilesConfig,
    drm: Arc<Drm>,
    validator: Arc<SessionValidator>,
    channels: Arc<dyn FilesChannels>,
    stores: PhoneStores,
    phones: Mutex<HashMap<DcgClient, PhoneRuntime>>,
    /// Serializes read-modify-write of a phone's state row.
    data_lock: Mutex<()>,
    /// The gate of a waiting upload may have opened.
    gate: Notify,
    events: broadcast::Sender<PhoneFilesEvent>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscriptions: Mutex<Vec<Subscription>>,
    /// The phone switch's last read failed and was logged; reset by a read that works.
    switch_unreadable: AtomicBool,
    /// The same for the phone's stored configuration.
    config_unreadable: AtomicBool,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The phone-files feature.
pub struct PhoneFiles {
    inner: Arc<Inner>,
}

impl PhoneFiles {
    /// Installs the feature on the CDEH identity's `env` and `routes`: the tag-4 and tag-3
    /// subscriptions, the triggers, and the registration of every phone now. Must run inside a tokio
    /// runtime.
    pub fn install(
        env: &FeatureEnv,
        config: PhoneFilesConfig,
        routes: &PlatformRoutes,
        channels: Arc<dyn FilesChannels>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("phone files must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            drm: routes.drm.clone(),
            validator: routes.validator.clone(),
            channels,
            stores: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
            phones: Mutex::default(),
            data_lock: Mutex::new(()),
            gate: Notify::new(),
            events: broadcast::channel(256).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscriptions: Mutex::default(),
            switch_unreadable: AtomicBool::new(false),
            config_unreadable: AtomicBool::new(false),
            weak: weak.clone(),
        });

        let triggers = Triggers {
            peers: env.peers.subscribe(),
            sessions: env.sessions.subscribe(),
            switches: env.switches.subscribe(),
        };

        let files =
            routes.msaep.subscribe(wire::FILES_TAG, DeviceFilter::Any, Arc::new(FilesObserver(inner.weak.clone())));
        let permissions = routes.msaep.subscribe(
            wire::PERMISSIONS_TAG,
            DeviceFilter::Any,
            Arc::new(PermissionObserver(inner.weak.clone())),
        );

        inner.subscriptions.lock_or_panic().extend([files, permissions]);
        inner.spawn_loop(triggers);
        for peer in inner.env.peers.peers() {
            inner.register(&peer);
        }

        Ok(Arc::new(Self { inner }))
    }

    /// Events for the UI and the file manager.
    pub fn subscribe(&self) -> broadcast::Receiver<PhoneFilesEvent> {
        self.inner.events.subscribe()
    }

    fn peer(&self, phone: &DcgClient) -> Result<PeerDevice, PhoneFilesError> {
        self.inner.env.peers.peer(phone).ok_or(PhoneFilesError::UnknownPhone)
    }

    /// `phone`'s name in the account's device list.
    pub fn phone_name(&self, phone: &DcgClient) -> Option<String> {
        self.inner.env.peers.peer(phone).and_then(|p| p.info.name)
    }

    /// `phone`'s state.
    pub fn status(&self, phone: &DcgClient) -> Result<PhoneFilesStatus, PhoneFilesError> {
        self.peer(phone)?;
        let data = self.inner.store(phone)?.data()?;
        let (running, sync, ltw_off) = self.inner.with_phone(phone, |p| (p.running, p.sync.clone(), p.ltw_off));
        let config = data.config.clone().unwrap_or_default();

        Ok(PhoneFilesStatus {
            capable: data.capable,
            enabled: data.enabled,
            running,
            permission_granted: data.permission_granted,
            available: self.inner.available(phone),
            sync,
            initial_sync_done: data.initial_sync_done,
            storage_free: data.storage_free,
            storage_total: data.storage_total,
            gigabyte_bytes: if config.gigabyte_to_byte > 0 { config.gigabyte_to_byte } else { 1_000_000_000 },
            recycle_bin: path::recycle_bin(&config.recycle_bin_path),
            ltw_off,
            recycle_notice_off: data.recycle_notice_off,
        })
    }

    /// The user's switch for `phone`. Turning it on turns it off for every other phone first.
    pub fn set_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), PhoneFilesError> {
        let peer = self.peer(phone)?;
        if enabled {
            for other in self.inner.env.peers.peers().into_iter().filter(|p| key(&p.client) != key(phone)) {
                let (before, after) = self.inner.update(&other.client, |d| d.enabled = false)?;
                if before != after {
                    self.inner.reconcile(&other, Cause::Switch);
                }
            }
        }

        let (before, after) = self.inner.update(phone, |d| {
            d.enabled = enabled;
            if enabled {
                d.recycle_notice_off = false;
            }
        })?;

        if before.enabled != after.enabled {
            info!(%phone, enabled, "phone files switched");
            self.inner.reconcile(&peer, Cause::Switch);
        }

        self.inner.status_changed(phone);
        Ok(())
    }

    /// The user chose not to be told again where deleted files go.
    pub fn set_recycle_notice_off(&self, phone: &DcgClient) -> Result<(), PhoneFilesError> {
        self.peer(phone)?;
        self.inner.update(phone, |d| d.recycle_notice_off = true)?;
        self.inner.status_changed(phone);
        Ok(())
    }

    /// Asks the phone for the files permission (it prompts).
    pub async fn request_permission(
        &self,
        phone: &DcgClient,
        origin: PermissionOrigin,
    ) -> Result<PermissionResult, PhoneFilesError> {
        let peer = self.peer(phone)?;
        Ok(self.inner.request_permission(&peer, origin).await)
    }

    /// The user's refresh: an incremental sync that makes the phone rescan, or a full one when nothing
    /// was synced yet.
    pub fn refresh(&self, phone: &DcgClient) -> Result<(), PhoneFilesError> {
        let peer = self.peer(phone)?;
        if !self.inner.with_phone(phone, |p| p.running) {
            return Err(PhoneFilesError::NotRunning);
        }
        self.inner.spawn_refresh(peer, true, Trigger::RefreshClicked);
        Ok(())
    }

    /// The entry `identifier` names ([`ROOT`] is the tree's root).
    pub fn item(&self, phone: &DcgClient, identifier: &str) -> Result<Option<Item>, PhoneFilesError> {
        self.inner.running_store(phone)?;
        self.inner.item(phone, identifier)
    }

    /// The entries directly inside the folder `identifier`; `None` when it is not a folder of the tree.
    pub fn children(&self, phone: &DcgClient, identifier: &str) -> Result<Option<Vec<Item>>, PhoneFilesError> {
        self.inner.running_store(phone)?;
        self.inner.children(phone, identifier)
    }

    /// The tree's current generation: what the file manager has seen once it read everything.
    pub fn generation(&self, phone: &DcgClient) -> Result<i64, PhoneFilesError> {
        Ok(self.inner.running_store(phone)?.generations()?.0)
    }

    /// What changed after `generation`; `None` when it is too old (read everything again).
    pub fn changes(&self, phone: &DcgClient, generation: i64) -> Result<Option<ItemChanges>, PhoneFilesError> {
        self.inner.running_store(phone)?;
        self.inner.changes(phone, generation)
    }

    /// The thumbnail of the file `identifier` (JPEG); `None` when it has none.
    pub async fn thumbnail(&self, phone: &DcgClient, identifier: &str) -> Result<Option<Vec<u8>>, TransferError> {
        let peer = self.peer(phone).map_err(|e| TransferError::Failed(e.to_string()))?;
        self.inner.clone().thumbnail(&peer, identifier).await
    }

    /// Downloads the file `identifier` into a new temporary file; `progress` hears `(done, total)`.
    pub async fn download(
        &self,
        phone: &DcgClient,
        identifier: &str,
        user: bool,
        progress: Arc<dyn Fn(u64, u64) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<(PathBuf, Item), TransferError> {
        let peer = self.peer(phone).map_err(|e| TransferError::Failed(e.to_string()))?;
        let result = self.inner.clone().download(&peer, identifier, user, progress, cancel).await;

        // A failure is the app's to log: it gets the error.
        match &result {
            Ok(_) => info!(%phone, outcome = "completed", "download ended"),
            Err(TransferError::UserCancelled) => info!(%phone, outcome = "cancelled", "download ended"),
            Err(_) => {}
        }
        result
    }

    /// Copies the local file `source` to the phone as `name` inside the folder `parent`; waits until the
    /// phone has it (or the retries ran out).
    pub async fn upload(
        &self,
        phone: &DcgClient,
        parent: &str,
        name: &str,
        source: PathBuf,
        times: Option<FileTimes>,
        cancel: CancellationToken,
    ) -> Result<Item, UploadError> {
        let peer = self.peer(phone).map_err(|_| UploadError::NotRunning)?;
        let result = self.inner.clone().upload(&peer, parent, name, source, times, cancel).await;

        // A failure is the app's to log: it gets the error.
        match &result {
            Ok(_) => info!(%phone, outcome = "completed", "upload ended"),
            Err(UploadError::Cancelled) => info!(%phone, outcome = "cancelled", "upload ended"),
            Err(_) => {}
        }
        result
    }

    /// Creates the folder `name` inside the folder `parent`.
    pub async fn create_folder(&self, phone: &DcgClient, parent: &str, name: &str) -> Result<Item, OpError> {
        let peer = self.peer(phone).map_err(|_| OpError::NotRunning)?;
        self.inner.create_folder(&peer, parent, name).await
    }

    /// Renames `identifier` to `name` and moves it into the folder `parent`.
    pub async fn rename(&self, phone: &DcgClient, identifier: &str, parent: &str, name: &str) -> Result<Item, OpError> {
        let peer = self.peer(phone).map_err(|_| OpError::NotRunning)?;
        self.inner.rename(&peer, identifier, parent, name).await
    }

    /// Deletes `identifier` (a folder with everything in it); waits for the phone's answer.
    pub async fn delete(&self, phone: &DcgClient, identifier: &str) -> Result<(), DeleteError> {
        let peer = self.peer(phone).map_err(|_| DeleteError::NotRunning)?;
        self.inner.clone().delete(&peer, identifier).await
    }
}

/// The trigger streams, subscribed before any state is read.
struct Triggers {
    peers: broadcast::Receiver<PeerEvent>,
    sessions: broadcast::Receiver<SessionEvent>,
    switches: broadcast::Receiver<PhoneSwitchChanged>,
}

impl Inner {
    fn emit(&self, event: PhoneFilesEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn status_changed(&self, phone: &DcgClient) {
        self.emit(PhoneFilesEvent::StatusChanged { phone: phone.clone() });
    }

    fn items_changed(&self, phone: &DcgClient) {
        self.emit(PhoneFilesEvent::ItemsChanged { phone: phone.clone() });
    }

    fn store(&self, phone: &DcgClient) -> Result<Store, StoreError> {
        Ok(Store::new(self.stores.get(&key(phone))?))
    }

    /// The store of a phone whose tree exists.
    fn running_store(&self, phone: &DcgClient) -> Result<Store, PhoneFilesError> {
        if !self.with_phone(phone, |p| p.running) {
            return Err(PhoneFilesError::NotRunning);
        }
        Ok(self.store(phone)?)
    }

    /// Applies `f` to `phone`'s state row: `(before, after)`.
    fn update(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneData)) -> Result<(PhoneData, PhoneData), StoreError> {
        let _guard = self.data_lock.lock_or_panic();
        self.store(phone)?.update(f)
    }

    fn with_phone<R>(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneRuntime) -> R) -> R {
        let mut phones = self.phones.lock_or_panic();
        let entry = phones.entry(key(phone)).or_insert_with(|| PhoneRuntime::new(&self.config));
        f(entry)
    }

    fn locks(&self, phone: &DcgClient) -> Arc<PhoneLocks> {
        self.with_phone(phone, |p| p.locks.clone())
    }

    fn set_sync(&self, phone: &DcgClient, state: SyncState) {
        let changed = self.with_phone(phone, |p| std::mem::replace(&mut p.sync, state.clone()) != state);
        if changed {
            self.status_changed(phone);
        }
    }

    /// The per-phone switch of the CDEH features; an unreadable switch counts as off. It is read on
    /// every request and gate check, so a failure is logged when it starts.
    fn phone_enabled(&self, phone: &DcgClient) -> bool {
        match self.env.switches.is_enabled(phone) {
            Ok(enabled) => {
                self.switch_unreadable.store(false, Ordering::Relaxed);
                enabled
            }
            Err(e) => {
                if !self.switch_unreadable.swap(true, Ordering::Relaxed) {
                    warn!(%phone, error = %e, "phone switch unreadable; counted as off");
                }
                false
            }
        }
    }

    /// Records whether Link to Windows is off on the phone; a change is logged and shown.
    fn set_ltw_off(&self, phone: &DcgClient, off: bool) {
        if self.with_phone(phone, |p| std::mem::replace(&mut p.ltw_off, off)) == off {
            return;
        }

        if off {
            info!(%phone, "Link to Windows is turned off on the phone");
        } else {
            info!(%phone, "Link to Windows is on again on the phone");
        }
        self.status_changed(phone);
    }

    /// Records an attempt over a channel in `phone`'s breaker; the breaker opening or closing is
    /// logged, each attempt is not.
    fn record_channel_attempt(&self, phone: &DcgClient, outcome: &Result<(), TransferError>) {
        match self.with_phone(phone, |p| p.breaker.record(outcome)) {
            Some(BreakerChange::Opened) => {
                let error = outcome.as_ref().err().map(ToString::to_string);
                warn!(%phone, error, "channel requests keep failing; metadata and thumbnails go over the relay for a while");
            }
            Some(BreakerChange::Closed) => info!(%phone, "channel requests work again"),
            None => {}
        }
    }

    fn available(&self, phone: &DcgClient) -> bool {
        self.env.sessions.is_connected(phone)
    }

    fn now_secs(&self) -> i64 {
        self.env.edges.clock.now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    /// The phone's stored configuration (defaults when none was published, or when the store cannot be
    /// read: it is read for every request, so that failure is logged when it starts).
    fn phone_config(&self, phone: &DcgClient) -> FilesConfigurationPayload {
        match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => {
                self.config_unreadable.store(false, Ordering::Relaxed);
                d.config.unwrap_or_default()
            }
            Err(e) => {
                if !self.config_unreadable.swap(true, Ordering::Relaxed) {
                    warn!(%phone, error = %e, "phone-files configuration unreadable; the defaults are used");
                }
                FilesConfigurationPayload::default()
            }
        }
    }
}

/// A request's trace: within `operation` (same trace id) when there is one, a new trace otherwise.
fn request_trace(operation: Option<&TraceContext>, trace_state: Vec<(String, String)>) -> TraceContext {
    match operation {
        Some(op) => op.for_request(trace_state),
        None => TraceContext::new_root(trace_state),
    }
}

struct FilesObserver(Weak<Inner>);

impl PublicationObserver for FilesObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_files(publication);
        }
    }
}

struct PermissionObserver(Weak<Inner>);

impl PublicationObserver for PermissionObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_permissions(publication);
        }
    }
}
