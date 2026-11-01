//! A fake `SignalR` relay hub on `127.0.0.1` for offline tests of the relay connection and of
//! anything layered on it. `tests/main.rs` declares it as `crate::support::fake_hub`.
//!
//! # What it serves
//!
//! - `POST /{hub}/negotiate` — answers the scripted [`NegotiateReply`] (default: a standard
//!   negotiate response with `connectionToken = "token-<n>"` and the WebSockets/Binary transport),
//!   recording method, path, query and every header ([`FakeHub::negotiates`]).
//! - `GET /{hub}/` with a WebSocket upgrade — answers the scripted [`UpgradeReply`] (default:
//!   accept), records the request ([`FakeHub::upgrades`]) and runs one [`HubSession`] per socket.
//!
//! Point the engine at it with `config.endpoints.prod_base = hub.base_url()` (the client appends
//! `relayhub/`). Any `{hub}` path segment works, so a negotiate redirect to
//! `format!("{}redirected/", hub.base_url())` lands on the same server.
//!
//! # A session
//!
//! The session reads the client's handshake (recorded with whether it came in a binary message),
//! answers per [`HandshakeReply`] (as a binary message by default, optionally as text, optionally
//! followed in the same message by hub frames — [`FakeHub::set_coalesce_after_handshake`]), then:
//!
//! - decodes every client frame; pings are timestamped ([`HubSession::pings`]) and invocations
//!   recorded ([`RecordedInvocation`]) and auto-completed per [`CompletionMode`] (default void,
//!   overridable per hub method with [`FakeHub::set_completion_for`]);
//! - runs every hook installed with [`FakeHub::on_invocation`] after the completion — a hook can
//!   push messages back, which is how a fake phone is scripted ([`FakeHub::auto_ack_fragments`]
//!   installs one that acknowledges every `ms-dcg` fragment sent to a peer id);
//! - pushes whatever the test sends: [`HubSession::send`] (one frame per WebSocket message),
//!   [`HubSession::send_coalesced`], [`HubSession::send_raw`] (arbitrary bytes, e.g. malformed
//!   frames), [`HubSession::send_text`];
//! - optionally pings the client ([`FakeHub::set_ping_interval`]);
//! - can stop reading the client's bytes while it keeps sending ([`HubSession::pause_reading`],
//!   [`HubSession::resume_reading`]), so the client's writes back up in the TCP buffers;
//! - can be killed abruptly ([`HubSession::drop_socket`], no close frame) or closed with a
//!   WebSocket close frame ([`HubSession::close_ws`]).
//!
//! The hub accepts WebSocket messages and frames up to [`MAX_MESSAGE_SIZE`] (the client lifts its
//! own limits, so a test may send a single frame of tens of MiB).
//!
//! The builders at the end ([`on_connected`], [`on_partner_connected`], [`on_receive_message`], …)
//! produce the hub → client invocations of the inbound hub methods. For message-level scripting,
//! [`message_fragments`] cuts a peer's [`TransportMessage`] into `ms-dcg` fragments
//! ([`HubSession::send_fragments`] sends them) and [`reassemble`] turns the fragments a peer
//! received back into messages.
//!
//! Waiting helpers ([`FakeHub::session`], [`FakeHub::wait_invocation`],
//! [`HubSession::wait_invocation`], [`HubSession::wait_pings`], [`HubSession::wait_closed`]) panic
//! with a description when their timeout passes, so a test fails instead of hanging.

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use phonelink_engine::codec::dcg_packet::{Ack, Fragment, ParsedPacket, VERSION, parse_packet};
use phonelink_engine::codec::envelope;
use phonelink_engine::codec::hub::{
    self, CompletionResult, HubMessage, MultiplexPacket, OnConnectedPayload, TraceContextPacket, method,
};
use phonelink_engine::types::{TraceContext, TransportMessage};
use rmpv::Value;
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// The largest WebSocket message and frame the hub accepts from the client.
pub const MAX_MESSAGE_SIZE: usize = 256 << 20;

/// How `POST …/negotiate` answers.
#[derive(Debug, Clone)]
pub enum NegotiateReply {
    /// `{ negotiateVersion: 1, connectionId: "conn-<n>", connectionToken: "token-<n>",
    /// availableTransports: [{ transport: "WebSockets", transferFormats: ["Text", "Binary"] }] }`.
    Ok,
    /// An HTTP error with this status and an empty body.
    Status(u16),
    /// The Azure-style redirect `{ url, accessToken }`.
    Redirect { url: String, access_token: Option<String> },
    /// This JSON body with status 200.
    Json(serde_json::Value),
}

/// How the WebSocket upgrade answers.
#[derive(Debug, Clone)]
pub enum UpgradeReply {
    Accept,
    /// Refuse the upgrade with this HTTP status.
    Status(u16),
}

/// How the hub answers the client's handshake.
#[derive(Debug, Clone)]
pub enum HandshakeReply {
    /// `{}` + `0x1E`.
    Ok,
    /// `{"error": "<text>"}` + `0x1E`, then the socket is closed.
    Error(String),
    /// Never answer.
    Silent,
}

/// How client invocations that carry an invocation id are completed.
#[derive(Debug, Clone)]
pub enum CompletionMode {
    /// `[3, {}, id, 2]`.
    Void,
    /// `[3, {}, id, 1, "<text>"]`.
    Error(String),
    /// Never complete.
    Never,
}

/// One recorded HTTP request (negotiate or upgrade).
#[derive(Debug, Clone)]
pub struct HttpRecord {
    pub at: Instant,
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    /// Every header as received (names are lower-case on the wire).
    pub headers: Vec<(String, String)>,
}

impl HttpRecord {
    /// The value of `name` (case-insensitive); panics when the header occurs more than once.
    pub fn header(&self, name: &str) -> Option<&str> {
        let mut values = self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name));
        let first = values.next().map(|(_, v)| v.as_str());
        assert!(values.next().is_none(), "header {name} occurs more than once");
        first
    }

    pub fn query_param(&self, key: &str) -> Option<String> {
        let query = self.query.as_deref()?;
        url_pairs(query).into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

fn url_pairs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(k), percent_decode(v))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap();
            out.push(u8::from_str_radix(hex, 16).unwrap());
            i += 3;
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(out).unwrap()
}

/// The client's handshake bytes and whether they came in a binary WebSocket message.
#[derive(Debug, Clone)]
pub struct HandshakeRecord {
    pub bytes: Vec<u8>,
    pub binary: bool,
}

/// One invocation the client made.
#[derive(Debug, Clone)]
pub struct RecordedInvocation {
    pub at: Instant,
    /// Index of the session it arrived on.
    pub session: usize,
    pub invocation_id: Option<String>,
    pub target: String,
    pub arguments: Vec<Value>,
    /// The frame payload (the `MessagePack` array, without the length prefix).
    pub payload: Vec<u8>,
}

impl RecordedInvocation {
    /// `targetDcgClientId` (argument 1) of `SendMessageAsync` / `SendSessionBasedMessageAsync` /
    /// `SendConnectedAsync`.
    pub fn target_id(&self) -> Option<&str> {
        self.arguments.get(1)?.as_str()
    }

    /// The `HubRelayTraceContextPacket` (argument 0).
    pub fn trace(&self) -> Option<TraceContextPacket> {
        TraceContextPacket::from_value(self.arguments.first()?).ok().flatten()
    }

    /// The `HubRelayMultiplexPacket` (argument 2) of a message send.
    pub fn packet(&self) -> Option<MultiplexPacket> {
        if self.target != method::SEND_MESSAGE && self.target != method::SEND_SESSION_BASED_MESSAGE {
            return None;
        }
        MultiplexPacket::from_value(self.arguments.get(2)?).ok()
    }

    /// `connectionSessionId` (argument 3) of `SendSessionBasedMessageAsync`.
    pub fn connection_session_id(&self) -> Option<&str> {
        self.arguments.get(3)?.as_str()
    }

    /// The packet parsed as an `ms-dcg` fragment or ack.
    pub fn dcg(&self) -> Option<ParsedPacket> {
        parse_packet(&self.packet()?).ok()
    }
}

/// A hook run for every client invocation, after its completion was queued.
pub type InvocationHook = Arc<dyn Fn(&Arc<HubSession>, &RecordedInvocation) + Send + Sync>;

#[derive(Clone)]
struct Options {
    handshake: HandshakeReply,
    handshake_as_text: bool,
    coalesce_after_handshake: Vec<HubMessage>,
    completion: CompletionMode,
    completion_for: HashMap<String, CompletionMode>,
    ping_interval: Option<Duration>,
    default_negotiate: NegotiateReply,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            handshake: HandshakeReply::Ok,
            handshake_as_text: false,
            coalesce_after_handshake: Vec::new(),
            completion: CompletionMode::Void,
            completion_for: HashMap::new(),
            ping_interval: None,
            default_negotiate: NegotiateReply::Ok,
        }
    }
}

struct HubShared {
    options: Mutex<Options>,
    negotiate_script: Mutex<VecDeque<NegotiateReply>>,
    upgrade_script: Mutex<VecDeque<UpgradeReply>>,
    negotiates: Mutex<Vec<HttpRecord>>,
    upgrades: Mutex<Vec<HttpRecord>>,
    sessions: Mutex<Vec<Arc<HubSession>>>,
    hooks: Mutex<Vec<InvocationHook>>,
    peer_fragments: Mutex<HashMap<String, Vec<Fragment>>>,
    tokens_issued: Mutex<u64>,
    changed: Notify,
}

impl HubShared {
    fn completion_for(&self, target: &str) -> CompletionMode {
        let options = self.options.lock().unwrap();
        options.completion_for.get(target).cloned().unwrap_or_else(|| options.completion.clone())
    }
}

/// The fake hub; dropping it stops the server and every session.
pub struct FakeHub {
    shared: Arc<HubShared>,
    addr: SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for FakeHub {
    fn drop(&mut self) {
        self.server.abort();
        for session in self.shared.sessions.lock().unwrap().iter() {
            session.kill.cancel();
        }
    }
}

enum Outgoing {
    Binary(Vec<u8>),
    Text(String),
    CloseFrame,
}

/// One WebSocket the client opened.
pub struct HubSession {
    pub index: usize,
    pub upgrade: HttpRecord,
    hub: Arc<HubShared>,
    handshake: Mutex<Option<HandshakeRecord>>,
    messages: Mutex<Vec<HubMessage>>,
    invocations: Mutex<Vec<RecordedInvocation>>,
    pings: Mutex<Vec<Instant>>,
    errors: Mutex<Vec<String>>,
    out: mpsc::UnboundedSender<Outgoing>,
    /// `false` while the session does not read the client's bytes.
    reading: watch::Sender<bool>,
    kill: CancellationToken,
    closed: CancellationToken,
    changed: Notify,
}

async fn wait_until<T>(
    notify: &Notify,
    timeout: Duration,
    what: impl Fn() -> String,
    mut check: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();

        if let Some(value) = check() {
            return value;
        }

        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            if let Some(value) = check() {
                return value;
            }
            panic!("fake hub: timed out after {timeout:?} waiting for {}", what());
        }
    }
}

impl FakeHub {
    pub async fn start() -> Self {
        let shared = Arc::new(HubShared {
            options: Mutex::new(Options::default()),
            negotiate_script: Mutex::new(VecDeque::new()),
            upgrade_script: Mutex::new(VecDeque::new()),
            negotiates: Mutex::new(Vec::new()),
            upgrades: Mutex::new(Vec::new()),
            sessions: Mutex::new(Vec::new()),
            hooks: Mutex::new(Vec::new()),
            peer_fragments: Mutex::new(HashMap::new()),
            tokens_issued: Mutex::new(0),
            changed: Notify::new(),
        });

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");

        let app = Router::new()
            .route("/{hub}/negotiate", post(negotiate_handler))
            .route("/{hub}/", get(upgrade_handler))
            .with_state(shared.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("fake hub server");
        });

        Self { shared, addr, server }
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:<port>/` — an environment base (`config.endpoints.prod_base`).
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.addr.port())
    }

    /// Queues the answer of the next negotiate (then the default answer applies again).
    pub fn push_negotiate(&self, reply: NegotiateReply) {
        self.shared.negotiate_script.lock().unwrap().push_back(reply);
    }

    /// The answer of every negotiate with nothing queued (initially [`NegotiateReply::Ok`]).
    pub fn set_default_negotiate(&self, reply: NegotiateReply) {
        self.shared.options.lock().unwrap().default_negotiate = reply;
    }

    /// Queues the answer of the next WebSocket upgrade.
    pub fn push_upgrade(&self, reply: UpgradeReply) {
        self.shared.upgrade_script.lock().unwrap().push_back(reply);
    }

    pub fn negotiates(&self) -> Vec<HttpRecord> {
        self.shared.negotiates.lock().unwrap().clone()
    }

    pub fn upgrades(&self) -> Vec<HttpRecord> {
        self.shared.upgrades.lock().unwrap().clone()
    }

    pub fn set_handshake(&self, reply: HandshakeReply) {
        self.shared.options.lock().unwrap().handshake = reply;
    }

    /// Answer the handshake in a text message (coalesced frames then follow in a binary one).
    pub fn set_handshake_as_text(&self, text: bool) {
        self.shared.options.lock().unwrap().handshake_as_text = text;
    }

    /// Hub messages appended after the handshake answer's `0x1E`, in the same WebSocket message.
    pub fn set_coalesce_after_handshake(&self, messages: Vec<HubMessage>) {
        self.shared.options.lock().unwrap().coalesce_after_handshake = messages;
    }

    pub fn set_completion(&self, mode: CompletionMode) {
        self.shared.options.lock().unwrap().completion = mode;
    }

    /// The completion mode for invocations of one hub method (`SendMessageAsync`, …).
    pub fn set_completion_for(&self, target: &str, mode: CompletionMode) {
        self.shared.options.lock().unwrap().completion_for.insert(target.to_string(), mode);
    }

    /// Ping the client (`[6]`) at this interval on sessions opened afterwards.
    pub fn set_ping_interval(&self, interval: Option<Duration>) {
        self.shared.options.lock().unwrap().ping_interval = interval;
    }

    /// Runs `hook` for every client invocation on every session (after its completion).
    pub fn on_invocation(&self, hook: impl Fn(&Arc<HubSession>, &RecordedInvocation) + Send + Sync + 'static) {
        self.shared.hooks.lock().unwrap().push(Arc::new(hook));
    }

    /// Scripts a peer `peer_id` that acknowledges every `ms-dcg` fragment the client sends to it:
    /// the ack (`Ack::success(SessionId, SequenceNumber)`) comes back as `OnReceiveMessage` from
    /// `peer_id`, or as `OnReceiveSessionBasedMessage` with the same `connectionSessionId` when the
    /// fragment was session-based. The fragments are kept in [`FakeHub::peer_fragments`].
    pub fn auto_ack_fragments(&self, peer_id: &str) {
        let peer = peer_id.to_string();
        let shared = Arc::downgrade(&self.shared);
        self.on_invocation(move |session, invocation| {
            if invocation.target_id() != Some(peer.as_str()) {
                return;
            }

            let Some(ParsedPacket::Fragment(fragment)) = invocation.dcg() else { return };
            let ack = Ack::success(fragment.session_id.clone(), fragment.sequence_number).to_packet();
            match invocation.connection_session_id() {
                Some(id) => session.send(on_receive_session_based_message(&peer, &ack, id)),
                None => session.send(on_receive_message(&peer, &ack)),
            }

            if let Some(shared) = shared.upgrade() {
                shared.peer_fragments.lock().unwrap().entry(peer.clone()).or_default().push(fragment);
                shared.changed.notify_waiters();
            }
        });
    }

    /// The fragments an [`FakeHub::auto_ack_fragments`] peer received, in arrival order.
    pub fn peer_fragments(&self, peer_id: &str) -> Vec<Fragment> {
        self.shared.peer_fragments.lock().unwrap().get(peer_id).cloned().unwrap_or_default()
    }

    /// Waits until the peer received `n` fragments.
    pub async fn wait_peer_fragments(&self, peer_id: &str, n: usize, timeout: Duration) -> Vec<Fragment> {
        wait_until(
            &self.shared.changed,
            timeout,
            || format!("{n} fragments for {peer_id}"),
            || {
                let fragments = self.peer_fragments(peer_id);
                (fragments.len() >= n).then_some(fragments)
            },
        )
        .await
    }

    pub fn sessions(&self) -> Vec<Arc<HubSession>> {
        self.shared.sessions.lock().unwrap().clone()
    }

    /// Waits (up to 5 s) for the `index`-th WebSocket session (0-based) whose client handshake has
    /// been received (the hub's answer is sent right after).
    pub async fn session(&self, index: usize) -> Arc<HubSession> {
        wait_until(
            &self.shared.changed,
            Duration::from_secs(5),
            || format!("session {index}"),
            || {
                self.shared
                    .sessions
                    .lock()
                    .unwrap()
                    .get(index)
                    .filter(|s| s.handshake.lock().unwrap().is_some())
                    .cloned()
            },
        )
        .await
    }

    /// Every invocation on every session, in arrival order per session.
    pub fn all_invocations(&self) -> Vec<RecordedInvocation> {
        let mut all: Vec<RecordedInvocation> = self.sessions().iter().flat_map(|s| s.invocations()).collect();
        all.sort_by_key(|i| i.at);
        all
    }

    /// Waits for an invocation (on any session) matching `pred`.
    pub async fn wait_invocation(
        &self,
        timeout: Duration,
        pred: impl Fn(&RecordedInvocation) -> bool,
    ) -> RecordedInvocation {
        wait_until(
            &self.shared.changed,
            timeout,
            || format!("an invocation; got {:?}", self.all_invocations().iter().map(|i| &i.target).collect::<Vec<_>>()),
            || self.all_invocations().into_iter().find(|i| pred(i)),
        )
        .await
    }
}

impl HubSession {
    fn push(&self, outgoing: Outgoing) {
        // A closed session drops what the test still pushes.
        let _ = self.out.send(outgoing);
    }

    /// One hub message as one frame in its own binary WebSocket message.
    pub fn send(&self, message: HubMessage) {
        self.push(Outgoing::Binary(hub::encode_frame(&message)));
    }

    /// Several frames coalesced in one binary WebSocket message.
    pub fn send_coalesced(&self, messages: &[HubMessage]) {
        let mut bytes = Vec::new();
        for message in messages {
            bytes.extend(hub::encode_frame(message));
        }
        self.push(Outgoing::Binary(bytes));
    }

    /// Arbitrary bytes in one binary WebSocket message.
    pub fn send_raw(&self, bytes: Vec<u8>) {
        self.push(Outgoing::Binary(bytes));
    }

    pub fn send_text(&self, text: &str) {
        self.push(Outgoing::Text(text.to_string()));
    }

    /// A hub → client invocation without an invocation id.
    pub fn invoke(&self, target: &str, arguments: Vec<Value>) {
        self.send(invocation(target, arguments));
    }

    /// Sends each fragment from `peer_id` as its own `OnReceiveMessage` (see [`message_fragments`]).
    pub fn send_fragments(&self, peer_id: &str, fragments: &[Fragment]) {
        for fragment in fragments {
            self.send(on_receive_message(peer_id, &fragment.to_packet()));
        }
    }

    /// Ends the socket abruptly (no close frame).
    pub fn drop_socket(&self) {
        self.kill.cancel();
    }

    /// Sends a WebSocket close frame and ends the session.
    pub fn close_ws(&self) {
        self.push(Outgoing::CloseFrame);
    }

    /// Stops reading the client's bytes (they back up in the socket buffers, so a large client
    /// write stalls); the session keeps sending what the test pushes and its pings.
    pub fn pause_reading(&self) {
        self.reading.send_replace(false);
    }

    /// Reads the client's bytes again.
    pub fn resume_reading(&self) {
        self.reading.send_replace(true);
    }

    pub fn handshake(&self) -> Option<HandshakeRecord> {
        self.handshake.lock().unwrap().clone()
    }

    /// Every decoded client message (pings included).
    pub fn messages(&self) -> Vec<HubMessage> {
        self.messages.lock().unwrap().clone()
    }

    pub fn invocations(&self) -> Vec<RecordedInvocation> {
        self.invocations.lock().unwrap().clone()
    }

    pub fn pings(&self) -> Vec<Instant> {
        self.pings.lock().unwrap().clone()
    }

    /// Client bytes the fake hub could not decode.
    pub fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    pub async fn wait_invocation(
        &self,
        timeout: Duration,
        pred: impl Fn(&RecordedInvocation) -> bool,
    ) -> RecordedInvocation {
        wait_until(
            &self.changed,
            timeout,
            || {
                format!(
                    "an invocation on session {}; got {:?}",
                    self.index,
                    self.invocations().iter().map(|i| &i.target).collect::<Vec<_>>()
                )
            },
            || self.invocations().into_iter().find(|i| pred(i)),
        )
        .await
    }

    /// Waits for `n` invocations of `target` on this session.
    pub async fn wait_invocations_of(&self, target: &str, n: usize, timeout: Duration) -> Vec<RecordedInvocation> {
        wait_until(
            &self.changed,
            timeout,
            || format!("{n} {target} invocations on session {}", self.index),
            || {
                let found: Vec<_> = self.invocations().into_iter().filter(|i| i.target == target).collect();
                (found.len() >= n).then_some(found)
            },
        )
        .await
    }

    pub async fn wait_pings(&self, n: usize, timeout: Duration) -> Vec<Instant> {
        wait_until(
            &self.changed,
            timeout,
            || format!("{n} pings on session {}", self.index),
            || {
                let pings = self.pings();
                (pings.len() >= n).then_some(pings)
            },
        )
        .await
    }

    /// Waits for the socket to end; `false` when it is still open after `timeout`.
    pub async fn wait_closed(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.closed.cancelled()).await.is_ok()
    }

    fn notify(&self) {
        self.changed.notify_waiters();
        self.hub.changed.notify_waiters();
    }
}

// ------------------------------------------------------------------------------------------------
// HTTP handlers and the session task
// ------------------------------------------------------------------------------------------------

fn record(method: &str, uri: &Uri, headers: &HeaderMap) -> HttpRecord {
    HttpRecord {
        at: Instant::now(),
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().map(str::to_string),
        headers: headers
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned()))
            .collect(),
    }
}

async fn negotiate_handler(
    State(shared): State<Arc<HubShared>>,
    Path(_hub): Path<String>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    shared.negotiates.lock().unwrap().push(record("POST", &uri, &headers));
    shared.changed.notify_waiters();

    let reply = shared
        .negotiate_script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| shared.options.lock().unwrap().default_negotiate.clone());

    match reply {
        NegotiateReply::Ok => {
            let n = {
                let mut issued = shared.tokens_issued.lock().unwrap();
                *issued += 1;
                *issued
            };
            axum::Json(serde_json::json!({
                "negotiateVersion": 1,
                "connectionId": format!("conn-{n}"),
                "connectionToken": format!("token-{n}"),
                "availableTransports": [
                    { "transport": "WebSockets", "transferFormats": ["Text", "Binary"] },
                    { "transport": "LongPolling", "transferFormats": ["Text", "Binary"] }
                ]
            }))
            .into_response()
        }
        NegotiateReply::Status(status) => StatusCode::from_u16(status).expect("valid HTTP status").into_response(),
        NegotiateReply::Redirect { url, access_token } => {
            axum::Json(serde_json::json!({ "url": url, "accessToken": access_token })).into_response()
        }
        NegotiateReply::Json(body) => axum::Json(body).into_response(),
    }
}

async fn upgrade_handler(
    State(shared): State<Arc<HubShared>>,
    Path(_hub): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let request = record("GET", &uri, &headers);
    shared.upgrades.lock().unwrap().push(request.clone());
    shared.changed.notify_waiters();

    if let Some(UpgradeReply::Status(status)) = shared.upgrade_script.lock().unwrap().pop_front() {
        return StatusCode::from_u16(status).expect("valid HTTP status").into_response();
    }

    let (tx, rx) = mpsc::unbounded_channel();
    let session = {
        let mut sessions = shared.sessions.lock().unwrap();
        let session = Arc::new(HubSession {
            index: sessions.len(),
            upgrade: request,
            hub: shared.clone(),
            handshake: Mutex::new(None),
            messages: Mutex::new(Vec::new()),
            invocations: Mutex::new(Vec::new()),
            pings: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
            out: tx,
            reading: watch::channel(true).0,
            kill: CancellationToken::new(),
            closed: CancellationToken::new(),
            changed: Notify::new(),
        });
        sessions.push(session.clone());
        session
    };

    let ws = ws.max_message_size(MAX_MESSAGE_SIZE).max_frame_size(MAX_MESSAGE_SIZE);
    ws.on_upgrade(move |socket| async move {
        run_session(shared, socket, session.clone(), rx).await;
        session.closed.cancel();
        session.notify();
    })
}

async fn tick(interval: &mut Option<tokio::time::Interval>) {
    match interval {
        Some(interval) => {
            interval.tick().await;
        }
        None => std::future::pending::<()>().await,
    }
}

type HubTx = futures_util::stream::SplitSink<WebSocket, Message>;
type HubRx = futures_util::stream::SplitStream<WebSocket>;

async fn run_session(
    shared: Arc<HubShared>,
    socket: WebSocket,
    session: Arc<HubSession>,
    out: mpsc::UnboundedReceiver<Outgoing>,
) {
    let (mut tx, mut rx) = socket.split();
    let Some((handshake, binary, mut client)) = read_handshake(&session, &mut rx).await else { return };

    let options = shared.options.lock().unwrap().clone();
    *session.handshake.lock().unwrap() = Some(HandshakeRecord { bytes: handshake, binary });
    session.notify();

    if !answer_handshake(&options, &mut tx).await {
        return;
    }

    process_client_frames(&shared, &session, &mut client);
    serve(&shared, &session, &options, (tx, rx), out, client).await;
}

/// The client's handshake: the bytes up to and including the record separator, whether it came as
/// binary, and the bytes after it. `None` when the session is killed or the socket ends first.
async fn read_handshake(session: &HubSession, rx: &mut HubRx) -> Option<(Vec<u8>, bool, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut binary = None;
    let end = loop {
        let message = tokio::select! {
            _ = session.kill.cancelled() => return None,
            message = rx.next() => message,
        };
        match message {
            Some(Ok(Message::Binary(bytes))) => {
                binary.get_or_insert(true);
                buf.extend_from_slice(&bytes);
            }
            Some(Ok(Message::Text(text))) => {
                binary.get_or_insert(false);
                buf.extend_from_slice(text.as_str().as_bytes());
            }
            Some(Ok(_)) => continue,
            _ => return None,
        }

        if let Some(pos) = buf.iter().position(|b| *b == hub::RECORD_SEPARATOR) {
            break pos + 1;
        }
    };

    let rest = buf.split_off(end);
    Some((buf, binary.unwrap_or(true), rest))
}

/// Answers the handshake as `options` say; `false` when the session ends here.
async fn answer_handshake(options: &Options, tx: &mut HubTx) -> bool {
    match &options.handshake {
        HandshakeReply::Ok => {
            let mut frames = Vec::new();
            for message in &options.coalesce_after_handshake {
                frames.extend(hub::encode_frame(message));
            }

            if options.handshake_as_text {
                if tx.send(Message::Text("{}\u{1e}".into())).await.is_err() {
                    return false;
                }
                if !frames.is_empty() && tx.send(Message::Binary(frames.into())).await.is_err() {
                    return false;
                }
            } else {
                let mut bytes = b"{}\x1e".to_vec();
                bytes.extend(frames);
                if tx.send(Message::Binary(bytes.into())).await.is_err() {
                    return false;
                }
            }
            true
        }
        HandshakeReply::Error(error) => {
            let mut bytes = serde_json::to_vec(&serde_json::json!({ "error": error })).unwrap();
            bytes.push(hub::RECORD_SEPARATOR);
            let _ = tx.send(Message::Binary(bytes.into())).await;
            let _ = tx.send(Message::Close(None)).await;
            false
        }
        HandshakeReply::Silent => true,
    }
}

/// After the handshake: sends what the test queues, reads the client's frames while reading is on,
/// and pings at the configured interval, until the session is killed or either side closes.
async fn serve(
    shared: &Arc<HubShared>,
    session: &Arc<HubSession>,
    options: &Options,
    (mut tx, mut rx): (HubTx, HubRx),
    mut out: mpsc::UnboundedReceiver<Outgoing>,
    mut client: Vec<u8>,
) {
    let mut ping = options.ping_interval.map(|d| {
        let mut interval = tokio::time::interval(d);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval
    });

    let mut reading = session.reading.subscribe();
    loop {
        let can_read = *reading.borrow_and_update();
        tokio::select! {
            _ = session.kill.cancelled() => return,
            _ = reading.changed() => {}
            outgoing = out.recv() => {
                if !send_outgoing(&mut tx, outgoing).await {
                    return;
                }
            }
            message = rx.next(), if can_read => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    client.extend_from_slice(&bytes);
                    process_client_frames(shared, session, &mut client);
                }
                Some(Ok(Message::Text(text))) => {
                    client.extend_from_slice(text.as_str().as_bytes());
                    process_client_frames(shared, session, &mut client);
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                Some(Ok(_)) => {}
            },
            _ = tick(&mut ping) => {
                if tx.send(Message::Binary(hub::encode_frame(&HubMessage::Ping).into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Sends one message the test queued; `false` when the session ends here.
async fn send_outgoing(tx: &mut HubTx, outgoing: Option<Outgoing>) -> bool {
    let result = match outgoing {
        Some(Outgoing::Binary(bytes)) => tx.send(Message::Binary(bytes.into())).await,
        Some(Outgoing::Text(text)) => tx.send(Message::Text(text.into())).await,
        Some(Outgoing::CloseFrame) => {
            let _ = tx.send(Message::Close(None)).await;
            return false;
        }
        None => return false,
    };
    result.is_ok()
}

fn process_client_frames(shared: &Arc<HubShared>, session: &Arc<HubSession>, buf: &mut Vec<u8>) {
    let frames = match hub::drain_frames(buf) {
        Ok(frames) => frames,
        Err(error) => {
            session.errors.lock().unwrap().push(format!("framing: {error}"));
            buf.clear();
            return;
        }
    };

    for payload in frames {
        let message = match hub::decode_message(&payload) {
            Ok(message) => message,
            Err(error) => {
                session.errors.lock().unwrap().push(format!("decode: {error}"));
                continue;
            }
        };

        session.messages.lock().unwrap().push(message.clone());
        match message {
            HubMessage::Ping => session.pings.lock().unwrap().push(Instant::now()),
            HubMessage::Invocation { invocation_id, target, arguments, .. } => {
                let invocation = RecordedInvocation {
                    at: Instant::now(),
                    session: session.index,
                    invocation_id: invocation_id.clone(),
                    target: target.clone(),
                    arguments,
                    payload: payload.clone(),
                };
                session.invocations.lock().unwrap().push(invocation.clone());

                if let Some(id) = invocation_id {
                    let result = match shared.completion_for(&target) {
                        CompletionMode::Void => Some(CompletionResult::Void),
                        CompletionMode::Error(text) => Some(CompletionResult::Error(text)),
                        CompletionMode::Never => None,
                    };
                    if let Some(result) = result {
                        session.send(HubMessage::Completion { headers: Vec::new(), invocation_id: id, result });
                    }
                }

                let hooks = shared.hooks.lock().unwrap().clone();
                for hook in hooks {
                    hook(session, &invocation);
                }
            }
            _ => {}
        }
        session.notify();
    }
}

// ------------------------------------------------------------------------------------------------
// Hub → client message builders (the inbound hub methods)
// ------------------------------------------------------------------------------------------------

/// A hub → client invocation without an invocation id: `[1, {}, nil, target, args, []]`.
pub fn invocation(target: &str, arguments: Vec<Value>) -> HubMessage {
    HubMessage::Invocation {
        headers: Vec::new(),
        invocation_id: None,
        target: target.to_string(),
        arguments,
        stream_ids: Vec::new(),
    }
}

/// A valid `HubRelayTraceContextPacket` value with a fixed trace id.
pub fn trace_value() -> Value {
    let ctx = TraceContext {
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
        parent_id: None,
        correlation_id: "00f067aa0ba902b7".into(),
        flags: 0,
        trace_state: vec![("ypScenarioId".into(), "PhoneScenario".into())],
    };
    TraceContextPacket::from_context(&ctx).to_value()
}

pub fn on_connected(region: Option<&str>, partners: &[&str]) -> HubMessage {
    let payload = OnConnectedPayload {
        region_name: region.map(str::to_string),
        partners: Some(partners.iter().map(|p| p.to_string()).collect()),
    };
    invocation(method::ON_CONNECTED, vec![payload.to_value()])
}

pub fn on_partner_connected(source: &str, region: Value) -> HubMessage {
    invocation(method::ON_PARTNER_CONNECTED, vec![Value::from(source), trace_value(), region])
}

pub fn on_partner_disconnected(source: &str) -> HubMessage {
    invocation(method::ON_PARTNER_DISCONNECTED, vec![Value::from(source)])
}

pub fn on_partner_heartbeat(suffix: &str, connection_id: &str) -> HubMessage {
    invocation(
        method::ON_PARTNER_HEARTBEAT,
        vec![Value::from(suffix), Value::from(connection_id), Value::from("2026-10-20T00:00:00Z")],
    )
}

pub fn on_receive_message(sender: &str, packet: &MultiplexPacket) -> HubMessage {
    invocation(method::ON_RECEIVE_MESSAGE, vec![Value::from(sender), trace_value(), packet.to_value()])
}

pub fn on_receive_session_based_message(
    sender: &str,
    packet: &MultiplexPacket,
    connection_session_id: &str,
) -> HubMessage {
    invocation(
        method::ON_RECEIVE_SESSION_BASED_MESSAGE,
        vec![Value::from(sender), trace_value(), packet.to_value(), Value::from(connection_session_id)],
    )
}

/// `[7, error, allowReconnect]` (`allowReconnect` omitted when `None`).
pub fn close_message(error: Option<&str>, allow_reconnect: Option<bool>) -> HubMessage {
    HubMessage::Close { error: error.map(str::to_string), allow_reconnect }
}

// ------------------------------------------------------------------------------------------------
// Message-level helpers for scripting a peer (`ms-dcg` framing)
// ------------------------------------------------------------------------------------------------

/// A peer's message as the `ms-dcg` fragments it would send: the binary inner envelope of the
/// message cut into `fragment_size`-byte chunks, numbered `FragmentId` 1.., with `SequenceNumber`s
/// from `first_sequence` and the peer's own `session_id`.
pub fn message_fragments(
    message: &TransportMessage,
    session_id: &str,
    message_id: i32,
    first_sequence: i32,
    fragment_size: usize,
) -> Vec<Fragment> {
    let bytes = envelope::encode(&message.headers, &message.payload).expect("a message with headers");
    let parts: Vec<&[u8]> = bytes.chunks(fragment_size).collect();
    let count = i32::try_from(parts.len()).expect("fragment count fits an int32");

    parts
        .iter()
        .enumerate()
        .map(|(k, part)| {
            let k = i32::try_from(k).expect("fragment index fits an int32");
            Fragment {
                version: VERSION,
                session_id: session_id.to_string(),
                sequence_number: first_sequence + k,
                message_id,
                message_type_raw: Some(i32::try_from(message.message_type.wire_value()).unwrap()),
                fragment_id: k + 1,
                fragment_count: count,
                raw: part.to_vec(),
            }
        })
        .collect()
}

/// Reassembles fragments (for example [`FakeHub::peer_fragments`]) into messages, one per
/// `(SessionId, MessageId)` in first-seen order. Panics on an incomplete or undecodable message.
pub fn reassemble(fragments: &[Fragment]) -> Vec<TransportMessage> {
    let mut order: Vec<(String, i32)> = Vec::new();
    let mut groups: HashMap<(String, i32), Vec<&Fragment>> = HashMap::new();
    for fragment in fragments {
        let key = (fragment.session_id.clone(), fragment.message_id);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(fragment);
    }

    order
        .into_iter()
        .map(|key| {
            let mut parts = groups.remove(&key).unwrap();
            parts.sort_by_key(|f| f.fragment_id);
            parts.dedup_by_key(|f| f.fragment_id);

            let count = parts[0].fragment_count;
            assert_eq!(parts.len() as i32, count, "message {key:?} is incomplete");

            let bytes: Vec<u8> = parts.iter().flat_map(|f| f.raw.iter().copied()).collect();
            let decoded = envelope::decode(&bytes).expect("a decodable envelope");
            TransportMessage::new(parts[0].message_type(), decoded.headers, decoded.payload)
        })
        .collect()
}
