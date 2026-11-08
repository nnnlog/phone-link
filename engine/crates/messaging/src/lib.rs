//! Messaging — SMS, MMS and RCS, on the Android path.
//!
//! | Module | Implements |
//! |---|---|
//! | [`consts`] | the message catalog, identity, timestamps and enumerations, the send response's `SendMessageFailureReason` |
//! | [`caps`] | gating (`MessagingCapabilities`, the predicates, the value before any report) |
//! | [`parse`] | metadata and item messages, error handling (validation before any change) |
//! | [`store`] | the local state the protocol depends on, delta computation, sync anchors (the app's own store) |
//! | `exchange` | the exchange model, delta computation, the metadata response, item processing and response, sync anchors, participants |
//! | `connect` | the contribution to the connect message |
//! | `outbound` | the keys common to every `/chat/v1` request, the sync request, the media download request, mark conversation as read, the messages permission request |
//! | `send` | the send request, composition rules, the send response |
//! | `notify` | new-message notifications (macOS: `UserNotifications` with a text-input reply) |
//! | `subject` | the text of an MMS subject, decoded from the bytes and character set the phone sends |
//! | [`views`] | what the UI shows: conversations, messages, parts and their bytes, pending sends (those to a new conversation with their addresses), the new-message composer's limits, sync state |
//! | [`edge`] | the feature's OS edges: `ImageIO` imaging, `UTType` lookups, `IOBluetooth` proximity |
//!
//! [`Messaging::install`] registers the `messaging` content-processor factory, the connect provider
//! `Messaging` and the notification responder for `messaging/…`; the handle it returns is the
//! UI's API and also [`MessagingAlerts`] for the notifications feature.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod caps;
pub mod consts;
pub mod edge;
pub mod parse;
pub mod store;
pub mod views;

mod connect;
mod exchange;
mod notify;
mod outbound;
mod send;
mod subject;

use phonelink_core::config::features::MessagingConfig;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::UNIX_EPOCH;

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use phonelink_app::{AppEvent, AppLayer, ConnectProvider, PhoneRecord, consts::permission};
use phonelink_core::config::AppConfig;
use phonelink_core::edge::{Clock, HostInfo};
use phonelink_core::seams::PartnerSessionControl;
use phonelink_core::types::DcgClient;

use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError};
use phonelink_features::shared::ongoing::OngoingFailure;
use phonelink_features::{ContactDirectory, FeatureEnv, FeatureError, MessagingAlerts, SystemNotifier};

pub use caps::Predicates;
pub use consts::SendMessageFailureReason;
pub use edge::{AttachmentImaging, MessagingFileTypes, PhoneProximity, ProximityPhone, ProximityState};
#[cfg(feature = "test-support")]
pub use edge::{FileFamily, FittedImage};
pub use outbound::PartRef;
use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;
pub use send::{Attachment, SendOutcome, SendTarget};
pub use store::{PcSettings, SendStatus};
#[cfg(feature = "test-support")]
pub use store::{PendingSend, SendOrigin, SentPart};
pub use views::{
    ConversationPreview, ConversationView, MessageKind, MessageView, NewConversationView, NewMessageLimits, PartState,
    PartView, Recipient, SyncState,
};

/// The feature's own OS edges.
#[derive(Clone)]
pub struct MessagingEdges {
    pub imaging: Arc<dyn AttachmentImaging>,
    pub file_types: Arc<dyn MessagingFileTypes>,
    pub proximity: Arc<dyn PhoneProximity>,
}

/// What the UI re-reads on.
#[derive(Debug, Clone, PartialEq)]
pub enum MessagingEvent {
    /// Conversations or their messages changed; `threads` are the thread ids (as the UI shows them)
    /// touched, empty when the UI should re-read every conversation.
    ContentChanged { phone: DcgClient, threads: Vec<i64> },
    /// A pending send was created, changed state, or was acked (then it is gone).
    SendChanged { phone: DcgClient, send_id: i64 },
    /// The up-to-date or syncing state changed.
    SyncStateChanged { phone: DcgClient, state: SyncState },
    /// `MessagingCapabilities`, "MMS auto-download on phone" or the PC settings changed.
    SettingsChanged { phone: DcgClient },
    /// The phone messages permission changed.
    PermissionChanged { phone: DcgClient, granted: bool },
    /// The user clicked a message notification: show this conversation.
    OpenConversation { phone: DcgClient, thread_id: i64 },
}

/// Failures of the UI-facing operations (inbound traffic never produces one).
#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("the App layer is gone")]
    AppGone,
    #[error("App layer: {0}")]
    App(#[from] phonelink_app::AppError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("no conversation {0}")]
    UnknownConversation(i64),
    #[error("no pending send {0}")]
    UnknownSend(i64),
    #[error("no MMS part or RCS file {0}")]
    UnknownPart(i64),
    #[error("the part is not offered for download")]
    NotDownloadable,
    #[error("nothing to send: no text and no attachment")]
    NothingToSend,
    #[error("at most {max} attachments per message")]
    TooManyAttachments { max: usize },
    #[error("an attachment's content type is unknown and cannot be detected")]
    UnknownAttachmentType,
    /// Rule 5: a message carries images (GIFs included) only; nothing was stored or sent.
    #[error("an attachment of type {content_type} is not an image")]
    NotAnImage { content_type: String },
    #[error("sending is not possible here: {0}")]
    NotAllowed(&'static str),
    /// A retry was asked for a message that did not fail (rule 1: a retry is for a failed message).
    #[error("pending send {0} has not failed")]
    NotFailed(i64),
    /// An OS-edge call panicked; nothing was sent.
    #[error(transparent)]
    EdgeFailed(EdgeFailure),
}

/// An OS-edge call that panicked: which edge, and the panic.
#[derive(Debug, thiserror::Error)]
#[error("the {edge} edge failed: {source}")]
pub struct EdgeFailure {
    pub edge: &'static str,
    #[source]
    pub source: tokio::task::JoinError,
}

impl From<rusqlite::Error> for MessagingError {
    fn from(e: rusqlite::Error) -> Self {
        MessagingError::Store(e.into())
    }
}

/// The syncing state of one phone (the UI shows a syncing state for at most 2 minutes while not up
/// to date; the same flag also marks a running permission-flow sync).
#[derive(Debug, Default)]
struct SyncTracking {
    /// The last up-to-date value seen (`None`: not evaluated yet this run).
    up_to_date: Option<bool>,
    /// The messages UI shows its syncing state.
    updating: bool,
    /// Bumped on every new wait so an older 2-minute timer does nothing.
    generation: u64,
}

/// Mark-as-read triggers: the UI's selection, view and window focus, and the pending delayed
/// trigger.
#[derive(Debug, Default)]
struct UiState {
    selected: Option<i64>,
    messages_view_active: bool,
    /// The app window is focused, as the UI last reported it ([`Messaging::on_window_focused`]).
    window_focused: bool,
    delayed: Option<CancellationToken>,
}

/// In-memory state of this app run.
#[derive(Default)]
struct RunState {
    /// Phones whose store was opened (and its phone-state row ensured) in this run.
    opened: HashSet<DcgClient>,
    /// The `MessagingCapabilities` of a phone that has reported none, fixed for this run when its
    /// store was first opened.
    default_caps: HashMap<DcgClient, u32>,
    /// RCS receive / RCS send as last seen, for sync trigger (b).
    rcs_predicates: HashMap<DcgClient, (bool, bool)>,
    sync: HashMap<DcgClient, SyncTracking>,
    ui: UiState,
    /// Parts with a media download in flight.
    downloading: HashSet<(DcgClient, outbound::Media)>,
}

pub(crate) struct Inner {
    config: MessagingConfig,
    app_config: AppConfig,
    app: Weak<AppLayer>,
    sessions: Arc<dyn PartnerSessionControl>,
    clock: Arc<dyn Clock>,
    host: Arc<dyn HostInfo>,
    notifier: Arc<dyn SystemNotifier>,
    contacts: Option<Arc<dyn ContactDirectory>>,
    edges: MessagingEdges,
    stores: PhoneStores,
    events: broadcast::Sender<MessagingEvent>,
    run: Mutex<RunState>,
    /// Held while a phone's store is opened for the first time in this run ([`Inner::db`]).
    first_open: Mutex<()>,
    /// The runtime `install` ran in: every later task (timers, triggers, replies) is spawned on it,
    /// whichever thread asks.
    runtime: Handle,
    /// Cancelled when the [`Messaging`] handle is dropped: stops the feature's background tasks.
    shutdown: CancellationToken,
    /// The PC settings cannot be read where each incoming message reads them (its notification,
    /// the group-message alerts).
    settings_failing: OngoingFailure,
    /// The up-to-date state cannot be read (each processed sync reads it).
    sync_state_failing: OngoingFailure,
    /// Message notifications cannot be posted (each incoming message tries).
    post_failing: OngoingFailure,
    /// New-message notifications and offered codes waiting for each other.
    code_pairing: Mutex<notify::CodePairing>,
}

impl Inner {
    fn app(&self) -> Result<Arc<AppLayer>, MessagingError> {
        self.app.upgrade().ok_or(MessagingError::AppGone)
    }

    /// The selected phone and its record.
    fn selected(&self) -> Result<(Arc<AppLayer>, DcgClient, PhoneRecord), MessagingError> {
        let app = self.app()?;
        let phone = app.selected_phone().ok_or(MessagingError::NoPhone)?;
        let record = app.phone_record().ok_or(MessagingError::NoPhone)?;
        Ok((app, phone, record))
    }

    /// `phone`'s store. The first open in this run ensures its phone-state row, marks the sends an
    /// earlier run left sending as failed (differs from Phone Link on Windows: no outcome can arrive
    /// for them any more, and a failed send offers the retry), and fixes the run's default
    /// `MessagingCapabilities`: `0` when the store is created now (the pairing run), else
    /// from the phone's contract version as its record holds it now.
    fn db(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, StoreError> {
        let db = self.stores.get(phone)?;

        if self.run.lock_or_panic().opened.contains(phone) {
            return Ok(db);
        }

        // One first open at a time: no send of this run can be stored before it marked the earlier
        // run's sends, since storing one needs this function to have returned.
        let _first = self.first_open.lock_or_panic();
        if self.run.lock_or_panic().opened.contains(phone) {
            return Ok(db);
        }

        let (created, interrupted) = db.transaction(|tx| {
            let created = store::ensure_phone_state(tx)?;
            Ok::<_, StoreError>((created, store::fail_interrupted_sends(tx)?))
        })?;

        if !interrupted.is_empty() {
            warn!(send_count = interrupted.len(), "message sends an earlier run left sending marked failed");
        }

        let contract = self
            .app
            .upgrade()
            .and_then(|a| a.phone_record())
            .filter(|r| r.phone() == *phone)
            .map(|r| self.contract_version(&r));

        let mut run = self.run.lock_or_panic();
        run.opened.insert(phone.clone());
        // Without the phone's record (not the selected phone) the default is fixed by the first
        // `effective_caps`, which has the record.
        if created || contract.is_some() {
            let default = caps::default_at_load(created, contract.unwrap_or_default());
            run.default_caps.insert(phone.clone(), default);
        }

        Ok(db)
    }

    /// The phone's contract version (`3.99` before any connect report).
    fn contract_version(&self, record: &PhoneRecord) -> f64 {
        record.contract_version(&self.app_config)
    }

    /// The `MessagingCapabilities` in force for `phone` given its stored value: the stored value, else
    /// the run's default for the phone.
    fn effective_caps(&self, phone: &DcgClient, stored: Option<u32>, record: &PhoneRecord) -> u32 {
        if let Some(v) = stored {
            return v;
        }
        let contract = self.contract_version(record);
        let mut run = self.run.lock_or_panic();
        let default = *run.default_caps.entry(phone.clone()).or_insert_with(|| caps::default_at_load(false, contract));
        caps::effective(stored, default)
    }

    /// The gating predicates for `phone` as stored now.
    fn predicates(&self, phone: &DcgClient, record: &PhoneRecord) -> Result<Predicates, StoreError> {
        let state = self.db(phone)?.read(store::phone_state)?;
        Ok(Predicates::new(self.effective_caps(phone, state.capabilities, record), self.contract_version(record)))
    }

    fn emit(&self, event: MessagingEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    /// Wall-clock now, Unix ms.
    fn now_ms(&self) -> i64 {
        match self.clock.now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
        }
    }

    /// The phone's messages permission (`DevicePermissions.Messages`, default granted).
    fn phone_permission(record: &PhoneRecord) -> bool {
        record.permission_or(permission::MESSAGES, true)
    }

    /// Sync trigger (b) after a `MessagingCapabilities` change: when RCS receive or RCS send changed
    /// value against what was last seen, a sync request (not user-initiated) is sent.
    fn after_capabilities_change(self: &Arc<Self>, phone: &DcgClient, before: Predicates, after: Predicates) {
        self.emit(MessagingEvent::SettingsChanged { phone: phone.clone() });

        let now = (after.rcs_receive(), after.rcs_send());
        let changed = {
            let mut run = self.run.lock_or_panic();
            let seen = run.rcs_predicates.entry(phone.clone()).or_insert((before.rcs_receive(), before.rcs_send()));
            let changed = *seen != now;
            *seen = now;
            changed
        };

        if changed {
            self.spawn_sync_request(false, "MessagingRcsCapabilitiesChanged");
        }
    }

    /// Re-evaluates the up-to-date state after anchors changed; a change starts a new wait: the
    /// syncing state shows while not up to date, for at most 2 minutes.
    fn refresh_sync_state(self: &Arc<Self>, phone: &DcgClient) {
        let up_to_date = match self.db(phone).and_then(|db| db.read(store::is_up_to_date)) {
            Ok(v) => {
                self.sync_state_failing.clear();
                v
            }
            Err(e) => {
                if self.sync_state_failing.starts() {
                    error!(error = %e, "messaging up-to-date state not read; the syncing state is unchanged");
                }
                return;
            }
        };

        let (generation, state) = {
            let mut run = self.run.lock_or_panic();
            let t = run.sync.entry(phone.clone()).or_default();
            if t.up_to_date == Some(up_to_date) {
                return;
            }
            t.up_to_date = Some(up_to_date);
            t.updating = !up_to_date;
            t.generation += 1;
            (t.generation, SyncState { up_to_date, syncing: t.updating })
        };

        self.emit(MessagingEvent::SyncStateChanged { phone: phone.clone(), state });

        if !up_to_date {
            let inner = Arc::clone(self);
            let phone = phone.clone();
            let limit = self.config.syncing_display_limit;
            self.runtime.spawn(async move {
                tokio::time::sleep(limit).await;
                inner.end_wait(&phone, generation);
            });
        }
    }

    /// The 2-minute limit of one wait ended.
    fn end_wait(&self, phone: &DcgClient, generation: u64) {
        let state = {
            let mut run = self.run.lock_or_panic();
            let Some(t) = run.sync.get_mut(phone) else { return };
            if t.generation != generation || !t.updating {
                return;
            }
            t.updating = false;
            SyncState { up_to_date: t.up_to_date.unwrap_or(true), syncing: false }
        };

        self.emit(MessagingEvent::SyncStateChanged { phone: phone.clone(), state });
    }

    /// Sets the syncing flag (the permission-flow sync); `false` when it was already set.
    fn try_begin_updating(&self, phone: &DcgClient) -> bool {
        let mut run = self.run.lock_or_panic();
        let t = run.sync.entry(phone.clone()).or_default();
        if t.updating {
            return false;
        }
        t.updating = true;
        t.generation += 1;
        true
    }

    fn end_updating(&self, phone: &DcgClient) {
        let state = {
            let mut run = self.run.lock_or_panic();
            let t = run.sync.entry(phone.clone()).or_default();
            t.updating = false;
            t.generation += 1;
            SyncState { up_to_date: t.up_to_date.unwrap_or(true), syncing: false }
        };

        self.emit(MessagingEvent::SyncStateChanged { phone: phone.clone(), state });
    }

    fn sync_state(&self, phone: &DcgClient) -> Result<SyncState, StoreError> {
        let up_to_date = self.db(phone)?.read(store::is_up_to_date)?;
        let syncing = self.run.lock_or_panic().sync.get(phone).is_some_and(|t| t.updating);
        Ok(SyncState { up_to_date, syncing })
    }
}

/// The messaging feature: the UI's API and [`MessagingAlerts`]. Dropping it stops the feature's
/// background tasks.
pub struct Messaging {
    inner: Arc<Inner>,
}

impl Drop for Messaging {
    fn drop(&mut self) {
        self.inner.shutdown.cancel();
    }
}

impl Messaging {
    /// Installs the feature on `env.app`: the `messaging` processor factory, the connect provider
    /// `Messaging`, the `messaging/…` notification responder and the permission-event forwarder.
    /// `contacts` resolves addresses to names and runs the `/contacts/v2` sync of the permission flow
    /// (`None`: names are the addresses and that sync is skipped). Must run inside a tokio runtime:
    /// the feature's later tasks run on it, whichever thread triggers them.
    pub fn install(
        env: &FeatureEnv,
        config: MessagingConfig,
        contacts: Option<Arc<dyn ContactDirectory>>,
        edges: MessagingEdges,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("messaging must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new(Inner {
            config,
            app_config: env.config.app.clone(),
            app: Arc::downgrade(&env.app),
            sessions: env.sessions.clone(),
            clock: env.edges.clock.clone(),
            host: env.edges.host.clone(),
            notifier: env.notifier.clone(),
            contacts,
            edges,
            stores: PhoneStores::new(env.stores.clone(), consts::FEATURE, store::MIGRATIONS),
            events: broadcast::channel(256).0,
            run: Mutex::new(RunState::default()),
            first_open: Mutex::new(()),
            runtime,
            shutdown: CancellationToken::new(),
            settings_failing: OngoingFailure::default(),
            sync_state_failing: OngoingFailure::default(),
            post_failing: OngoingFailure::default(),
            code_pairing: Mutex::new(notify::CodePairing::default()),
        });

        env.app.register_processor_factory(&[consts::CONTENT_TYPE], Arc::new(exchange::Factory::new(&inner)))?;
        env.app
            .register_connect_contributor(ConnectProvider::Messaging, Arc::new(connect::Contributor::new(&inner)))?;
        env.responses
            .register(consts::FEATURE, Arc::new(notify::Responder::new(&inner)))
            .map_err(RegistrationError::from)?;

        // The phone messages permission lives in the phone record; announce its changes.
        let mut app_events = env.app.subscribe_events();
        let weak = Arc::downgrade(&inner);
        let shutdown = inner.shutdown.clone();
        inner.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = app_events.recv() => event,
                };
                match event {
                    Ok(AppEvent::PermissionsChanged { phone, .. }) => {
                        let Some(inner) = weak.upgrade() else { break };
                        let Some(record) = inner.app.upgrade().and_then(|a| a.phone_record()) else { continue };
                        if record.phone() == phone {
                            let granted = Inner::phone_permission(&record);
                            inner.emit(MessagingEvent::PermissionChanged { phone, granted });
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "messaging missed App events; a permission change may not show");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        // The selected phone's store and syncing state.
        if let Some(phone) = env.app.selected_phone() {
            inner.db(&phone)?;
            if let Some(record) = env.app.phone_record() {
                let p = inner.predicates(&phone, &record)?;
                inner.run.lock_or_panic().rcs_predicates.insert(phone.clone(), (p.rcs_receive(), p.rcs_send()));
            }
            inner.refresh_sync_state(&phone);
        }

        Ok(Arc::new(Self { inner }))
    }

    /// Events the UI re-reads on.
    pub fn subscribe(&self) -> broadcast::Receiver<MessagingEvent> {
        self.inner.events.subscribe()
    }

    /// This handle as the [`MessagingAlerts`] the notifications feature reads.
    pub fn alerts(self: &Arc<Self>) -> Arc<dyn MessagingAlerts> {
        self.clone()
    }

    /// The selected phone's `MessagingCapabilities` and contract version, as the predicates read them.
    pub fn predicates(&self) -> Result<Predicates, MessagingError> {
        let (_, phone, record) = self.inner.selected()?;
        Ok(self.inner.predicates(&phone, &record)?)
    }

    /// The phone messages permission (default granted).
    pub fn phone_permission(&self) -> Result<bool, MessagingError> {
        let (_, _, record) = self.inner.selected()?;
        Ok(Inner::phone_permission(&record))
    }

    /// The PC settings of the selected phone.
    pub fn pc_settings(&self) -> Result<PcSettings, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::phone_state)?.settings)
    }

    /// Changes the PC settings (receive MMS, auto-download MMS media, send MMS, consent, message
    /// notifications). No request follows: the sync request reads them when it is next sent.
    pub fn set_pc_settings(&self, settings: PcSettings) -> Result<(), MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        self.inner.db(&phone)?.transaction(|tx| store::set_pc_settings(tx, settings))?;
        info!(settings_state = ?settings, "messaging settings on this Mac switched");
        self.inner.emit(MessagingEvent::SettingsChanged { phone });
        Ok(())
    }

    /// "MMS auto-download on phone" as the phone last reported it.
    pub fn mms_auto_download_on_phone(&self) -> Result<bool, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::phone_state)?.mms_auto_download_on_phone)
    }

    /// Up to date / syncing.
    pub fn sync_state(&self) -> Result<SyncState, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.sync_state(&phone)?)
    }

    /// Unix ms of the last completed exchange.
    pub fn last_update_ms(&self) -> Result<Option<i64>, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::phone_state)?.last_update_ms)
    }

    /// Messaging was set up once on this phone (a sync, send or prompt was granted).
    pub fn setup_completed_once(&self) -> Result<bool, MessagingError> {
        let (_, phone, _) = self.inner.selected()?;
        Ok(self.inner.db(&phone)?.read(store::phone_state)?.setup_completed_once)
    }

    /// The Bluetooth state the messages view shows as an error while the phone requires a Bluetooth
    /// Classic connection for sending (`SendRequiresProximity`, `0x80`); `None` when it does not.
    pub fn proximity_state(&self) -> Result<Option<ProximityState>, MessagingError> {
        let (_, phone, record) = self.inner.selected()?;
        if !self.inner.predicates(&phone, &record)?.requires_proximity() {
            return Ok(None);
        }
        let names = [record.name, record.display_name].into_iter().flatten().collect();
        Ok(Some(self.inner.edges.proximity.state(ProximityPhone { dcg_client_id: phone.id.clone(), names })))
    }

    /// Mutes or unmutes, hides or shows a conversation (notifications skip muted and hidden ones).
    pub fn set_conversation_flags(&self, thread_id: i64, muted: bool, hidden: bool) -> Result<(), MessagingError> {
        let (_, phone, _) = self.inner.selected()?;

        self.inner.db(&phone)?.transaction(|tx| {
            let mut a = store::conversation_attr(tx, thread_id)?;
            a.muted = muted;
            a.hidden = hidden;
            store::set_conversation_attr(tx, thread_id, a)
        })?;

        self.inner.emit(MessagingEvent::ContentChanged { phone, threads: vec![thread_id] });
        Ok(())
    }

    /// The user's SIM choice for a conversation (`None`: none), composition rule 4.
    pub fn select_subscription(&self, thread_id: i64, subscription_id: Option<i64>) -> Result<(), MessagingError> {
        let (_, phone, _) = self.inner.selected()?;

        self.inner.db(&phone)?.transaction(|tx| {
            let mut a = store::conversation_attr(tx, thread_id)?;
            a.selected_subscription = subscription_id;
            store::set_conversation_attr(tx, thread_id, a)
        })?;

        self.inner.emit(MessagingEvent::ContentChanged { phone, threads: vec![thread_id] });
        Ok(())
    }
}

impl MessagingAlerts for Messaging {
    /// The PC consent and the message-notification setting of `phone`.
    fn group_message_alerts_allowed(&self, phone: &DcgClient) -> bool {
        match self.inner.db(phone).and_then(|db| db.read(store::phone_state)) {
            Ok(s) => {
                self.inner.settings_failing.clear();
                s.settings.pc_consent && s.settings.message_notifications
            }
            Err(e) => {
                if self.inner.settings_failing.starts() {
                    error!(error = %e, "messaging settings not read; message alerts are not shown");
                }
                false
            }
        }
    }

    fn copy_code_offered(&self, phone: &DcgClient, code: &str) {
        notify::code_offered(&self.inner, phone, code);
    }
}
