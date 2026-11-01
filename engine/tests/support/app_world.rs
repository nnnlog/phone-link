//! Fakes of the platform seams the App layer consumes (`PlatformMessenger`,
//! `PartnerSessionControl`), a fake `AccountGate`, and a harness wiring them to an `AppLayer`.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, oneshot, watch};

use phonelink_engine::app::{AccountGate, AppError, AppLayer};
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::standins::{FixedLock, FixedReachability};
use phonelink_engine::edge::testing::{MemoryStore, memory_edges};
use phonelink_engine::edge::{Edges, LockState};
use phonelink_engine::seams::{
    ConnectOutcome, ConnectRequest, ConnectionFailure, PartnerSessionControl, PlatformMessenger, PlatformRequest,
    PlatformRequestStatus, PlatformResponse, SessionEvent,
};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{
    DcgClient, DcgSendStatus, DeviceInfoItem, Environment, HeaderMap, MessageType, TraceContext, TransportMessage,
};

pub const PHONE_ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
/// `customData` with `AsyncTrust` (0x20), so background requests pass the `AsyncTrust` gate.
pub const ASYNC_TRUST_CUSTOM_DATA: &str = r#"{"yppCapabilities":32}"#;

pub fn phone() -> DcgClient {
    DcgClient::new(PHONE_ID, Environment::Prod)
}

pub fn device_item(custom_data: Option<&str>) -> DeviceInfoItem {
    DeviceInfoItem {
        id: PHONE_ID.into(),
        is_linked: Some(true),
        os_name: Some("Android".into()),
        os_version: Some("14".into()),
        consent_version: Some(0),
        custom_data: custom_data.map(str::to_string),
        name: Some("Pixel".into()),
        ..Default::default()
    }
}

// ------------------------------------------------------------------------------------------------
// PlatformMessenger
// ------------------------------------------------------------------------------------------------

/// How the fake router answers one request.
pub enum Reply {
    Now(PlatformResponse),
    /// Answers when the sender fires; honours the request's timeout and cancel like the router.
    Later(oneshot::Receiver<PlatformResponse>),
    /// Never answers: `ResponseTimedOut` after the request's timeout, `Canceled` on cancel.
    Hang,
}

type Responder = Box<dyn FnMut(&PlatformRequest) -> Reply + Send>;

#[derive(Debug, Clone)]
pub struct RecordedResponse {
    pub target: DcgClient,
    pub request: TransportMessage,
    pub response: TransportMessage,
    pub priority: u32,
    pub trace: TraceContext,
}

impl RecordedResponse {
    pub fn values(&self) -> ValueSet {
        valueset::decode(&self.response.payload).expect("answer is a ValueSet")
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.response.headers.get(name).map(str::to_string)
    }
}

pub struct FakeMessenger {
    requests: Mutex<Vec<PlatformRequest>>,
    responses: Mutex<Vec<RecordedResponse>>,
    responder: Mutex<Responder>,
    request_count: watch::Sender<usize>,
    response_count: watch::Sender<usize>,
}

/// A successful response carrying `values` (and the `result` header when it has an `Int32` result).
pub fn response_with(values: ValueSet) -> PlatformResponse {
    let mut headers = HeaderMap::new();
    headers.insert("_originalRequestId", "x");
    headers.insert("_route", "/internal/response");
    if let Some(r) = values.i32("result") {
        headers.insert("result", r.to_string());
    }

    PlatformResponse {
        status: PlatformRequestStatus::Success,
        message: Some(TransportMessage::new(MessageType::App, headers, valueset::encode(&values))),
    }
}

pub fn result_response(code: i32) -> PlatformResponse {
    response_with(ValueSet::new().with("result", code))
}

pub fn status_response(status: PlatformRequestStatus) -> PlatformResponse {
    PlatformResponse { status, message: None }
}

impl FakeMessenger {
    /// Answers every request `{ result: 0 }`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            responses: Mutex::new(Vec::new()),
            responder: Mutex::new(Box::new(|_| Reply::Now(result_response(0)))),
            request_count: watch::channel(0).0,
            response_count: watch::channel(0).0,
        })
    }

    pub fn set_responder(&self, f: impl FnMut(&PlatformRequest) -> Reply + Send + 'static) {
        *self.responder.lock().unwrap() = Box::new(f);
    }

    /// Every request answered with `response`.
    pub fn answer_all(&self, response: PlatformResponse) {
        self.set_responder(move |_| Reply::Now(response.clone()));
    }

    pub fn requests(&self) -> Vec<PlatformRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn requests_on(&self, route: &str) -> Vec<PlatformRequest> {
        self.requests().into_iter().filter(|r| r.message.route() == Some(route)).collect()
    }

    pub fn responses(&self) -> Vec<RecordedResponse> {
        self.responses.lock().unwrap().clone()
    }

    pub async fn wait_requests(&self, n: usize) {
        let mut rx = self.request_count.subscribe();
        tokio::time::timeout(Duration::from_secs(3600), rx.wait_for(|c| *c >= n))
            .await
            .expect("requests did not arrive")
            .unwrap();
    }

    pub async fn wait_responses(&self, n: usize) {
        let mut rx = self.response_count.subscribe();
        tokio::time::timeout(Duration::from_secs(3600), rx.wait_for(|c| *c >= n))
            .await
            .expect("responses did not arrive")
            .unwrap();
    }
}

#[async_trait]
impl PlatformMessenger for FakeMessenger {
    async fn request(&self, request: PlatformRequest) -> PlatformResponse {
        let reply = (self.responder.lock().unwrap())(&request);
        let timeout = request.timeout;
        let cancel = request.cancel.clone();

        let n = {
            let mut v = self.requests.lock().unwrap();
            v.push(request);
            v.len()
        };
        self.request_count.send_replace(n);

        match reply {
            Reply::Now(r) => r,
            Reply::Later(rx) => tokio::select! {
                r = rx => r.unwrap_or(status_response(PlatformRequestStatus::Canceled)),
                _ = tokio::time::sleep(timeout) => status_response(PlatformRequestStatus::ResponseTimedOut),
                _ = cancel.cancelled() => status_response(PlatformRequestStatus::Canceled),
            },
            Reply::Hang => tokio::select! {
                _ = tokio::time::sleep(timeout) => status_response(PlatformRequestStatus::ResponseTimedOut),
                _ = cancel.cancelled() => status_response(PlatformRequestStatus::Canceled),
            },
        }
    }

    async fn respond(
        &self,
        target: &DcgClient,
        request: &TransportMessage,
        response: TransportMessage,
        priority: u32,
        trace: TraceContext,
    ) -> DcgSendStatus {
        let n = {
            let mut v = self.responses.lock().unwrap();
            v.push(RecordedResponse { target: target.clone(), request: request.clone(), response, priority, trace });
            v.len()
        };
        self.response_count.send_replace(n);
        DcgSendStatus::Success
    }
}

// ------------------------------------------------------------------------------------------------
// PartnerSessionControl
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ConnectBehavior {
    /// Becomes connected and reports `Connected`.
    Connect,
    /// Reports `Pending` without a last failure; connects later only if the test calls
    /// `set_connected(true)`.
    Pending,
    /// Reports `Pending { last_failure: Some(..) }`: an earlier attempt failed, the attempts go on.
    PendingAfter(ConnectionFailure),
    /// Reports `Pending`, but the phone came up just as the feedback window ended.
    PendingButConnected,
    Fail(ConnectionFailure),
    /// Never returns.
    Hang,
    /// Waits, then behaves as the inner behaviour.
    Delayed(Duration, Box<ConnectBehavior>),
}

pub struct FakeSessions {
    connected: watch::Sender<bool>,
    behavior: Mutex<ConnectBehavior>,
    connects: Mutex<Vec<(DcgClient, ConnectRequest)>>,
    events: broadcast::Sender<SessionEvent>,
}

impl FakeSessions {
    pub fn new(connected: bool) -> Arc<Self> {
        Arc::new(Self {
            connected: watch::channel(connected).0,
            behavior: Mutex::new(ConnectBehavior::Connect),
            connects: Mutex::new(Vec::new()),
            events: broadcast::channel(16).0,
        })
    }
    pub fn set_connected(&self, connected: bool) {
        self.connected.send_replace(connected);
    }
    pub fn set_behavior(&self, b: ConnectBehavior) {
        *self.behavior.lock().unwrap() = b;
    }
    pub fn connects(&self) -> Vec<(DcgClient, ConnectRequest)> {
        self.connects.lock().unwrap().clone()
    }
    pub fn send_event(&self, e: SessionEvent) {
        let _ = self.events.send(e);
    }
}

#[async_trait]
impl PartnerSessionControl for FakeSessions {
    async fn connect(&self, partner: &DcgClient, request: ConnectRequest) -> ConnectOutcome {
        self.connects.lock().unwrap().push((partner.clone(), request));
        let mut behavior = self.behavior.lock().unwrap().clone();
        loop {
            match behavior {
                ConnectBehavior::Connect => {
                    self.connected.send_replace(true);
                    return ConnectOutcome::Connected;
                }
                ConnectBehavior::Pending => return ConnectOutcome::Pending { last_failure: None },
                ConnectBehavior::PendingAfter(f) => return ConnectOutcome::Pending { last_failure: Some(f) },
                ConnectBehavior::PendingButConnected => {
                    self.connected.send_replace(true);
                    return ConnectOutcome::Pending { last_failure: None };
                }
                ConnectBehavior::Fail(f) => return ConnectOutcome::Failed(f),
                ConnectBehavior::Hang => std::future::pending().await,
                ConnectBehavior::Delayed(d, inner) => {
                    tokio::time::sleep(d).await;
                    behavior = *inner;
                }
            }
        }
    }

    fn is_connected(&self, _partner: &DcgClient) -> bool {
        *self.connected.borrow()
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }
}

// ------------------------------------------------------------------------------------------------
// AccountGate
// ------------------------------------------------------------------------------------------------

pub struct FakeAccount(pub AtomicBool);

impl FakeAccount {
    pub fn set(&self, ready: bool) {
        self.0.store(ready, Ordering::SeqCst);
    }
}

#[async_trait]
impl AccountGate for FakeAccount {
    async fn is_ready(&self, _phone: &DcgClient) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

// ------------------------------------------------------------------------------------------------
// Harness
// ------------------------------------------------------------------------------------------------

/// An `AppLayer` over fresh fakes and `store` (for construction failures).
pub fn try_app(store: Arc<MemoryStore>, config: EngineConfig) -> Result<Arc<AppLayer>, AppError> {
    let edges = Edges { store, ..memory_edges() };
    AppLayer::new(
        Arc::new(config),
        edges,
        FakeMessenger::new(),
        FakeSessions::new(true),
        Arc::new(FakeAccount(AtomicBool::new(true))),
        MemoryTap::new(),
    )
}

pub struct Harness {
    pub app: Arc<AppLayer>,
    pub messenger: Arc<FakeMessenger>,
    pub sessions: Arc<FakeSessions>,
    pub account: Arc<FakeAccount>,
    pub store: Arc<MemoryStore>,
    pub lock: Arc<FixedLock>,
    pub reachability: Arc<FixedReachability>,
    pub tap: Arc<MemoryTap>,
}

impl Harness {
    /// A connected, unlocked, online harness whose phone is selected with `AsyncTrust`.
    pub fn new() -> Self {
        let h = Self::without_phone(Arc::new(MemoryStore::new()), EngineConfig::default());
        h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
        h
    }

    pub fn with_custom_data(custom_data: Option<&str>) -> Self {
        let h = Self::without_phone(Arc::new(MemoryStore::new()), EngineConfig::default());
        h.app.set_phone(phone(), &device_item(custom_data)).unwrap();
        h
    }

    pub fn without_phone(store: Arc<MemoryStore>, config: EngineConfig) -> Self {
        let messenger = FakeMessenger::new();
        let sessions = FakeSessions::new(true);
        let account = Arc::new(FakeAccount(AtomicBool::new(true)));
        let lock = Arc::new(FixedLock::new(LockState::Unlocked));
        let reachability = Arc::new(FixedReachability::online());
        let tap = MemoryTap::new();

        let edges =
            Edges { store: store.clone(), lock: lock.clone(), reachability: reachability.clone(), ..memory_edges() };
        let app =
            AppLayer::new(Arc::new(config), edges, messenger.clone(), sessions.clone(), account.clone(), tap.clone())
                .unwrap();

        Self { app, messenger, sessions, account, store, lock, reachability, tap }
    }
}

// ------------------------------------------------------------------------------------------------
// Inbound messages
// ------------------------------------------------------------------------------------------------

pub fn message(headers: &[(&str, &str)], payload: Vec<u8>) -> TransportMessage {
    TransportMessage::new(MessageType::App, HeaderMap::from_pairs(headers.iter().copied()).unwrap(), payload)
}

/// A `/legacy/phonecontent` request with `_requestId: req-1`, carrying `contentType`,
/// `contractVersion: Double 3.99`, `correlationVector: cv-1` and `extra`.
pub fn phone_content(content_type: &str, extra: ValueSet) -> TransportMessage {
    let mut vs = ValueSet::new()
        .with("contentType", content_type)
        .with("contractVersion", 3.99f64)
        .with("correlationVector", "cv-1");
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }

    message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs))
}

pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

pub fn trace() -> TraceContext {
    TraceContext::new_root(Vec::new())
}
