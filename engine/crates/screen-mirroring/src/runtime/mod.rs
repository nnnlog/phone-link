//! The feature's runtime: the set-up every window waits for, the workflows' messages, the windows'
//! channel sets, state and teardown.

use phonelink_core::config::features::ScreenMirroringConfig;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, Weak};

use futures_util::future::{BoxFuture, Shared};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::edge::MirrorPresenter;
use super::errors::code;
use super::link::{Links, MirrorConnector, WorkflowStartError};
use super::machine::PhoneState;
use super::session::{Grouping, Session};
use super::store::{self};
use super::wire::{LockedState, ParseError};
use super::{NearbyPrompt, PackageAudio, ScreenMirroringEvent};
use phonelink_core::edge::LockState;
use phonelink_core::types::new_guid_lower;
use phonelink_features::RegistrationError;
use phonelink_features::shared::db::{PhoneStores, StoreLocation};
use phonelink_features::shared::notifier::{NotificationResponder, NotificationResponse};
use phonelink_features::{FeatureEnv, FeatureError};
use phonelink_nano::channel::message::{InboundMessage, MessageChannel};
use phonelink_nano::channel::{ChannelError, RawChannel};

mod audio;
mod launch;
mod nearby;
mod notices;
mod orchestrator;
mod phone_launch;
mod phone_values;
mod setup;
mod state_changes;
mod transactions;
mod window_actions;
mod windows;

/// The phone record's `DeviceAssetCategory` values the layout switch reads.
const ASSET_PHONE: u32 = 1;
const ASSET_TABLET: u32 = 2;

type SetupFuture = Shared<BoxFuture<'static, Result<(), SetupFailure>>>;

/// How the set-up ended when it did not complete. Every variant but `Closed` puts the phone state in
/// Error with its code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum SetupFailure {
    /// A step failed with this code.
    #[error("status {0}")]
    Error(i32),
    /// A workflow of the set-up did not start.
    #[error(transparent)]
    Start(#[from] WorkflowStartError),
    /// A step's message was not sent; it fails with `code`.
    #[error("{route} not sent: {error}")]
    NotSent {
        code: i32,
        route: &'static str,
        #[source]
        error: ChannelError,
    },
    /// The phone's answer to a step was malformed; it fails with `code`.
    #[error("malformed {route}: {error}")]
    Malformed {
        code: i32,
        route: &'static str,
        #[source]
        error: ParseError,
    },
    /// A precondition on the Mac is off ("allow apps"): the connection closes with the code.
    #[error("apps are not allowed on this Mac (status {0})")]
    Closed(i32),
}

impl SetupFailure {
    fn code(&self) -> i32 {
        match self {
            Self::Error(c) | Self::Closed(c) => *c,
            Self::Start(e) => e.code(),
            Self::NotSent { code, .. } | Self::Malformed { code, .. } => *code,
        }
    }
}

/// Where the shared set-up is, for the code a release that cancels it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SetupStage {
    #[default]
    Start,
    Nearby,
    Permission,
    Audio,
    Remote,
}

/// What a pending request waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transaction {
    /// `appResumableResult` on the orchestrator.
    AppResumable,
    /// Any other answer.
    Other,
}

/// What a phone-state change carries to the windows. A command without any of these changes the
/// phone state only when it moves it to another state.
#[derive(Debug, Clone, Default)]
struct PhoneArgs {
    error: Option<i32>,
    lock: Option<LockedState>,
    /// The nearby check's prompt changed.
    nearby: bool,
}

impl PhoneArgs {
    fn error(code: i32) -> Self {
        Self { error: Some(code), ..Default::default() }
    }

    fn lock(l: LockedState) -> Self {
        Self { lock: Some(l), ..Default::default() }
    }

    fn has_details(&self) -> bool {
        self.error.is_some() || self.lock.is_some() || self.nearby
    }
}

/// The nearby check's waits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct NearbySignal {
    unlock: bool,
    proceed: bool,
    failed: Option<i32>,
}

#[derive(Default)]
struct NearbyFlow {
    expiry_token: i32,
    expired: bool,
    verified: bool,
    local: bool,
    timer_epoch: u64,
    /// The check waits for the unlock (same network) or the verification (other network); a check
    /// done without waiting leaves it false, so a later `deviceLock` changes nothing.
    waiting: bool,
}

struct State {
    phone: PhoneState,
    phone_error: i32,
    phone_epoch: u64,
    /// The last phone state the windows were given, for windows opened later.
    last_fanout: (PhoneState, PhoneArgs),
    nearby: Option<NearbyPrompt>,
    nearby_flow: NearbyFlow,
    sessions: BTreeMap<String, Session>,
    next_order: u64,
    orchestrator: Option<Arc<MessageChannel>>,
    early: Option<Arc<MessageChannel>>,
    permission: Option<Arc<MessageChannel>>,
    nearby_channel: Option<Arc<MessageChannel>>,
    audio: Option<Arc<MessageChannel>>,
    /// The phone's frame length on the open `audio` workflow; its audio channels wait for it.
    frame_length: Option<u32>,
    held_audio: Vec<RawChannel>,
    /// Cancelled when the `audio` workflow ends: its open audio channels close.
    audio_stop: CancellationToken,
    /// Whether the Mac had an audio output when the phone was last told (`pcStreamingAllowed`);
    /// `None` while no `audio` workflow is open.
    audio_output: Option<bool>,
    package_audio: HashMap<String, PackageAudio>,
    /// The phone's last `permissionStatus` on the open permission workflow.
    permission_status: Option<String>,
    setup: Option<SetupFuture>,
    /// The window whose arrival started the running set-up.
    setup_requester: Option<String>,
    setup_cancel: CancellationToken,
    setup_stage: SetupStage,
    /// The code the set-up fails with when a release cancels it.
    setup_cancel_code: i32,
    /// Counts the releases: a launch whose set-up a release interrupted sends no `launchApp`.
    setup_epoch: u64,
    /// The `remote` workflow's start has begun (and the orchestrator has not closed since).
    remote_started: bool,
    /// The answers to the audio-permission questions, by scope and permission.
    audio_answers: HashMap<(&'static str, &'static str), String>,
    /// Cancels the running audio-permission flows (continue without audio, a failed early launch).
    audio_cancel: CancellationToken,
    /// Phone-initiated sessions by the phone's `transactionId`: (package, session id).
    served: HashMap<String, (String, String)>,
    transactions: HashMap<String, (Transaction, oneshot::Sender<Vec<u8>>)>,
    /// A notification to open on the phone the next time the phone-screen window becomes active.
    /// Every later launch of the phone screen replaces it (with nothing, unless it is a notification
    /// launch too).
    pending_notification: Option<(String, i32)>,
    /// The phone-screen window is active: it became connected, or gained the focus while connected.
    phone_screen_active: bool,
    /// Notification launches waiting for a result by key, for results that carry only the key.
    notification_keys: HashMap<String, String>,
    grouping: Grouping,
}

impl State {
    fn new() -> Self {
        Self {
            phone: PhoneState::Disconnected,
            phone_error: 0,
            phone_epoch: 0,
            last_fanout: (PhoneState::Disconnected, PhoneArgs::default()),
            nearby: None,
            nearby_flow: NearbyFlow::default(),
            sessions: BTreeMap::new(),
            next_order: 0,
            orchestrator: None,
            early: None,
            permission: None,
            nearby_channel: None,
            audio: None,
            frame_length: None,
            held_audio: Vec::new(),
            audio_stop: CancellationToken::new(),
            audio_output: None,
            package_audio: HashMap::new(),
            permission_status: None,
            setup: None,
            setup_requester: None,
            setup_cancel: CancellationToken::new(),
            setup_stage: SetupStage::Start,
            setup_cancel_code: code::CONNECTION_CANCELED,
            setup_epoch: 0,
            remote_started: false,
            audio_answers: HashMap::new(),
            audio_cancel: CancellationToken::new(),
            served: HashMap::new(),
            transactions: HashMap::new(),
            pending_notification: None,
            phone_screen_active: false,
            notification_keys: HashMap::new(),
            grouping: Grouping::default(),
        }
    }
}

/// Effects collected under the lock and run after it.
#[derive(Default)]
struct Out {
    events: Vec<ScreenMirroringEvent>,
    session_watch: Vec<(String, u64)>,
    phone_watch: Option<u64>,
    /// Sessions whose state changed (the stored mute may now apply to their button).
    changed: Vec<String>,
    /// A notification to open on the phone: the phone-screen window became active.
    notification: Option<(String, i32)>,
}

pub(crate) struct Inner {
    env: FeatureEnv,
    config: ScreenMirroringConfig,
    pub(super) events: broadcast::Sender<ScreenMirroringEvent>,
    presenter: Arc<dyn MirrorPresenter>,
    links: Arc<Links>,
    store: PhoneStores,
    state: Mutex<State>,
    nearby_signal: watch::Sender<NearbySignal>,
    permission_signal: watch::Sender<u64>,
    /// The answer that ended a prompt for (`oemAudio`, `audio`) in this permission workflow: a
    /// window whose own prompt was refused waits for one.
    audio_signal: watch::Sender<(Option<String>, Option<String>)>,
    /// One `earlyLaunch` start at a time.
    early_lock: tokio::sync::Mutex<()>,
    runtime: Handle,
    /// The folder phone → Mac drags receive their files in, one subfolder per window.
    drag_staging: PathBuf,
    /// The settings, or an app's data, failed to read since the last read that succeeded: the
    /// failure was logged, and the next ones are not.
    settings_unread: AtomicBool,
    package_unread: AtomicBool,
    shutdown: CancellationToken,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.links.close(code::WINDOWS_CLOSED);
    }
}

/// The responder of the drag notifications (`screen_mirroring/drag/<session>`): Cancel cancels the
/// window's drag of phone files.
struct DragResponder(Weak<Inner>);

impl NotificationResponder for DragResponder {
    fn on_response(&self, id: &str, _user_info: &HashMap<String, String>, response: NotificationResponse) {
        let Some(inner) = self.0.upgrade() else { return };
        let Some(session) = id.strip_prefix(store::FEATURE).and_then(|r| r.strip_prefix("/drag/")) else {
            // The drag notices (`screen_mirroring/dragNotAccepted/…`) take no action.
            return debug!(id, "mirroring notification response without an action; ignored");
        };

        match response {
            NotificationResponse::Action { action_id } if action_id == super::drag::ACTION_CANCEL => {
                match inner.drag(session) {
                    Ok(d) => d.cancel_from_notification(),
                    Err(e) => debug!(error = %e, "drag cancel for a closed window; ignored"),
                }
            }
            NotificationResponse::Activated | NotificationResponse::Dismissed => {}
            other => warn!(id, response_kind = ?other, "drag notification response not expected; ignored"),
        }
    }
}

/// One inbound message with the registered scope it was delivered to. The channel delivers a
/// message to the longest registered scope its target starts with, so the target can be longer
/// (`/a/b/c` to `/a/b`). Most handlers act only on a target equal to their scope; the ones that act
/// on whatever their scope receives match on `scope`.
struct Scoped {
    scope: String,
    m: InboundMessage,
}

/// One merged stream of a channel's handled scopes; it ends once the channel closed (`closed`)
/// and what it had received is delivered.
fn merged(
    inbound: HashMap<String, mpsc::UnboundedReceiver<InboundMessage>>,
    closed: watch::Receiver<bool>,
) -> mpsc::UnboundedReceiver<Scoped> {
    let (tx, rx) = mpsc::unbounded_channel();
    for (scope, mut r) in inbound {
        let tx = tx.clone();
        let mut closed = closed.clone();
        tokio::spawn(async move {
            loop {
                let m = tokio::select! {
                    biased;
                    m = r.recv() => m,
                    _ = closed.wait_for(|c| *c) => r.try_recv().ok(),
                };

                let Some(m) = m else { break };
                if tx.send(Scoped { scope: scope.clone(), m }).is_err() {
                    break;
                }
            }
        });
    }
    rx
}

fn send_on(channel: &Option<Arc<MessageChannel>>, target: &str, body: &[u8]) -> bool {
    match channel {
        Some(c) => match c.send(target, body) {
            Ok(()) => true,
            Err(e) => {
                warn!(error = %e, route = target, "message not sent");
                false
            }
        },
        None => {
            debug!(route = target, "message not sent; its channel is not open");
            false
        }
    }
}

impl Inner {
    pub(super) fn install(
        env: &FeatureEnv,
        config: ScreenMirroringConfig,
        connector: Arc<dyn MirrorConnector>,
        presenter: Arc<dyn MirrorPresenter>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current().map_err(|_| {
            FeatureError::Programming("screen mirroring must be installed inside a tokio runtime".into())
        })?;

        let (notices_tx, notices) = mpsc::unbounded_channel();
        let lock_edge = env.edges.lock.clone();
        let mac_locked: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || lock_edge.lock_state() == LockState::Locked);
        let links = Links::new(connector, config.link, notices_tx, mac_locked, runtime.clone());

        // Phone files dragged to the Mac are received under the stores' folder, where files an earlier
        // run left behind are removed; with in-memory stores, in a folder of this run's own.
        let drag_staging = match &env.stores {
            StoreLocation::Directory(dir) => {
                let root = dir.join("drag-staging");
                if let Err(e) = std::fs::remove_dir_all(&root)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    warn!(error = %e, "earlier drag files not removed");
                }
                root
            }
            StoreLocation::Memory => std::env::temp_dir().join("PhoneLink-drag").join(new_guid_lower()),
        };

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            events: broadcast::channel(256).0,
            presenter,
            links,
            store: PhoneStores::new(env.stores.clone(), store::FEATURE, store::MIGRATIONS),
            state: Mutex::new(State::new()),
            nearby_signal: watch::channel(NearbySignal::default()).0,
            permission_signal: watch::channel(0).0,
            audio_signal: watch::channel((None, None)).0,
            early_lock: tokio::sync::Mutex::new(()),
            runtime,
            drag_staging,
            settings_unread: AtomicBool::new(false),
            package_unread: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
            weak: weak.clone(),
        });

        inner.spawn_notices(notices);
        inner.spawn_phone_launches(env.app.subscribe_events());
        env.responses
            .register(store::FEATURE, Arc::new(DragResponder(Arc::downgrade(&inner))))
            .map_err(RegistrationError::from)?;
        Ok(inner)
    }

    fn spawn(&self, f: impl Future<Output = ()> + Send + 'static) {
        let shutdown = self.shutdown.clone();
        self.runtime.spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {}
                _ = f => {}
            }
        });
    }

    fn me(&self) -> Arc<Inner> {
        self.weak.upgrade().expect("the feature is alive while it runs")
    }
}

/// What the phone-screen window does once connected.
pub(super) enum PendingPhoneScreen {
    App(String),
    Notification(String, i32),
}
