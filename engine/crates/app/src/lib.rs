//! The App message layer shared by Phone Link's features.
//!
//! | Module | Covers |
//! |---|---|
//! | [`request`] | Building an outbound App request, sending it with timeouts and retries, reading its response |
//! | [`message`], [`outcome`] | Reading an App message (inflation, key lift) and mapping its `result` to an outcome |
//! | [`connect`] | The connect message and the connect sequence |
//! | [`inbound`], [`processor`] | Inbound dispatch, and unknown or unsupported inbound traffic |
//! | [`shared`] | Shared messages (`heartbeat`, `permissions`, `/permission`, `deviceQuery`, `phoneCommand`) |
//! | [`phone_record`] | Preconditions and state (phone record, `installationId`, app session id) |
//!
//! [`AppLayer`] is the facade: features send requests through it, register their content-processor
//! factories and connect contributors on it, and read the phone record from it. It talks to the
//! platform layer only through [`PlatformMessenger`] and [`PartnerSessionControl`] and receives
//! inbound App messages as the platform router's [`phonelink_core::seams::AppInbound`].

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod connect;
pub mod consts;
pub mod inbound;
pub mod message;
pub mod outcome;
pub mod phone_record;
pub mod processor;
pub mod request;
pub mod shared;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};

use async_trait::async_trait;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use phonelink_core::config::{EngineConfig, display_name_from_host};
use phonelink_core::edge::{EdgeError, Edges, LockState};
use phonelink_core::seams::{PartnerSessionControl, PlatformMessenger, SessionEvent};
use phonelink_core::tap::Tap;
use phonelink_core::types::{DcgClient, DeviceInfoItem};

#[cfg(feature = "test-support")]
pub use connect::ConnectSendResult;
pub use connect::{
    ConnectContext, ConnectContribution, ConnectContributor, ConnectKind, ConnectProvider, ConnectTrigger,
};
#[cfg(feature = "test-support")]
pub use inbound::{DropReason, InboundDisposition};
#[cfg(feature = "test-support")]
pub use message::{AppMessage, ContractVersion};
pub use outcome::AppOutcome;
#[cfg(feature = "test-support")]
pub use outcome::ResourceFailure;
#[cfg(feature = "test-support")]
pub use phone_record::HotspotRecord;
pub use phone_record::{BluetoothRecord, PhoneRecord, StatusKeys};
pub use processor::{
    ContentProcessor, ContentProcessorFactory, InboundContext, Malformed, Processed, Rejection, rejection_message,
    simple_answer,
};
pub use request::{AppRequest, AppResponse, DrmAppRequest, RequestOptions, StandardKeys};
#[cfg(feature = "test-support")]
pub use request::{LocalGate, RequestKind};

use connect::ConnectState;
use consts::content_type;
use inbound::{InboundItem, TransactionKey};
use phone_record::{load_or_create_installation_id, load_record, process_app_session_id, record_key, save_record};
use phonelink_core::sync::{MutexExt, RwLockExt};

/// Failures of the App layer's own invariants (runtime results of requests are [`AppOutcome`]s).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    #[error("store failure: {0}")]
    Store(#[from] EdgeError),
    #[error("stored {key} is corrupt: {reason}")]
    CorruptStore { key: String, reason: String },
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("programming error: {0}")]
    Programming(String),
    #[error("the App layer must be started inside a tokio runtime")]
    NoRuntime,
}

/// The local account gate, "the phone is not linked to the current account, or the account needs
/// the user's attention" — port: the enrolled account is missing or its token cannot be refreshed.
///
/// Implemented by the integrator over the auth layer.
#[async_trait]
pub trait AccountGate: Send + Sync {
    /// `true` when App requests to `phone` may be sent.
    async fn is_ready(&self, phone: &DcgClient) -> bool;
}

/// Called after every App request with its outcome, gated or attempted (e.g. for the device-status
/// rule that resets the recorded Wi-Fi state after a failed request).
pub trait RequestObserver: Send + Sync {
    fn on_request_completed(&self, phone: &DcgClient, route: &str, response: &AppResponse);
}

/// App-level events for the UI and the features.
#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    /// `phoneCommand` type 1 with a valid `ms-phone:` URI that is not a remoting URI: bring the app's
    /// window forward.
    BringWindowForward { uri: String },
    /// `phoneCommand` type 1 with a remoting URI for `phone`: screen mirroring launches what it names.
    RemotingLaunch { phone: DcgClient, uri: shared::RemotingUri },
    /// The phone's "all features disabled" flag changed.
    AllFeaturesDisabled { phone: DcgClient, disabled: bool },
    /// The phone's permission bits (`DevicePermissions`) or the set of reported bits changed; read
    /// a flag with [`PhoneRecord::permission_or`].
    PermissionsChanged { phone: DcgClient, bits: u32 },
    /// Any field of the phone record changed.
    PhoneRecordChanged { phone: DcgClient },
    /// A connect finished (sent; not dropped).
    ConnectCompleted { phone: DcgClient, kind: ConnectKind, outcome: AppOutcome },
}

struct PhoneState {
    phone: DcgClient,
    record: PhoneRecord,
}

/// The feature connect providers by position (iteration = provider order).
type ContributorSlots = BTreeMap<u8, (ConnectProvider, Arc<dyn ConnectContributor>)>;

// A lock of this layer that a panic poisoned (a feature bug while it was held: a phone-record
// update closure, a processor) is recovered, so one failing message does not take every later
// message and request on the shared connection with it. The data stays consistent: every write
// under these locks is one insertion, removal or whole-value replacement made after the work that
// can fail (`update_record_for` changes a copy of the record).

/// A device-list entry handed in for `phone` must be that phone's.
fn check_entry(phone: &DcgClient, device: &DeviceInfoItem) -> Result<(), AppError> {
    if device.id.eq_ignore_ascii_case(&phone.id) {
        return Ok(());
    }
    Err(AppError::Programming("a device-list entry is not the phone it was looked up for".to_owned()))
}

/// The App message layer.
pub struct AppLayer {
    pub(crate) config: Arc<EngineConfig>,
    pub(crate) edges: Edges,
    pub(crate) messenger: Arc<dyn PlatformMessenger>,
    pub(crate) sessions: Arc<dyn PartnerSessionControl>,
    pub(crate) account: Arc<dyn AccountGate>,
    pub(crate) tap: Tap,
    pub(crate) standard_keys: StandardKeys,
    pub(crate) app_session_id: String,
    foreground: AtomicBool,
    phone: Mutex<Option<PhoneState>>,
    factories: RwLock<HashMap<&'static str, Arc<dyn ContentProcessorFactory>>>,
    contributors: RwLock<ContributorSlots>,
    observers: RwLock<Vec<Arc<dyn RequestObserver>>>,
    pub(crate) transactions: Mutex<HashMap<TransactionKey, Box<dyn ContentProcessor>>>,
    pub(crate) inbound_tx: mpsc::UnboundedSender<InboundItem>,
    inbound_rx: Mutex<Option<mpsc::UnboundedReceiver<InboundItem>>>,
    events: broadcast::Sender<AppEvent>,
    pub(crate) connect: ConnectState,
    runtime: OnceLock<Handle>,
    /// Set by [`AppLayer::start`]; until then a connect trigger sends nothing.
    started: AtomicBool,
    /// The last write of a record update failed: the next failures are not logged until one
    /// succeeds (a failing store fails every update).
    record_save_failing: AtomicBool,
    /// Held while [`AppLayer::start`] sets `started` and reads the foreground state, and while
    /// [`AppLayer::foreground_changed`] records it, so a coming to the foreground around the start
    /// sends exactly one activation connect.
    activation: Mutex<()>,
    shutdown: CancellationToken,
    pub(crate) weak_self: Weak<AppLayer>,
}

impl Drop for AppLayer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl AppLayer {
    /// Loads (or creates and persists) the `installationId`, takes the app session id of this run
    /// (one per process: a layer built again in the same run sends the same one), resolves the
    /// display name (`config.identity.display_name`, else the host name cut before its first `.`),
    /// and registers the built-in `heartbeat`, `permissions`, `deviceQuery` and `phoneCommand`
    /// processors. Spawns nothing; [`AppLayer::start`] does.
    pub fn new(
        config: Arc<EngineConfig>,
        edges: Edges,
        messenger: Arc<dyn PlatformMessenger>,
        sessions: Arc<dyn PartnerSessionControl>,
        account: Arc<dyn AccountGate>,
        tap: Tap,
    ) -> Result<Arc<Self>, AppError> {
        let installation_id = load_or_create_installation_id(&*edges.store)?;
        let display_name = match &config.identity.display_name {
            Some(name) => name.clone(),
            None => display_name_from_host(&edges.host.host_name()),
        };

        if display_name.trim().is_empty() {
            return Err(AppError::Config("the PC display name is empty".into()));
        }

        let standard_keys = StandardKeys {
            display_name,
            app_version: config.app.app_version.clone(),
            installation_id,
            contract_version: config.app.contract_version,
        };

        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (events, _) = broadcast::channel(256);
        let factories = shared::builtin_factories().into_iter().collect();

        Ok(Arc::new_cyclic(|weak| Self {
            config,
            edges,
            messenger,
            sessions,
            account,
            tap,
            standard_keys,
            app_session_id: process_app_session_id().to_string(),
            foreground: AtomicBool::new(false),
            phone: Mutex::new(None),
            factories: RwLock::new(factories),
            contributors: RwLock::new(BTreeMap::new()),
            observers: RwLock::new(Vec::new()),
            transactions: Mutex::new(HashMap::new()),
            inbound_tx,
            inbound_rx: Mutex::new(Some(inbound_rx)),
            events,
            connect: ConnectState::new(),
            runtime: OnceLock::new(),
            started: AtomicBool::new(false),
            record_save_failing: AtomicBool::new(false),
            activation: Mutex::new(()),
            shutdown: CancellationToken::new(),
            weak_self: weak.clone(),
        }))
    }

    /// Starts the inbound worker (arrival-order processing), subscribes to the session events for
    /// the became-`Connected` connect trigger, and — when a phone is selected — sends the
    /// engine-start connect, then the immediate connect of "app came to the foreground" when the
    /// app is in the foreground: a coming to the foreground reported before the start was only
    /// recorded. Call it once every feature's connect contributor is registered — connect triggers
    /// send nothing before it, so no connect is built from a partial provider set. Must run inside
    /// a tokio runtime; a second call does nothing.
    pub fn start(self: &Arc<Self>) -> Result<(), AppError> {
        let handle = Handle::try_current().map_err(|_| AppError::NoRuntime)?;
        // Before `started`: a trigger that passes the gate from another thread finds the runtime.
        let _ = self.runtime.set(handle.clone());

        let foreground = {
            let _activation = self.activation.lock_or_recover();
            if self.started.swap(true, Ordering::SeqCst) {
                return Ok(());
            }
            self.is_foreground()
        };

        if let Some(mut rx) = self.inbound_rx.lock_or_recover().take() {
            let weak = Arc::downgrade(self);
            let shutdown = self.shutdown.clone();
            handle.spawn(async move {
                loop {
                    let item = tokio::select! {
                        _ = shutdown.cancelled() => break,
                        item = rx.recv() => item,
                    };
                    let Some(item) = item else { break };
                    let Some(app) = weak.upgrade() else { break };
                    app.handle_inbound_isolated(item).await;
                }
            });
        }

        let mut session_events = self.sessions.subscribe();
        let weak = Arc::downgrade(self);
        let shutdown = self.shutdown.clone();
        handle.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = session_events.recv() => event,
                };
                match event {
                    Ok(SessionEvent::Connected(partner)) => {
                        let Some(app) = weak.upgrade() else { break };
                        if app.selected_phone().is_some_and(|p| p.is_same_client(&partner)) {
                            app.trigger(ConnectTrigger::BecameConnected);
                        }
                    }
                    Ok(SessionEvent::Disconnected { .. }) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "session events lagged; a connect on Connected may not be sent");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        if self.selected_phone().is_some() {
            self.trigger(ConnectTrigger::EngineStart);
            if foreground {
                self.trigger(ConnectTrigger::AppActivated);
            }
        }

        Ok(())
    }

    /// [`AppLayer::start`] ran: connect triggers send.
    pub fn is_started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// The engine configuration the layer was built with.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Selects `phone` and writes its record from its device-list entry (Android). The record is
    /// loaded from the store the first time a phone is selected, and persisted. Every later
    /// device-list refresh rewrites the selected phone's record through
    /// [`AppLayer::reapply_device_list`].
    pub fn set_phone(&self, phone: DcgClient, device: &DeviceInfoItem) -> Result<(), AppError> {
        check_entry(&phone, device)?;
        record_key(&phone)?;

        let (before, after) = {
            let mut guard = self.phone.lock_or_recover();
            let (before, mut record) = match guard.as_ref() {
                Some(s) if s.phone.is_same_client(&phone) => (Some(s.record.clone()), s.record.clone()),
                _ => match load_record(&*self.edges.store, &phone)? {
                    Some(r) => (Some(r.clone()), r),
                    None => (None, PhoneRecord::new(&phone)),
                },
            };

            record.apply_device_list(device);
            save_record(&*self.edges.store, &record)?;

            let switched = guard.as_ref().is_none_or(|s| !s.phone.is_same_client(&phone));
            *guard = Some(PhoneState { phone: phone.clone(), record: record.clone() });
            if switched {
                self.transactions.lock_or_recover().retain(|k, _| k.phone.is_same_client(&phone));
            }
            (before, record)
        };

        match before {
            Some(before) => self.after_record_change(&before, &after),
            None => self.emit(AppEvent::PhoneRecordChanged { phone }),
        }

        Ok(())
    }

    /// Rewrites the selected phone's record from its entry of a device-list refresh — every
    /// refresh, changed or not, because the device list and the connect response write the same
    /// fields and the last writer wins. Persisted; events and the status connect trigger follow.
    /// `false` when `phone` is not the selected phone (nothing is switched or written).
    pub fn reapply_device_list(&self, phone: &DcgClient, device: &DeviceInfoItem) -> Result<bool, AppError> {
        check_entry(phone, device)?;
        Ok(self.update_record_for(phone, |r| r.apply_device_list(device)).is_some())
    }

    /// A strong handle to this layer while it is alive — for contributors and processors that
    /// spawn requests of their own from a `&AppLayer` they were handed.
    pub fn self_arc(&self) -> Option<Arc<AppLayer>> {
        self.weak_self.upgrade()
    }

    /// The selected phone.
    pub fn selected_phone(&self) -> Option<DcgClient> {
        self.phone.lock_or_recover().as_ref().map(|s| s.phone.clone())
    }

    /// A snapshot of the selected phone's record.
    pub fn phone_record(&self) -> Option<PhoneRecord> {
        self.phone.lock_or_recover().as_ref().map(|s| s.record.clone())
    }

    /// Updates the selected phone's record (persisted; events and the status connect trigger
    /// follow). The extension point for features that own record fields (status capabilities,
    /// their permission bits, the device-status keys).
    pub fn update_phone_record<R>(&self, f: impl FnOnce(&mut PhoneRecord) -> R) -> Option<R> {
        let phone = self.selected_phone()?;
        self.update_record_for(&phone, f)
    }

    /// Applies `f` to `phone`'s record when it is the selected phone; `None` otherwise. Atomic: `f`
    /// changes a copy, which replaces the record only when `f` returns — a panic in `f` leaves the
    /// record as it was.
    pub fn update_record_for<R>(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneRecord) -> R) -> Option<R> {
        let (result, before, after) = {
            let mut guard = self.phone.lock_or_recover();
            let state = guard.as_mut().filter(|s| s.phone.is_same_client(phone))?;
            let mut next = state.record.clone();
            let result = f(&mut next);

            if next == state.record {
                return Some(result);
            }

            self.log_record_save(save_record(&*self.edges.store, &next));
            let before = std::mem::replace(&mut state.record, next);
            (result, before, state.record.clone())
        };

        self.after_record_change(&before, &after);
        Some(result)
    }

    /// Logs the first failed write of a record update, and the next write that succeeds after it.
    fn log_record_save(&self, saved: Result<(), AppError>) {
        match saved {
            Err(e) if !self.record_save_failing.swap(true, Ordering::SeqCst) => {
                error!(error = %e, "phone record not persisted; it is kept in memory, later failures are not logged");
            }
            Ok(()) if self.record_save_failing.swap(false, Ordering::SeqCst) => {
                info!("phone record persisted again");
            }
            _ => {}
        }
    }

    /// Events of a record change, and the connect trigger for a changed Wi-Fi state or
    /// sync-over-mobile-data setting when afterwards either is on.
    fn after_record_change(&self, before: &PhoneRecord, after: &PhoneRecord) {
        if before == after {
            return;
        }

        let phone = after.phone();

        // A report that leaves a bit unchanged can still change what a feature reads: a permission
        // that defaults to granted, reported denied, keeps bit 0 and becomes known.
        if before.permission_bits != after.permission_bits
            || before.permission_known_bits != after.permission_known_bits
        {
            self.emit(AppEvent::PermissionsChanged { phone: phone.clone(), bits: after.permission_bits });
        }

        if before.all_features_disabled != after.all_features_disabled {
            info!(is_disabled = after.all_features_disabled, "the phone's all-features-disabled state changed");
            self.emit(AppEvent::AllFeaturesDisabled { phone: phone.clone(), disabled: after.all_features_disabled });
        }

        let (b, a) = (&before.status_keys, &after.status_keys);
        let wrote = b.wifi_state != a.wifi_state || b.sync_over_metered_connection != a.sync_over_metered_connection;
        if wrote && (a.wifi_state == Some(true) || a.sync_over_metered_connection == Some(true)) {
            self.trigger(ConnectTrigger::StatusChanged);
        }

        self.emit(AppEvent::PhoneRecordChanged { phone });
    }

    /// Registers `factory` for each catalog `contentType` in `content_types`. Fails for a string
    /// outside the catalog (it would be classified as `test`), for `test`, and for a type that
    /// already has a factory.
    pub fn register_processor_factory(
        &self,
        content_types: &[&str],
        factory: Arc<dyn ContentProcessorFactory>,
    ) -> Result<(), AppError> {
        let mut map = self.factories.write_or_recover();
        let mut names: Vec<&'static str> = Vec::new();

        for ct in content_types {
            let Some(name) = content_type::CATALOG.iter().copied().find(|c| c == ct) else {
                return Err(AppError::Programming(format!(
                    "{ct:?} is not a catalog contentType; it would be classified as test"
                )));
            };
            if name == content_type::TEST {
                return Err(AppError::Programming("test has no processor".into()));
            }
            if map.contains_key(name) || names.contains(&name) {
                return Err(AppError::Programming(format!("{name} already has a processor factory")));
            }
            names.push(name);
        }

        for name in names {
            map.insert(name, factory.clone());
        }
        Ok(())
    }

    /// Registers the contributor of one feature provider slot; a slot takes one contributor.
    pub fn register_connect_contributor(
        &self,
        provider: ConnectProvider,
        contributor: Arc<dyn ConnectContributor>,
    ) -> Result<(), AppError> {
        let mut map = self.contributors.write_or_recover();
        if map.contains_key(&provider.number()) {
            return Err(AppError::Programming(format!(
                "connect provider {} already has a contributor",
                provider.name()
            )));
        }

        map.insert(provider.number(), (provider, contributor));
        Ok(())
    }

    pub fn register_request_observer(&self, observer: Arc<dyn RequestObserver>) {
        self.observers.write_or_recover().push(observer);
    }

    /// App-level events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<AppEvent> {
        self.events.subscribe()
    }

    /// Whether the app is in the foreground (`isForeground` of the trace state). Sets the state only
    /// (a later [`AppLayer::start`] reads it); the app's own foreground changes go through
    /// [`AppLayer::foreground_changed`].
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.store(foreground, Ordering::SeqCst);
    }

    /// The app came to the foreground (`true`) or left it (`false`): `isForeground` of the trace
    /// state follows, and coming to the foreground sends the immediate connect ("app came to the
    /// foreground" → app activation) — once the layer has started; before that the state is only
    /// recorded and [`AppLayer::start`] sends the connect. Reporting the state the app is already
    /// in changes nothing and sends nothing.
    pub fn foreground_changed(&self, foreground: bool) {
        let activate = {
            let _activation = self.activation.lock_or_recover();
            let was = self.foreground.swap(foreground, Ordering::SeqCst);
            foreground && !was && self.started.load(Ordering::SeqCst)
        };

        if activate {
            self.trigger(ConnectTrigger::AppActivated);
        }
    }

    pub fn is_foreground(&self) -> bool {
        self.foreground.load(Ordering::SeqCst)
    }

    /// The per-install `installationId` (upper-case GUID without braces).
    pub fn installation_id(&self) -> &str {
        &self.standard_keys.installation_id
    }

    /// The per-run app session id (upper-case GUID with braces; `permissionSessionId`).
    pub fn app_session_id(&self) -> &str {
        &self.app_session_id
    }

    /// The standard keys this layer writes into every request.
    pub fn standard_keys(&self) -> &StandardKeys {
        &self.standard_keys
    }

    /// The macOS lock test: locked or unknown counts as locked.
    pub(crate) fn is_locked(&self) -> bool {
        !matches!(self.edges.lock.lock_state(), LockState::Unlocked)
    }

    pub(crate) fn emit(&self, event: AppEvent) {
        // No receiver is not an error.
        let _ = self.events.send(event);
    }

    pub(crate) fn factory_for(&self, content_type: &str) -> Option<Arc<dyn ContentProcessorFactory>> {
        self.factories.read_or_recover().get(content_type).cloned()
    }

    /// The contributors in provider order.
    pub(crate) fn contributors_snapshot(&self) -> Vec<(ConnectProvider, Arc<dyn ConnectContributor>)> {
        self.contributors.read_or_recover().values().cloned().collect()
    }

    pub(crate) fn notify_request_observers(&self, phone: &DcgClient, route: &str, response: &AppResponse) {
        let observers = self.observers.read_or_recover().clone();
        for o in observers {
            o.on_request_completed(phone, route, response);
        }
    }

    pub(crate) fn runtime_handle(&self) -> Option<Handle> {
        self.runtime.get().cloned().or_else(|| Handle::try_current().ok())
    }
}
