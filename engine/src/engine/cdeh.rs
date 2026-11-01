//! The CDEH identity: its part of the start, and its session to the selected phone — the changes run
//! one at a time, and what triggered them.

use super::{EngineCore, EngineError, FetchMode, IdentityStack, auth_err};
use crate::auth::AuthService;
use crate::edge::LockState;
use crate::features::shared::db::StoreError;
use crate::platform::version::client_version_passes;
use crate::platform::{PartnerSessions, PlatformRoutes};
use crate::seams::{PartnerSessionControl, StartPolicy};
use crate::sync::MutexExt;
use crate::types::{DcgClient, TraceContext};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

impl EngineCore {
    /// The CDEH identity's part of the start: its sign-in, provisioning and platform routes, and
    /// its trust synchronization. Both identities must be one Microsoft account, as on Windows. A
    /// displayed phone missing from the CDEH identity's linked devices leaves the CDEH features
    /// without a session until a refresh lists it.
    pub(super) async fn start_cdeh(&self, cdeh: &IdentityStack) -> Result<(), EngineError> {
        cdeh.provision(&self.edges).await?;

        let primary = self.auth.local_account().and_then(|a| a.puid);
        let secondary = cdeh.auth.local_account().and_then(|a| a.puid);
        if let (Some(a), Some(b)) = (&primary, &secondary) {
            if a != b {
                return Err(auth_err(
                    "the Phone Link and Cross Device sign-ins are different Microsoft accounts; sign in to both with one",
                ));
            }
        }

        cdeh.auth.sync_trust(FetchMode::Default).await.map_err(auth_err)?;
        Ok(())
    }

    /// Starts the CDEH identity's session to the selected phone in the background for `trigger`,
    /// unless the Mac is locked — its unlock starts it. Nothing without a CDEH identity.
    pub(super) fn connect_cdeh(&self, trigger: CdehTrigger) {
        if !matches!(self.edges.lock.lock_state(), LockState::Unlocked) {
            self.submit_cdeh_change(SessionChange::Skip(Refusal::expected("the Mac is locked")));
            return;
        }
        self.start_cdeh_session(trigger);
    }

    /// The start CDEH's session service makes for a user-initiated trigger: policy `NewSession`,
    /// ignoring the device-disabled status, bounded by the session maximum wait, its trace state naming
    /// the scenario `StartSession` and the trigger.
    pub(super) fn new_session_request(&self, trigger: CdehTrigger) -> ConnectRequest {
        ConnectRequest {
            policy: StartPolicy::NewSession,
            ignore_device_disabled: true,
            feedback_window: self.config.platform.session_maximum_wait,
            trace: TraceContext::new_root(vec![
                (crate::types::trace_key::SCENARIO.to_string(), "StartSession".to_string()),
                (crate::types::trace_key::TRIGGER.to_string(), trigger.trigger_id().to_string()),
            ]),
        }
    }

    /// Why a start of the CDEH session to `phone` starts nothing, if it does: the phone is not in the
    /// CDEH identity's linked devices, its Link to Windows is older than the minimum version, or its
    /// per-phone switch is off (or cannot be read).
    pub(super) fn cdeh_start_refusal(&self, cdeh: &IdentityStack, phone: &DcgClient) -> Option<Refusal> {
        let Some(peer) = cdeh.auth.peers().into_iter().find(|p| p.client.is_same_client(phone)) else {
            return Some(Refusal::expected("the phone is not a linked device of the CDEH identity"));
        };
        if !client_version_passes(peer.info.client_version.as_deref(), self.minimum_client_version) {
            return Some(Refusal::expected("the phone's Link to Windows is older than the minimum version"));
        }
        match self.switches.is_enabled(phone) {
            Ok(true) => None,
            Ok(false) => Some(Refusal::expected("the phone is disabled")),
            Err(e) => Some(Refusal { reason: "the per-phone switch could not be read", failure: Some(e) }),
        }
    }

    /// Starts the CDEH identity's session to the selected phone in the background for `trigger`
    /// ([`EngineCore::new_session_request`]), or records a start that starts nothing
    /// ([`EngineCore::cdeh_start_refusal`]). Nothing without a CDEH identity.
    pub(super) fn start_cdeh_session(&self, trigger: CdehTrigger) {
        let (Some(cdeh), Some(phone)) = (self.cdeh.as_ref(), self.phone()) else { return };
        let change = match self.cdeh_start_refusal(cdeh, &phone) {
            Some(reason) => SessionChange::Skip(reason),
            None => SessionChange::Start { request: self.new_session_request(trigger), trigger },
        };
        self.cdeh_changes.submit(cdeh.sessions.clone(), phone, change);
    }

    /// Stops the CDEH identity's session to the selected phone in the background (a local
    /// disconnect), as CDEH does when the session locks, the phone is disabled or leaves the CDEH
    /// identity's linked devices. Nothing without a CDEH identity.
    pub(super) fn stop_cdeh_session(&self) {
        self.submit_cdeh_change(SessionChange::Stop);
    }

    pub(super) fn submit_cdeh_change(&self, change: SessionChange) {
        let (Some(cdeh), Some(phone)) = (self.cdeh.as_ref(), self.phone()) else { return };
        self.cdeh_changes.submit(cdeh.sessions.clone(), phone, change);
    }

    /// The stack that carries the CDEH features: the CDEH identity's, else the primary one's parts.
    pub(super) fn cdeh_parts(&self) -> (Arc<AuthService>, Arc<PartnerSessions>, Option<Arc<PlatformRoutes>>) {
        match &self.cdeh {
            Some(s) => (s.auth.clone(), s.sessions.clone(), s.routes()),
            None => (self.auth.clone(), self.sessions.clone(), self.routes()),
        }
    }
}

use crate::seams::ConnectRequest;

/// A change of one identity's session to the selected phone.
pub(super) enum SessionChange {
    Start {
        request: ConnectRequest,
        trigger: CdehTrigger,
    },
    /// A start that returns without starting; it still supersedes the changes before it.
    Skip(Refusal),
    Stop,
}

impl SessionChange {
    /// What the change is, for the log.
    fn kind(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::Skip(_) => "skip",
            Self::Stop => "stop",
        }
    }

    /// The start's `ypTriggerId`, for the log.
    fn trigger_id(&self) -> Option<&'static str> {
        match self {
            Self::Start { trigger, .. } => Some(trigger.trigger_id()),
            _ => None,
        }
    }
}

/// Why a session start starts nothing, for the log: a state the code expects, or a failure.
pub(super) struct Refusal {
    reason: &'static str,
    failure: Option<StoreError>,
}

impl Refusal {
    fn expected(reason: &'static str) -> Self {
        Self { reason, failure: None }
    }

    fn log(&self, phone: &DcgClient, identity: &str) {
        match &self.failure {
            None => info!(%phone, identity_type = identity, reason = self.reason, "the identity's session not started"),
            Some(e) => {
                error!(%phone, identity_type = identity, reason = self.reason, error = %e, "the identity's session not started");
            }
        }
    }
}

/// What changed the CDEH session, sent as the `ypTriggerId` `CDEH.<trigger>` of its trace state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CdehTrigger {
    /// The session loop for the phone began: the engine started with it, the user selected it, or
    /// the CDEH identity's device list gained it.
    ProcessSessionChanges,
    DeviceEnabled,
    SessionUnlocked,
}

impl CdehTrigger {
    pub(super) fn trigger_id(self) -> &'static str {
        match self {
            Self::ProcessSessionChanges => "CDEH.ProcessSessionChanges",
            Self::DeviceEnabled => "CDEH.DeviceEnabled",
            Self::SessionUnlocked => "CDEH.SessionUnlocked",
        }
    }
}

/// One identity's session changes, run one at a time in arrival order by one task: a change another
/// one already follows in the queue is skipped, and a start still waiting for its connection stops
/// waiting when a later change arrives (its attempts go on until that change stops them) — as CDEH's
/// per-phone session-change loop reads its channel.
pub(super) struct SessionChanges {
    /// The identity's `clientType`, for the log.
    identity: &'static str,
    /// The queue of the task that runs the changes, made by the first change (inside the runtime).
    queue: Mutex<Option<mpsc::UnboundedSender<QueuedChange>>>,
}

struct QueuedChange {
    sessions: Arc<PartnerSessions>,
    phone: DcgClient,
    change: SessionChange,
}

impl SessionChanges {
    pub(super) fn new(identity: &'static str) -> Arc<Self> {
        Arc::new(Self { identity, queue: Mutex::new(None) })
    }

    pub(super) fn submit(&self, sessions: Arc<PartnerSessions>, phone: DcgClient, change: SessionChange) {
        let mut queue = self.queue.lock_or_panic();
        let mut queued = QueuedChange { sessions, phone, change };

        if let Some(sender) = queue.as_ref() {
            match sender.send(queued) {
                Ok(()) => return,
                // The task ended: a change it ran panicked. The next one starts a new task.
                Err(mpsc::error::SendError(unsent)) => {
                    error!(identity_type = self.identity, "the session-change task ended; a new one runs the changes");
                    queued = unsent;
                }
            }
        }

        let (sender, receiver) = mpsc::unbounded_channel();
        tokio::spawn(run_changes(self.identity, receiver));
        sender.send(queued).unwrap_or_else(|_| unreachable!("the task just spawned holds the receiver"));
        *queue = Some(sender);
    }
}

/// Runs the queued changes until the [`SessionChanges`] that sends them is dropped.
async fn run_changes(identity: &'static str, mut queue: mpsc::UnboundedReceiver<QueuedChange>) {
    let mut next = queue.recv().await;
    while let Some(queued) = next.take() {
        next = match queue.try_recv() {
            Ok(later) => {
                log_superseded(identity, &queued);
                Some(later)
            }
            Err(_) => match run_change(identity, queued, &mut queue).await {
                Ran::Done => queue.recv().await,
                Ran::Interrupted(later) => later,
            },
        };
    }
}

/// How a change ran: to its end, or until a later change arrived (`None`: the queue closed).
enum Ran {
    Done,
    Interrupted(Option<QueuedChange>),
}

async fn run_change(
    identity: &'static str,
    QueuedChange { sessions, phone, change }: QueuedChange,
    queue: &mut mpsc::UnboundedReceiver<QueuedChange>,
) -> Ran {
    match change {
        SessionChange::Skip(refusal) => refusal.log(&phone, identity),
        SessionChange::Stop => {
            info!(%phone, identity_type = identity, "the identity's session stopping");
            sessions.disconnect(&phone).await;
        }
        SessionChange::Start { request, trigger } => {
            info!(%phone, identity_type = identity, trigger = trigger.trigger_id(), "the identity's session starting");
            return run_start(identity, &sessions, &phone, request, queue).await;
        }
    }
    Ran::Done
}

/// A start waits for its connection until a later change arrives.
async fn run_start(
    identity: &'static str,
    sessions: &PartnerSessions,
    phone: &DcgClient,
    request: ConnectRequest,
    queue: &mut mpsc::UnboundedReceiver<QueuedChange>,
) -> Ran {
    tokio::select! {
        _ = sessions.connect(phone, request) => Ran::Done,
        later = queue.recv() => {
            debug!(%phone, identity_type = identity, "a later session change came; the start stops waiting");
            Ran::Interrupted(later)
        }
    }
}

fn log_superseded(identity: &'static str, queued: &QueuedChange) {
    debug!(
        phone = %queued.phone,
        identity_type = identity,
        change_kind = queued.change.kind(),
        trigger = queued.change.trigger_id(),
        "session change superseded by a later one; skipped"
    );
}
