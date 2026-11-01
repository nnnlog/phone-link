//! Mirroring's own Nano connection.
//!
//! It covers opening it (the `/nanohandshake` race completed by the phone's `launcher` channel),
//! the workflows started and stopped on that channel, the channels the phone creates for each
//! workflow, the idle release and the loss rules.

use phonelink_core::config::features::LinkTimers;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

pub use super::errors::ConnectFailure;
use super::errors::{attempt_status, close_code, code};
use super::gates::PhoneValues;
use super::wire::{self, property, scope, workflow};
use phonelink_app::AppLayer;
use phonelink_core::config::NanoConfig;
use phonelink_nano::channel::message::{InboundMessage, MessageChannel};
use phonelink_nano::channel::{ChannelError, ChannelType, RawChannel};
use phonelink_nano::cloud_relay::RelaySource;
use phonelink_nano::connect::{AttemptError, PhoneRemoting, open_mirroring};

use super::handshake::NanoHandshake;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use phonelink_nano::connection::{CloseReason, ConnectionType, Health, NanoConnection};

/// What the connection reports besides the channels it hands over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEvent {
    /// Missed packets.
    Unstable,
    Recovered,
    Closed(CloseReason),
}

/// Closing an opened connection.
pub trait LinkControl: Send + Sync {
    fn close(&self);
}

/// A connection whose `launcher` channel is open.
pub struct OpenedLink {
    pub launcher: MessageChannel,
    /// `/stopWorkflow` from the phone on `launcher`.
    pub stop_workflow: mpsc::UnboundedReceiver<InboundMessage>,
    /// The other channels the phone creates, in order.
    pub channels: mpsc::UnboundedReceiver<RawChannel>,
    pub events: mpsc::UnboundedReceiver<LinkEvent>,
    pub control: Arc<dyn LinkControl>,
    /// The winning attempt was the direct IP one (the phone is on the Mac's network).
    pub local: bool,
}

/// Opens mirroring's connection for the workflow named `workflow`; `Err` is the status code.
#[async_trait]
pub trait MirrorConnector: Send + Sync {
    async fn connect(&self, workflow: &str, audio: bool) -> Result<OpenedLink, ConnectFailure>;
}

/// The screen-mirroring path over the App layer's `/nanohandshake`.
pub struct NanoMirrorConnector {
    pub config: NanoConfig,
    pub app: Arc<AppLayer>,
    pub relay: Arc<dyn RelaySource>,
}

struct ConnectionControl(NanoConnection);

impl LinkControl for ConnectionControl {
    fn close(&self) {
        self.0.close();
    }
}

fn workflow_of(channel: &RawChannel) -> Option<&str> {
    channel.announcement.property(property::WORKFLOW)
}

/// Waits for the phone's `launcher` Message channel and opens it; the channels before it are kept.
async fn complete(connection: NanoConnection) -> Result<OpenedLink, AttemptError> {
    let mut channels = connection.channels();
    let mut early = Vec::new();
    let raw = loop {
        let Some(channel) = channels.recv().await else {
            return Err(AttemptError::Completion("the connection closed before its launcher channel".into()));
        };
        match workflow_of(&channel) {
            Some(workflow::LAUNCHER) if channel.announcement.channel_type == Some(ChannelType::Message) => {
                break channel;
            }
            Some(_) => early.push(channel),
            None => {
                warn!(raw_announcement = ?channel.announcement, "channel without a workflow; closed");
                channel.close();
            }
        }
    };

    let (launcher, mut listening) = MessageChannel::open_listening(raw, &[scope::STOP_WORKFLOW.to_string()])
        .await
        .map_err(|e| AttemptError::Completion(e.to_string()))?;
    let stop_workflow = listening.remove(scope::STOP_WORKFLOW).expect("registered");

    let local = connection.kind == ConnectionType::IpSrtp;

    let (tx, rx) = mpsc::unbounded_channel();
    for c in early {
        let _ = tx.send(c);
    }
    tokio::spawn(async move {
        while let Some(c) = channels.recv().await {
            if tx.send(c).is_err() {
                break;
            }
        }
    });

    let (events_tx, events) = mpsc::unbounded_channel();
    let mut health = connection.health();
    let mut closed = connection.closed();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                h = health.recv() => match h {
                    Ok(Health::MissedPackets) => { let _ = events_tx.send(LinkEvent::Unstable); }
                    Ok(Health::RecoveredPackets) => { let _ = events_tx.send(LinkEvent::Recovered); }
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        warn!(missed, "connection health events missed; the unstable state may be stale");
                    }
                    Err(broadcast::error::RecvError::Closed) => {}
                },
                r = closed.wait_for(Option::is_some) => {
                    let reason = r.map(|v| v.clone().expect("waited for Some")).unwrap_or(CloseReason::None);
                    let _ = events_tx.send(LinkEvent::Closed(reason));
                    return;
                }
            }
        }
    });

    Ok(OpenedLink {
        launcher,
        stop_workflow,
        channels: rx,
        events,
        control: Arc::new(ConnectionControl(connection)),
        local,
    })
}

#[async_trait]
impl MirrorConnector for NanoMirrorConnector {
    async fn connect(&self, workflow: &str, audio: bool) -> Result<OpenedLink, ConnectFailure> {
        let Some(record) = self.app.phone_record() else {
            return Err(ConnectFailure { code: code::DEVICE_UNREACHABLE, cause: "no phone record".into() });
        };

        let values = PhoneValues::from_record(&record, &self.app.config().app);
        let phone = PhoneRemoting { flags: values.remoting as i32, contract_version: values.contract_version };
        let handshake = Arc::new(NanoHandshake {
            app: self.app.clone(),
            correlation_vector: new_guid_lower(),
            workflow: workflow.to_string(),
            audio,
            diagnostics_servers: NanoHandshake::diagnostics_from(&self.config),
        });

        open_mirroring(&self.config, phone, &*self.relay, handshake, None, complete)
            .await
            .map_err(|e| ConnectFailure { code: attempt_status(&e), cause: e.to_string() })
    }
}

/// What the connection tells the feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkNotice {
    /// A workflow ended without the PC stopping it: the phone stopped it (85) or the connection
    /// closed (its code).
    WorkflowEnded {
        name: String,
        code: i32,
    },
    Unstable,
    Recovered,
    /// The connection closed with this code.
    Closed {
        code: i32,
    },
}

/// The channels the phone created for a workflow, after its Message channel.
pub struct WorkflowChannels {
    early: Vec<RawChannel>,
    rx: mpsc::UnboundedReceiver<RawChannel>,
}

impl WorkflowChannels {
    pub async fn next(&mut self) -> Option<RawChannel> {
        if !self.early.is_empty() {
            return Some(self.early.remove(0));
        }
        self.rx.recv().await
    }
}

/// A started workflow whose Message channel is open.
pub struct StartedWorkflow {
    pub name: String,
    pub id: String,
    pub channel: Arc<MessageChannel>,
    pub inbound: HashMap<String, mpsc::UnboundedReceiver<InboundMessage>>,
    pub others: WorkflowChannels,
    /// The connection is the direct IP one.
    pub local: bool,
}

/// A workflow whose `/workflow` is sent, waiting for its Message channel.
pub struct PendingWorkflow<'a> {
    links: &'a Links,
    name: String,
    id: String,
    rx: mpsc::UnboundedReceiver<RawChannel>,
    local: bool,
}

impl PendingWorkflow<'_> {
    /// Waits for the workflow's Message channel (for `remote`, the one with `type = orchestrator`)
    /// and opens it with handlers for `scopes`; the phone's other channels before it are kept.
    ///
    /// The connection or the workflow ending first, or the channel failing to open, fails the start
    /// with 268, so the window shows its error at once. (Phone Link on Windows shows it only about
    /// 100 s later.)
    pub async fn open(self, scopes: &[&str]) -> Result<StartedWorkflow, WorkflowStartError> {
        let PendingWorkflow { links, name, id, mut rx, local } = self;
        let mut early = Vec::new();
        let raw = loop {
            let Some(channel) = rx.recv().await else {
                return Err(WorkflowStartError::Status(code::CONNECTION_CANCELED));
            };
            let is_message = channel.announcement.channel_type == Some(ChannelType::Message);
            let typed = name != workflow::REMOTE
                || channel.announcement.property(property::TYPE) == Some(property::ORCHESTRATOR);
            if is_message && typed {
                break channel;
            }
            early.push(channel);
        };

        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        let (channel, inbound) = MessageChannel::open_listening(raw, &scopes).await.map_err(|e| {
            links.stop(&name);
            WorkflowStartError::ChannelNotOpened(e)
        })?;

        info!(workflow = %name, "workflow started");
        Ok(StartedWorkflow {
            name,
            id,
            channel: Arc::new(channel),
            inbound,
            others: WorkflowChannels { early, rx },
            local,
        })
    }
}

/// Why a start failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkflowStartError {
    #[error("status {0}")]
    Status(i32),
    #[error("{0}")]
    NotConnected(ConnectFailure),
    #[error("workflow {0} is already live")]
    AlreadyLive(String),
    #[error("/workflow not sent: {0}")]
    NotSent(#[source] ChannelError),
    #[error("the workflow's message channel did not open: {0}")]
    ChannelNotOpened(#[source] ChannelError),
}

impl WorkflowStartError {
    pub fn code(&self) -> i32 {
        match self {
            Self::Status(c) => *c,
            Self::NotConnected(f) => f.code,
            // A second start of a live workflow is refused as a failure of the connection service.
            Self::AlreadyLive(_) => code::CONNECTION_FAILED,
            Self::NotSent(_) => code::DISCONNECTED_UNKNOWN,
            Self::ChannelNotOpened(_) => code::CONNECTION_CANCELED,
        }
    }
}

struct Handle {
    id: String,
    tx: mpsc::UnboundedSender<RawChannel>,
}

struct Live {
    generation: u64,
    control: Arc<dyn LinkControl>,
    launcher: Arc<MessageChannel>,
    handles: HashMap<String, Handle>,
    idle: Option<CancellationToken>,
    /// The code the PC closed the connection with.
    requested: Option<i32>,
    local: bool,
}

/// The connection and its workflows.
pub struct Links {
    connector: Arc<dyn MirrorConnector>,
    timers: LinkTimers,
    connect_lock: tokio::sync::Mutex<()>,
    live: Mutex<Option<Live>>,
    generation: AtomicU64,
    notices: mpsc::UnboundedSender<LinkNotice>,
    mac_locked: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The runtime the idle timer runs on: a workflow stops from the app's synchronous calls too,
    /// which run outside any runtime.
    runtime: tokio::runtime::Handle,
    me: std::sync::Weak<Links>,
}

impl Links {
    pub fn new(
        connector: Arc<dyn MirrorConnector>,
        timers: LinkTimers,
        notices: mpsc::UnboundedSender<LinkNotice>,
        mac_locked: Arc<dyn Fn() -> bool + Send + Sync>,
        runtime: tokio::runtime::Handle,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            connector,
            timers,
            connect_lock: tokio::sync::Mutex::new(()),
            live: Mutex::new(None),
            generation: AtomicU64::new(0),
            notices,
            mac_locked,
            runtime,
            me: me.clone(),
        })
    }

    pub fn is_connected(&self) -> bool {
        self.live.lock_or_panic().is_some()
    }

    pub fn is_live(&self, name: &str) -> bool {
        self.live.lock_or_panic().as_ref().is_some_and(|l| l.handles.contains_key(name))
    }

    pub fn live_workflows(&self) -> usize {
        self.live.lock_or_panic().as_ref().map_or(0, |l| l.handles.len())
    }

    /// Starts workflow `name` and waits for its Message channel (for `remote`, the one with
    /// `type = orchestrator`), opened with handlers for `scopes`. `new_connection`: an idle
    /// connection is closed first and a new one opened. `audio` is the handshake's `settings.audio`
    /// when a connection is opened.
    pub async fn start(
        &self,
        name: &str,
        scopes: &[&str],
        new_connection: bool,
        audio: bool,
    ) -> Result<StartedWorkflow, WorkflowStartError> {
        self.begin(name, new_connection, audio).await?.open(scopes).await
    }

    /// The first half of [`Self::start`]: connects when needed and sends `/workflow`.
    pub async fn begin(
        &self,
        name: &str,
        new_connection: bool,
        audio: bool,
    ) -> Result<PendingWorkflow<'_>, WorkflowStartError> {
        let (id, rx, launcher, local) = {
            let _connecting = self.connect_lock.lock().await;
            // A start that asks for a new connection while no workflow is live closes the current
            // one (when there is one) and always pauses before connecting.
            if new_connection && self.live_workflows() == 0 {
                self.close_idle();
                tokio::time::sleep(self.timers.new_connection_pause).await;
            }

            if !self.is_connected() {
                let opened = tokio::time::timeout(self.timers.connect, self.connector.connect(name, audio))
                    .await
                    .unwrap_or_else(|_| {
                        Err(ConnectFailure {
                            code: code::CONNECTION_TIMED_OUT,
                            cause: "no connection within the connect timeout".into(),
                        })
                    })
                    .map_err(WorkflowStartError::NotConnected)?;
                self.install(opened);
            }

            let mut guard = self.live.lock_or_panic();
            let live = guard.as_mut().ok_or(WorkflowStartError::Status(code::DISCONNECTED_UNKNOWN))?;
            if live.handles.contains_key(name) {
                return Err(WorkflowStartError::AlreadyLive(name.to_string()));
            }
            if let Some(idle) = live.idle.take() {
                idle.cancel();
            }

            let id = new_guid_lower();
            let (tx, rx) = mpsc::unbounded_channel();
            live.handles.insert(name.to_string(), Handle { id: id.clone(), tx });
            (id, rx, live.launcher.clone(), live.local)
        };

        if let Err(e) = launcher.send(scope::WORKFLOW, &wire::workflow_body(name, &id)) {
            self.forget(name, &id);
            return Err(WorkflowStartError::NotSent(e));
        }

        info!(workflow = name, "workflow starting");
        Ok(PendingWorkflow { links: self, name: name.to_string(), id, rx, local })
    }

    fn forget(&self, name: &str, id: &str) {
        let mut guard = self.live.lock_or_panic();
        if let Some(live) = guard.as_mut() {
            if live.handles.get(name).is_some_and(|h| h.id == id) {
                live.handles.remove(name);
            }
        }

        drop(guard);
        self.arm_idle();
    }

    /// The PC stops workflow `name`: `/stopWorkflow`, and the idle release when it was the last.
    pub fn stop(&self, name: &str) {
        let (launcher, handle) = {
            let mut guard = self.live.lock_or_panic();
            let Some(live) = guard.as_mut() else { return };
            let Some(h) = live.handles.remove(name) else { return };
            (live.launcher.clone(), h)
        };

        let sent = launcher.send(scope::STOP_WORKFLOW, &wire::workflow_body(name, &handle.id));
        info!(
            workflow = name,
            sent = sent.is_ok(),
            error = sent.as_ref().err().map(tracing::field::display),
            "workflow stopped by the mac"
        );

        self.arm_idle();
    }

    fn arm_idle(&self) {
        let token = {
            let mut guard = self.live.lock_or_panic();
            let Some(live) = guard.as_mut() else { return };
            if !live.handles.is_empty() || live.idle.is_some() {
                return;
            }
            let t = CancellationToken::new();
            live.idle = Some(t.clone());
            (t, live.generation)
        };

        let (token, generation) = token;
        let me = self.me.clone();
        let after = self.timers.idle_release;
        self.runtime.spawn(async move {
            tokio::select! {
                _ = token.cancelled() => {}
                _ = tokio::time::sleep(after) => {
                    // The close's line carries the code (`EXPIRED`) that tells the idle release.
                    if let Some(me) = me.upgrade() {
                        if me.generation() == Some(generation) {
                            me.close(code::EXPIRED);
                        }
                    }
                }
            }
        });
    }

    fn generation(&self) -> Option<u64> {
        self.live.lock_or_panic().as_ref().map(|l| l.generation)
    }

    /// Closes the connection when no workflow is live on it, to open a new one: the windows waiting
    /// for that new one are not told of a lost connection. `true` when one was closed.
    fn close_idle(&self) -> bool {
        let live = {
            let mut guard = self.live.lock_or_panic();
            if !guard.as_ref().is_some_and(|l| l.handles.is_empty()) {
                return false;
            }
            guard.take()
        };

        let Some(live) = live else { return false };
        if let Some(idle) = &live.idle {
            idle.cancel();
        }

        live.control.close();
        info!("idle mirroring connection closed; a new one opens");
        true
    }

    /// Closes the connection with `code`.
    pub fn close(&self, code: i32) {
        let control = {
            let mut guard = self.live.lock_or_panic();
            let Some(live) = guard.as_mut() else { return };
            live.requested = Some(code);
            live.control.clone()
        };

        control.close();
        self.closed(self.generation(), &CloseReason::UserEnded);
    }

    /// The connection of `generation` closed with `reason`.
    fn closed(&self, generation: Option<u64>, reason: &CloseReason) {
        let live = {
            let mut guard = self.live.lock_or_panic();
            match guard.as_ref() {
                Some(l) if Some(l.generation) == generation => guard.take(),
                _ => None,
            }
        };

        let Some(live) = live else { return };
        let code = close_code(reason, live.requested, (self.mac_locked)());
        info!(code, reason = %reason, "mirroring connection closed");

        for name in live.handles.keys() {
            let _ = self.notices.send(LinkNotice::WorkflowEnded { name: name.clone(), code });
        }
        let _ = self.notices.send(LinkNotice::Closed { code });
    }

    fn install(&self, opened: OpenedLink) {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let OpenedLink { launcher, stop_workflow, channels, events, control, local } = opened;
        let launcher = Arc::new(launcher);
        let launcher_closed = launcher.closed();
        info!(is_local = local, "mirroring connection opened");

        *self.live.lock_or_panic() =
            Some(Live { generation, control, launcher, handles: HashMap::new(), idle: None, requested: None, local });

        let me = self.me.clone();
        let grace = self.timers.launcher_closed;
        tokio::spawn(route(me, generation, channels, stop_workflow, events, launcher_closed, grace));
    }

    fn route_channel(&self, generation: u64, channel: RawChannel) {
        let Some(name) = workflow_of(&channel).map(str::to_string) else {
            warn!(raw_announcement = ?channel.announcement, "channel without a workflow; closed");
            channel.close();
            return;
        };

        let guard = self.live.lock_or_panic();
        let handle = guard.as_ref().filter(|l| l.generation == generation).and_then(|l| l.handles.get(&name));
        match handle {
            Some(h) => {
                if let Err(e) = h.tx.send(channel) {
                    warn!(workflow = %name, "channel for a workflow that is ending; closed");
                    e.0.close();
                }
            }
            None => {
                warn!(raw_workflow = %name, raw_announcement = ?channel.announcement, "channel for no live workflow; closed");
                channel.close();
            }
        }
    }

    fn peer_stopped(&self, generation: u64, body: &[u8]) {
        let (name, id) = match wire::parse_stop_workflow(body) {
            Ok(v) => v,
            Err(e) => return warn!(error = %e, "malformed /stopWorkflow; dropped"),
        };

        let removed = {
            let mut guard = self.live.lock_or_panic();
            let Some(live) = guard.as_mut().filter(|l| l.generation == generation) else { return };
            // With a workflowId only that workflow ends; without one, the workflow of that name.
            let target = match id.as_deref() {
                Some(id) => live.handles.iter().find(|(_, h)| h.id == id).map(|(n, _)| n.clone()),
                None => live.handles.contains_key(&name).then(|| name.clone()),
            };
            target.and_then(|t| live.handles.remove(&t).map(|_| t))
        };

        match removed {
            Some(name) => {
                info!(workflow = %name, "workflow stopped by the phone");
                // The phone ending the last workflow does not start the idle release by itself.
                let _ = self.notices.send(LinkNotice::WorkflowEnded { name, code: code::PEER_ENDED });
            }
            None => debug!(raw_workflow = %name, "/stopWorkflow for no live workflow; ignored"),
        }
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

async fn route(
    me: std::sync::Weak<Links>,
    generation: u64,
    mut channels: mpsc::UnboundedReceiver<RawChannel>,
    mut stop_workflow: mpsc::UnboundedReceiver<InboundMessage>,
    mut events: mpsc::UnboundedReceiver<LinkEvent>,
    mut launcher_closed: tokio::sync::watch::Receiver<bool>,
    grace: Duration,
) {
    let mut channels_open = true;
    let mut stops_open = true;
    let mut watching_launcher = true;
    let mut launcher_deadline: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            c = channels.recv(), if channels_open => match c {
                Some(c) => match me.upgrade() {
                    Some(links) => links.route_channel(generation, c),
                    None => return,
                },
                None => channels_open = false,
            },
            m = stop_workflow.recv(), if stops_open => match (m, me.upgrade()) {
                (Some(m), Some(links)) => links.peer_stopped(generation, &m.body),
                (None, _) => stops_open = false,
                (Some(_), None) => return,
            },
            r = launcher_closed.wait_for(|c| *c), if watching_launcher => {
                watching_launcher = false;
                if r.is_ok() {
                    warn!("launcher channel closed; the connection closes after a grace period");
                    launcher_deadline = Some(tokio::time::Instant::now() + grace);
                }
            },
            _ = sleep_until(launcher_deadline) => {
                if let Some(links) = me.upgrade() {
                    if links.generation() == Some(generation) {
                        links.close(code::DISCONNECTED_UNKNOWN);
                    }
                }
                return;
            },
            e = events.recv() => {
                let Some(links) = me.upgrade() else { return };
                match e {
                    Some(LinkEvent::Unstable) => { let _ = links.notices.send(LinkNotice::Unstable); }
                    Some(LinkEvent::Recovered) => { let _ = links.notices.send(LinkNotice::Recovered); }
                    Some(LinkEvent::Closed(reason)) => {
                        links.closed(Some(generation), &reason);
                        return;
                    }
                    None => {
                        links.closed(Some(generation), &CloseReason::None);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    //! A connector whose connections are in-memory: the test is the phone.
    use std::sync::Mutex;

    use super::{ConnectFailure, LinkControl, LinkEvent, MirrorConnector, OpenedLink, code, scope};

    use async_trait::async_trait;

    use phonelink_nano::channel::RawChannel;

    use phonelink_nano::channel::message::InboundMessage;

    use phonelink_nano::channel::message::MessageChannel;

    use phonelink_nano::connection::CloseReason;

    use std::sync::Arc;

    use std::sync::atomic::AtomicU64;

    use std::sync::atomic::Ordering;

    use phonelink_core::sync::MutexExt;
    use phonelink_nano::channel::testing::{Loopback, pair};
    use tokio::sync::mpsc;

    /// The phone's side of one test connection.
    pub struct PhoneSide {
        /// The phone's end of `launcher`.
        pub launcher: MessageChannel,
        /// `/workflow` and `/stopWorkflow` the PC sent.
        pub workflows: mpsc::UnboundedReceiver<InboundMessage>,
        pub stops: mpsc::UnboundedReceiver<InboundMessage>,
        channels: mpsc::UnboundedSender<RawChannel>,
        events: mpsc::UnboundedSender<LinkEvent>,
        pub closed_by_pc: Arc<std::sync::atomic::AtomicBool>,
        pub workflow: String,
        pub audio: bool,
    }

    impl PhoneSide {
        /// Creates a channel announced as `class,id,type,props` and returns the phone's end.
        pub fn create(&self, announcement: &str) -> (RawChannel, Arc<Loopback>) {
            let ((pc, _), (phone, link)) = pair(announcement, 1400);
            self.channels.send(pc).expect("the PC side listens");
            (phone, link)
        }

        pub fn event(&self, e: LinkEvent) {
            let _ = self.events.send(e);
        }
    }

    struct Control(Arc<std::sync::atomic::AtomicBool>, mpsc::UnboundedSender<LinkEvent>);

    impl LinkControl for Control {
        fn close(&self) {
            self.0.store(true, Ordering::SeqCst);
            let _ = self.1.send(LinkEvent::Closed(CloseReason::UserEnded));
        }
    }

    /// Hands every connection's phone side to the test; fails with `fail` when set.
    #[derive(Default)]
    pub struct TestConnector {
        pub phones: Mutex<Vec<PhoneSide>>,
        pub fail: Mutex<Option<i32>>,
        pub attempts: AtomicU64,
        pub notify: tokio::sync::Notify,
    }

    impl TestConnector {
        pub fn take(&self) -> Option<PhoneSide> {
            let mut p = self.phones.lock_or_panic();
            (!p.is_empty()).then(|| p.remove(0))
        }

        /// Waits for the next connection.
        pub async fn next(&self) -> PhoneSide {
            loop {
                let notified = self.notify.notified();
                if let Some(p) = self.take() {
                    return p;
                }
                notified.await;
            }
        }
    }

    #[async_trait]
    impl MirrorConnector for TestConnector {
        async fn connect(&self, workflow: &str, audio: bool) -> Result<OpenedLink, ConnectFailure> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if let Some(code) = *self.fail.lock_or_panic() {
                return Err(ConnectFailure { code, cause: "scripted".into() });
            }

            let ((pc, _), (phone, _)) =
                pair("Microsoft::Basix::Dct::Channel::Class::Messaging,1,4,workflow=launcher", 1400);
            let scopes = [scope::STOP_WORKFLOW.to_string()];
            let phone_scopes = [scope::WORKFLOW.to_string(), scope::STOP_WORKFLOW.to_string()];
            let (pc, phone) = tokio::join!(
                MessageChannel::open_listening(pc, &scopes),
                MessageChannel::open_listening(phone, &phone_scopes)
            );
            let (launcher, mut l) =
                pc.map_err(|e| ConnectFailure { code: code::MESSAGE_CHANNEL_FAILED, cause: e.to_string() })?;
            let (phone, mut pl) =
                phone.map_err(|e| ConnectFailure { code: code::MESSAGE_CHANNEL_FAILED, cause: e.to_string() })?;

            let (channels_tx, channels) = mpsc::unbounded_channel();
            let (events_tx, events) = mpsc::unbounded_channel();
            let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
            self.phones.lock_or_panic().push(PhoneSide {
                launcher: phone,
                workflows: pl.remove(scope::WORKFLOW).expect("registered"),
                stops: pl.remove(scope::STOP_WORKFLOW).expect("registered"),
                channels: channels_tx,
                events: events_tx.clone(),
                closed_by_pc: closed.clone(),
                workflow: workflow.to_string(),
                audio,
            });

            self.notify.notify_waiters();
            Ok(OpenedLink {
                launcher,
                stop_workflow: l.remove(scope::STOP_WORKFLOW).expect("registered"),
                channels,
                events,
                control: Arc::new(Control(closed, events_tx)),
                local: true,
            })
        }
    }
}
