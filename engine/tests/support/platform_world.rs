//! In-memory doubles for the platform-layer tests: a loopback `MessageTransport` pair that connects a
//! PC-side `Router` to a phone-side `Router` (the scripted fake phone), a fake `HubControl`, a fake
//! `Waker` and a fake `TrustGate`. No sockets; tests run on paused tokio time.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use prost::Message;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::LockState;
use phonelink_engine::edge::standins::FixedLock;
use phonelink_engine::edge::testing::ManualClock;
use phonelink_engine::platform::{
    InboundMessage, PartnerSessions, PlatformDeps, PlatformRouteHandler, PlatformRoutes, Router, route,
};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::{
    AppInbound, HubControl, MessageSink, MessageTransport, PlatformMessenger, RelayEvent, TrustGate,
    TrustRotationError, Waker,
};
use phonelink_engine::tap::{MemoryTap, Tap, null_tap};
use phonelink_engine::types::{
    ConnectReason, DcgClient, DcgSendStatus, Environment, HeaderMap, HubSendResult, HubState, MessageType, OpenStatus,
    Opened, OutboundMessage, PartnerDisconnectReason, TraceContext, TransportMessage, WakeOptions, WakeStatus, Woken,
};

pub fn pc_id() -> DcgClient {
    DcgClient::new("pc-11111111", Environment::Prod)
}

pub fn phone_id() -> DcgClient {
    DcgClient::new("phone-22222222", Environment::Prod)
}

/// Lets every spawned task run until the runtime is idle (paused time advances 1 ms per round).
pub async fn settle() {
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

pub fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    HeaderMap::from_pairs(pairs.iter().copied()).unwrap()
}

pub fn pairs(h: &HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

pub fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

pub fn platform(pairs: &[(&str, &str)], payload: &[u8]) -> TransportMessage {
    TransportMessage::new(MessageType::Platform, headers(pairs), payload.to_vec())
}

pub fn app(pairs: &[(&str, &str)], payload: &[u8]) -> TransportMessage {
    TransportMessage::new(MessageType::App, headers(pairs), payload.to_vec())
}

// ------------------------------------------------------------------------------------------------
// Loopback transport
// ------------------------------------------------------------------------------------------------

/// What the next send does.
#[derive(Debug, Clone)]
pub enum Behavior {
    /// Hand the message to the peer router and report success.
    Deliver,
    /// Report this failure without delivering.
    Fail(DcgSendStatus),
    /// Report success without delivering.
    Swallow,
    /// Never complete.
    Hang,
}

pub struct Loopback {
    pub own: DcgClient,
    peer: Mutex<Option<Weak<Router>>>,
    sent: Mutex<Vec<OutboundMessage>>,
    script: Mutex<VecDeque<Behavior>>,
    default: Mutex<Behavior>,
}

impl Loopback {
    pub fn new(own: DcgClient) -> Arc<Self> {
        Arc::new(Self {
            own,
            peer: Mutex::new(None),
            sent: Mutex::new(Vec::new()),
            script: Mutex::new(VecDeque::new()),
            default: Mutex::new(Behavior::Deliver),
        })
    }

    pub fn connect(&self, peer: &Arc<Router>) {
        *self.peer.lock().unwrap() = Some(Arc::downgrade(peer));
    }

    pub fn sent(&self) -> Vec<OutboundMessage> {
        self.sent.lock().unwrap().clone()
    }

    pub fn take_sent(&self) -> Vec<OutboundMessage> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }

    /// Messages sent whose `_route` header is `route`.
    pub fn sent_on(&self, route: &str) -> Vec<OutboundMessage> {
        self.sent().into_iter().filter(|m| m.message.route() == Some(route)).collect()
    }

    /// The behavior of the next send (queued in order).
    pub fn script(&self, behavior: Behavior) {
        self.script.lock().unwrap().push_back(behavior);
    }

    pub fn set_default(&self, behavior: Behavior) {
        *self.default.lock().unwrap() = behavior;
    }
}

#[async_trait]
impl MessageTransport for Loopback {
    async fn send(&self, message: OutboundMessage) -> DcgSendStatus {
        let scripted = self.script.lock().unwrap().pop_front();
        let behavior = scripted.unwrap_or_else(|| self.default.lock().unwrap().clone());
        self.sent.lock().unwrap().push(message.clone());

        match behavior {
            Behavior::Deliver => {
                let peer = self.peer.lock().unwrap().as_ref().and_then(Weak::upgrade);
                if let Some(peer) = peer {
                    peer.on_message(
                        self.own.clone(),
                        message.connection_session_id.clone(),
                        message.trace.clone(),
                        message.message,
                    );
                }
                DcgSendStatus::Success
            }
            Behavior::Fail(status) => status,
            Behavior::Swallow => DcgSendStatus::Success,
            Behavior::Hang => std::future::pending().await,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The scripted phone's handlers and App recorders
// ------------------------------------------------------------------------------------------------

pub type Reply = Box<dyn Fn(&InboundMessage) -> Option<TransportMessage> + Send + Sync>;

/// A phone-side route handler: records every request and answers with whatever `reply` returns.
pub struct Scripted {
    router: Weak<Router>,
    reply: Mutex<Reply>,
    received: Mutex<Vec<InboundMessage>>,
    tx: mpsc::UnboundedSender<InboundMessage>,
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<InboundMessage>>,
}

impl Scripted {
    pub fn install(router: &Arc<Router>, route: &str, reply: Reply) -> Arc<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let handler = Arc::new(Self {
            router: Arc::downgrade(router),
            reply: Mutex::new(reply),
            received: Mutex::new(Vec::new()),
            tx,
            rx: tokio::sync::Mutex::new(rx),
        });
        router.register_route(route, handler.clone()).unwrap();
        handler
    }

    /// Records only, never answers.
    pub fn silent(router: &Arc<Router>, route: &str) -> Arc<Self> {
        Self::install(router, route, Box::new(|_| None))
    }

    pub fn set_reply(&self, reply: Reply) {
        *self.reply.lock().unwrap() = reply;
    }

    pub fn received(&self) -> Vec<InboundMessage> {
        self.received.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.received.lock().unwrap().len()
    }

    /// The next request, waiting up to 10 virtual seconds.
    pub async fn next(&self) -> InboundMessage {
        let mut rx = self.rx.lock().await;
        tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("no request arrived").unwrap()
    }
}

#[async_trait]
impl PlatformRouteHandler for Scripted {
    async fn handle(&self, inbound: InboundMessage) {
        let reply = (self.reply.lock().unwrap())(&inbound);
        self.received.lock().unwrap().push(inbound.clone());
        let _ = self.tx.send(inbound.clone());
        if let (Some(reply), Some(router)) = (reply, self.router.upgrade()) {
            router.respond(&inbound.source, &inbound.message, reply, 0, inbound.trace.clone()).await;
        }
    }
}

#[derive(Default)]
pub struct AppRecorder {
    messages: Mutex<Vec<(DcgClient, Option<String>, TransportMessage)>>,
}

impl AppRecorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn messages(&self) -> Vec<(DcgClient, Option<String>, TransportMessage)> {
        self.messages.lock().unwrap().clone()
    }
}

impl AppInbound for AppRecorder {
    fn on_app_message(
        &self,
        source: DcgClient,
        connection_session_id: Option<String>,
        _trace: TraceContext,
        message: TransportMessage,
    ) {
        self.messages.lock().unwrap().push((source, connection_session_id, message));
    }
}

/// A KVP `text/json` reply body.
pub fn kvp_reply(body: &str) -> TransportMessage {
    platform(&[("ms-content-type", "text/json")], body.as_bytes())
}

// ------------------------------------------------------------------------------------------------
// The PC ↔ phone router pair
// ------------------------------------------------------------------------------------------------

pub struct Pair {
    pub config: Arc<EngineConfig>,
    pub pc: Arc<Router>,
    pub phone: Arc<Router>,
    pub pc_link: Arc<Loopback>,
    pub phone_link: Arc<Loopback>,
    pub pc_tap: Arc<MemoryTap>,
    pub pc_app: Arc<AppRecorder>,
    pub phone_app: Arc<AppRecorder>,
}

impl Pair {
    pub fn new(config: EngineConfig) -> Self {
        let config = Arc::new(config);
        let pc_link = Loopback::new(pc_id());
        let phone_link = Loopback::new(phone_id());

        let pc_tap = MemoryTap::new();
        let tap: Tap = pc_tap.clone();
        let pc = Router::new(config.clone(), pc_link.clone(), tap);
        let phone = Router::new(config.clone(), phone_link.clone(), null_tap());

        pc_link.connect(&phone);
        phone_link.connect(&pc);

        let pc_app = AppRecorder::new();
        let phone_app = AppRecorder::new();
        pc.set_app_inbound(pc_app.clone());
        phone.set_app_inbound(phone_app.clone());

        Pair { config, pc, phone, pc_link, phone_link, pc_tap, pc_app, phone_app }
    }

    /// Hands `message` to the PC router as if the phone had sent it.
    pub fn inject(&self, message: TransportMessage) {
        self.pc.on_message(phone_id(), None, TraceContext::new_root(Vec::new()), message);
    }

    pub fn inject_session_based(&self, message: TransportMessage, connection_session_id: &str) {
        self.pc.on_message(
            phone_id(),
            Some(connection_session_id.to_owned()),
            TraceContext::new_root(Vec::new()),
            message,
        );
    }
}

// ------------------------------------------------------------------------------------------------
// Fake hub
// ------------------------------------------------------------------------------------------------

pub struct FakeHubControl {
    states: Mutex<HashMap<Environment, HubState>>,
    partners: Mutex<HashSet<DcgClient>>,
    opens: Mutex<Vec<(DcgClient, ConnectReason)>>,
    /// The trace context each `open` was given, in call order.
    open_traces: Mutex<Vec<TraceContext>>,
    /// How long each `open` takes (zero by default).
    open_delay: Mutex<Duration>,
    /// Opens whose future was dropped before they finished.
    opens_dropped: AtomicUsize,
    open_script: Mutex<VecDeque<Opened>>,
    announces: Mutex<Vec<DcgClient>>,
    closes: Mutex<Vec<Environment>>,
    events: broadcast::Sender<RelayEvent>,
}

impl FakeHubControl {
    /// The Prod connection starts in `prod_state`; the others are disconnected.
    pub fn new(prod_state: HubState) -> Arc<Self> {
        let mut states = HashMap::new();
        for env in Environment::ALL {
            states.insert(env, HubState::Disconnected);
        }
        states.insert(Environment::Prod, prod_state);

        Arc::new(Self {
            states: Mutex::new(states),
            partners: Mutex::new(HashSet::new()),
            opens: Mutex::new(Vec::new()),
            open_traces: Mutex::new(Vec::new()),
            open_delay: Mutex::new(Duration::ZERO),
            opens_dropped: AtomicUsize::new(0),
            open_script: Mutex::new(VecDeque::new()),
            announces: Mutex::new(Vec::new()),
            closes: Mutex::new(Vec::new()),
            events: broadcast::channel(256).0,
        })
    }

    pub fn set_state(&self, env: Environment, state: HubState) {
        self.states.lock().unwrap().insert(env, state);
        let _ = self.events.send(RelayEvent::HubState { env, state });
    }

    /// The partner enters the partner set (`OnPartnerConnected`).
    pub fn partner_joins(&self, partner: &DcgClient) {
        self.partners.lock().unwrap().insert(partner.clone());
        let _ = self.events.send(RelayEvent::PartnerConnected(partner.clone()));
    }

    /// The partner leaves the partner set with `reason`.
    pub fn partner_leaves(&self, partner: &DcgClient, reason: PartnerDisconnectReason) {
        self.partners.lock().unwrap().remove(partner);
        let _ = self.events.send(RelayEvent::PartnerDisconnected { partner: partner.clone(), reason });
    }

    /// The connection of `env` goes to `state`, removing every partner with `LocalHubDisconnect`.
    pub fn lose_connection(&self, env: Environment, state: HubState) {
        self.set_state(env, state);
        let gone: Vec<DcgClient> = self.partners.lock().unwrap().iter().filter(|p| p.env == env).cloned().collect();
        for partner in gone {
            self.partner_leaves(&partner, PartnerDisconnectReason::LocalHubDisconnect);
        }
    }

    pub fn script_open(&self, status: OpenStatus) {
        self.open_script.lock().unwrap().push_back(Opened::from(status));
    }

    /// The next open fails with `status`, saying `cause`.
    pub fn script_open_failure(&self, status: OpenStatus, cause: &str) {
        self.open_script.lock().unwrap().push_back(Opened { status, cause: Some(cause.to_string()) });
    }

    pub fn opens(&self) -> Vec<(DcgClient, ConnectReason)> {
        self.opens.lock().unwrap().clone()
    }

    /// The trace contexts the opens carried in their connection headers.
    pub fn open_traces(&self) -> Vec<TraceContext> {
        self.open_traces.lock().unwrap().clone()
    }

    /// Every later `open` takes `delay` before it answers.
    pub fn set_open_delay(&self, delay: Duration) {
        *self.open_delay.lock().unwrap() = delay;
    }

    /// Opens dropped by their caller before they finished (the real relay's open must run to its end).
    pub fn opens_dropped(&self) -> usize {
        self.opens_dropped.load(Ordering::SeqCst)
    }

    pub fn announces(&self) -> Vec<DcgClient> {
        self.announces.lock().unwrap().clone()
    }

    pub fn closes(&self) -> Vec<Environment> {
        self.closes.lock().unwrap().clone()
    }
}

/// Counts a future dropped before it finished; `forget` it on completion.
struct Unfinished<'a>(&'a AtomicUsize);

impl Drop for Unfinished<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl HubControl for FakeHubControl {
    async fn open(&self, partner: &DcgClient, reason: ConnectReason, trace: &TraceContext) -> Opened {
        self.opens.lock().unwrap().push((partner.clone(), reason));
        self.open_traces.lock().unwrap().push(trace.clone());

        let delay = *self.open_delay.lock().unwrap();
        if !delay.is_zero() {
            let unfinished = Unfinished(&self.opens_dropped);
            tokio::time::sleep(delay).await;
            std::mem::forget(unfinished);
        }

        let opened = self.open_script.lock().unwrap().pop_front().unwrap_or(Opened::from(OpenStatus::Success));
        if opened.status == OpenStatus::Success {
            self.set_state(partner.env, HubState::Connected);
        }
        opened
    }

    fn hub_state(&self, env: Environment) -> HubState {
        self.states.lock().unwrap()[&env]
    }

    fn is_partner_connected(&self, partner: &DcgClient) -> bool {
        self.partners.lock().unwrap().contains(partner)
    }

    async fn send_connected(&self, partner: &DcgClient) -> HubSendResult {
        self.announces.lock().unwrap().push(partner.clone());
        HubSendResult::Success
    }

    fn subscribe(&self) -> broadcast::Receiver<RelayEvent> {
        self.events.subscribe()
    }

    async fn close(&self, env: Environment) {
        self.closes.lock().unwrap().push(env);
        self.lose_connection(env, HubState::Disconnected);
    }
}

// ------------------------------------------------------------------------------------------------
// Fake waker
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeEnd {
    /// The partner joins the hub; `Success`.
    Succeed,
    Fail(WakeStatus),
    /// Waits for the cancel token; `Cancelled`.
    UntilCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeScript {
    pub delay: Duration,
    pub end: WakeEnd,
}

impl WakeScript {
    pub fn now(end: WakeEnd) -> Self {
        Self { delay: Duration::ZERO, end }
    }
}

/// One wake as the fake waker received it.
#[derive(Debug, Clone)]
pub struct WakeCall {
    pub target: DcgClient,
    pub options: WakeOptions,
    /// The TTL the caller passed.
    pub ttl: Duration,
    pub at: tokio::time::Instant,
}

/// A `Waker` that answers from a script. Like the real waker it keeps no TTL state: it records the
/// TTL each caller passes.
pub struct FakeWaker {
    hub: Arc<FakeHubControl>,
    calls: Mutex<Vec<WakeCall>>,
    script: Mutex<VecDeque<WakeScript>>,
    default: Mutex<WakeScript>,
    pub breaker_resets: AtomicUsize,
    pub cancelled: AtomicUsize,
}

impl FakeWaker {
    pub fn new(hub: Arc<FakeHubControl>) -> Arc<Self> {
        Arc::new(Self {
            hub,
            calls: Mutex::new(Vec::new()),
            script: Mutex::new(VecDeque::new()),
            default: Mutex::new(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable))),
            breaker_resets: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
        })
    }

    pub fn script(&self, script: WakeScript) {
        self.script.lock().unwrap().push_back(script);
    }

    pub fn set_default(&self, script: WakeScript) {
        *self.default.lock().unwrap() = script;
    }

    pub fn calls(&self) -> Vec<WakeCall> {
        self.calls.lock().unwrap().clone()
    }

    pub fn options(&self) -> Vec<WakeOptions> {
        self.calls().into_iter().map(|c| c.options).collect()
    }

    /// The TTL of every wake, in seconds.
    pub fn ttls(&self) -> Vec<u64> {
        self.calls().into_iter().map(|c| c.ttl.as_secs()).collect()
    }
}

#[async_trait]
impl Waker for FakeWaker {
    async fn wake(&self, target: &DcgClient, options: WakeOptions, ttl: Duration, cancel: CancellationToken) -> Woken {
        Woken::from(
            async {
                self.calls.lock().unwrap().push(WakeCall {
                    target: target.clone(),
                    options,
                    ttl,
                    at: tokio::time::Instant::now(),
                });

                let scripted = self.script.lock().unwrap().pop_front();
                let script = scripted.unwrap_or_else(|| *self.default.lock().unwrap());
                if !script.delay.is_zero() {
                    tokio::select! {
                        _ = tokio::time::sleep(script.delay) => {}
                        _ = cancel.cancelled() => {
                            self.cancelled.fetch_add(1, Ordering::SeqCst);
                            return WakeStatus::Cancelled;
                        }
                    }
                }

                match script.end {
                    WakeEnd::Succeed => {
                        self.hub.partner_joins(target);
                        WakeStatus::Success
                    }
                    WakeEnd::Fail(status) => status,
                    WakeEnd::UntilCancelled => {
                        cancel.cancelled().await;
                        self.cancelled.fetch_add(1, Ordering::SeqCst);
                        WakeStatus::Cancelled
                    }
                }
            }
            .await,
        )
    }

    fn reset_breakers(&self) {
        self.breaker_resets.fetch_add(1, Ordering::SeqCst);
    }
}

// ------------------------------------------------------------------------------------------------
// Fake trust gate
// ------------------------------------------------------------------------------------------------

pub struct FakeTrust {
    pub expiration: Mutex<Option<SystemTime>>,
    pub rotate_result: Mutex<Result<(), TrustRotationError>>,
    pub rotated: Mutex<Vec<(DcgClient, Vec<u8>)>>,
}

impl FakeTrust {
    pub fn new(expiration: Option<SystemTime>) -> Arc<Self> {
        Arc::new(Self {
            expiration: Mutex::new(expiration),
            rotate_result: Mutex::new(Ok(())),
            rotated: Mutex::new(Vec::new()),
        })
    }
}

impl TrustGate for FakeTrust {
    fn is_trusted(&self, _partner: &DcgClient) -> bool {
        self.expiration.lock().unwrap().is_some()
    }

    fn partner_key_expiration(&self, _partner: &DcgClient) -> Option<SystemTime> {
        *self.expiration.lock().unwrap()
    }

    fn rotate_partner_certificate(
        &self,
        partner: &DcgClient,
        certificate_der: &[u8],
    ) -> Result<(), TrustRotationError> {
        self.rotated.lock().unwrap().push((partner.clone(), certificate_der.to_vec()));
        self.rotate_result.lock().unwrap().clone()
    }
}

// ------------------------------------------------------------------------------------------------
// The whole PC platform layer against the scripted phone
// ------------------------------------------------------------------------------------------------

/// The phone's `/SessionValidation` answer as a third-party client's notes report it: all three
/// capabilities, versions 14 and 3.
pub fn full_validation_response() -> TransportMessage {
    let body = pb::ProtoPlatformSessionValidationResponse {
        platform_capabilities: vec![
            pb::ProtoPlatformCapabilities::SessionValidation as i32,
            pb::ProtoPlatformCapabilities::PersistentMessageChannel as i32,
            pb::ProtoPlatformCapabilities::NanoTransportPreference as i32,
        ],
        persistent_messaging_channel_version: 14,
        nano_transport_preference_version: 3,
    }
    .encode_to_vec();
    platform(&[("ms-content-type", "application/x-binary")], &body)
}

pub struct World {
    pub pair: Pair,
    pub hub: Arc<FakeHubControl>,
    pub waker: Arc<FakeWaker>,
    pub sessions: Arc<PartnerSessions>,
    pub routes: Arc<PlatformRoutes>,
    pub trust: Arc<FakeTrust>,
    pub lock: Arc<FixedLock>,
    pub clock: Arc<ManualClock>,
    /// The phone's handlers for the routes the PC sends.
    pub phone_validation: Arc<Scripted>,
    pub phone_activate: Arc<Scripted>,
    pub phone_disconnect: Arc<Scripted>,
    pub phone_publish: Arc<Scripted>,
    pub phone_drm: Arc<Scripted>,
}

impl World {
    /// Builds the PC layer (call inside the runtime: it spawns). The hub's Prod connection is up and
    /// the phone is not on it yet.
    pub fn new(config: EngineConfig) -> World {
        Self::with_hub(config, HubState::Connected)
    }

    pub fn with_hub(config: EngineConfig, prod_state: HubState) -> World {
        let pair = Pair::new(config);
        let hub = FakeHubControl::new(prod_state);
        let waker = FakeWaker::new(hub.clone());
        let sessions =
            PartnerSessions::new(pair.config.clone(), hub.clone(), waker.clone(), pair.pc.clone(), pair.pc_tap.clone());
        sessions.start();

        let trust = FakeTrust::new(None);
        let lock = Arc::new(FixedLock::new(LockState::Unlocked));
        let clock = Arc::new(ManualClock::fixed());

        let routes = PlatformRoutes::install(PlatformDeps {
            config: pair.config.clone(),
            router: pair.pc.clone(),
            sessions: sessions.clone(),
            trust: trust.clone(),
            own: pc_id(),
            lock: lock.clone(),
            clock: clock.clone(),
        });

        let phone_validation =
            Scripted::install(&pair.phone, route::SESSION_VALIDATION, Box::new(|_| Some(full_validation_response())));
        let phone_activate = Scripted::silent(&pair.phone, route::PLATFORM_ACTIVATE_SESSION_REQUEST);
        let phone_disconnect = Scripted::silent(&pair.phone, route::PLATFORM_DISCONNECT_REQUEST);
        let phone_publish = Scripted::install(&pair.phone, route::CONTEXT_PUBLISH, Box::new(|_| Some(kvp_reply("{}"))));
        let phone_drm = Scripted::silent(&pair.phone, route::DEVICE_RESOURCE_MANAGER);

        World {
            pair,
            hub,
            waker,
            sessions,
            routes,
            trust,
            lock,
            clock,
            phone_validation,
            phone_activate,
            phone_disconnect,
            phone_publish,
            phone_drm,
        }
    }

    /// The phone appears on the hub and the PC reports it connected.
    pub async fn connect_phone(&self) {
        self.hub.partner_joins(&phone_id());
        assert_eq!(
            self.sessions.wait_connected(&phone_id(), Duration::from_secs(5)).await,
            phonelink_engine::platform::session::WaitOutcome::Connected
        );
        settle().await;
    }
}

/// `PlatformRequest` builder with the defaults most tests use.
pub fn request(target: DcgClient, message: TransportMessage) -> phonelink_engine::seams::PlatformRequest {
    phonelink_engine::seams::PlatformRequest {
        target,
        message,
        priority: 100,
        options: phonelink_engine::types::SendOptions::NONE,
        timeout: Duration::from_secs(120),
        trace: TraceContext::new_root(Vec::new()),
        cancel: CancellationToken::new(),
    }
}

/// A request from the phone to the PC with the defaults most tests use.
pub fn from_phone(message: TransportMessage) -> phonelink_engine::seams::PlatformRequest {
    request(pc_id(), message)
}

/// How a scripted phone answers a `/DeviceResourceManager` request; `None` sends nothing.
pub type DrmAnswer = Box<
    dyn Fn(&phonelink_engine::proto::platform_sdk::v1::DeviceResourceManagerMessage) -> Option<TransportMessage>
        + Send
        + Sync,
>;

/// A `/DeviceResourceManager` answer carrying `payload` with `response_type`.
pub fn drm_answer(payload: &[u8], response_type: Option<i32>) -> TransportMessage {
    let body = phonelink_engine::proto::platform_sdk::v1::DeviceResourceManagerResponseMessage {
        payload: payload.to_vec(),
        response_type,
    }
    .encode_to_vec();
    platform(&[("ms-content-type", "application/x-binary")], &body)
}

/// Sends a request from the phone router to the PC and returns the response.
pub async fn phone_request(pair: &Pair, message: TransportMessage) -> phonelink_engine::seams::PlatformResponse {
    pair.phone.request(request(pc_id(), message)).await
}
