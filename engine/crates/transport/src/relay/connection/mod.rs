//! One hub connection: the endpoint and HTTP layer, connection headers, the `messagepack` version 2
//! hub protocol, connection states and the open sequence, the start retry policy (HTTP 401 and 429),
//! the `OpenStatus` mapping, automatic reconnect and send results.
//!
//! Life cycle. `open` is serialized per connection and runs in a task of its own, so a caller that
//! stops waiting (its future dropped by a `select!` or a timeout) cancels the open instead of
//! leaving the connection `Connecting`: the task abandons the start and reports `Disconnected`
//! before the next `open` may run. A start is negotiate → WebSocket upgrade → handshake; on success
//! a *session* (one WebSocket) is installed and run by two tasks: the reader reads and dispatches
//! hub frames and ends the session when no byte was received for the server timeout; the writer
//! writes queued invocation frames and a client `Ping` when nothing was sent for the keepalive
//! interval. Reading never waits for a write, and "received" is measured on the transport's bytes
//! (beneath TLS and WebSocket framing), so neither a long write nor a long inbound message trips
//! the server timeout while bytes keep arriving. An unexpected end (socket loss, server timeout,
//! server `Close` with `allowReconnect`) moves the connection to `Reconnecting` and runs the
//! reconnect schedule; a server `Close` without `allowReconnect`, an exhausted schedule or a local
//! `close` moves it to `Disconnected`.
//!
//! Every state transition and its reaction (partner removal, region reset, the `HubState` event)
//! runs under one async `transition` lock, so events are reported in the order the states were
//! entered. Results of in-flight work (an `open` start, a reconnect attempt) are discarded when the
//! connection's `epoch` moved on meanwhile.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use rmpv::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::RelayShared;
use super::presence::Presence;
use phonelink_core::codec::hub::{self, HubMessage};
use phonelink_core::config::RelayConfig;
use phonelink_core::constants::http_header as h;
use phonelink_core::seams::RelayEvent;
use phonelink_core::sync::MutexExt;
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use phonelink_core::types::{Environment, HubState, TraceContext};

mod activity;
mod session_task;
mod start_error;

use activity::{ActivityStream, ReceiveClock, connect_tcp};
use session_task::run_session;
pub(crate) use start_error::{Established, StartError};

mod inbound;
mod invocations;
mod open_close;
mod starting;

type WsStream = WebSocketStream<MaybeTlsStream<ActivityStream<TcpStream>>>;

/// Bound on the handshake answer. Assumption (the protocol names none): the `SignalR` client's
/// `HandshakeTimeout` default.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// The library defaults of `KeepAliveInterval` and `ServerTimeout`, which apply when the configured
/// server timeout is not above the keepalive.
const LIBRARY_KEEP_ALIVE: Duration = Duration::from_secs(15);
const LIBRARY_SERVER_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on sending the WebSocket close frame of a locally stopped session.
const CLOSE_FRAME_TIMEOUT: Duration = Duration::from_secs(1);

// ------------------------------------------------------------------------------------------------
// Sessions and invocations
// ------------------------------------------------------------------------------------------------

/// Why an invocation did not complete successfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InvokeError {
    /// The pre-check: the connection is not `Connected`.
    NotConnected,
    /// The session ended before the completion arrived.
    ConnectionLost,
    /// An error completion (kind 1) with the hub's text, or a completion that failed to decode.
    Failed(String),
}

pub(crate) type CompletionReceiver = oneshot::Receiver<Result<(), InvokeError>>;

#[derive(Default)]
struct Pending {
    closed: bool,
    next_id: u64,
    waiters: HashMap<String, oneshot::Sender<Result<(), InvokeError>>>,
}

/// One WebSocket of the connection. Invocation ids are decimal strings incrementing from `"1"` per
/// session.
pub(crate) struct Session {
    pub id: u64,
    outbound: mpsc::UnboundedSender<Vec<u8>>,
    pending: Mutex<Pending>,
    cancel: CancellationToken,
}

impl Session {
    fn new(id: u64, outbound: mpsc::UnboundedSender<Vec<u8>>, cancel: CancellationToken) -> Self {
        Self { id, outbound, pending: Mutex::new(Pending::default()), cancel }
    }

    /// Registers a completion waiter and queues the invocation frame `[1, {}, id, target, args, []]`.
    fn invoke(
        &self,
        shared: &RelayShared,
        env: Environment,
        target: &str,
        arguments: Vec<Value>,
    ) -> Result<CompletionReceiver, InvokeError> {
        let mut pending = self.pending.lock_or_panic();
        if pending.closed {
            return Err(InvokeError::ConnectionLost);
        }

        pending.next_id += 1;
        let id = pending.next_id.to_string();
        let summary_args: Vec<serde_json::Value> = arguments.iter().map(hub::value_to_json).collect();
        let frame = hub::encode_frame(&HubMessage::invocation(id.clone(), target, arguments));
        let (tx, rx) = oneshot::channel();
        pending.waiters.insert(id.clone(), tx);

        shared.tap.emit(TapEvent::new(
            TapLayer::HubMessage,
            Direction::Out,
            Some(env.name().to_string()),
            serde_json::json!({ "type": 1, "invocationId": id, "target": target, "arguments": summary_args }),
            Some(&frame),
        ));

        if self.outbound.send(frame).is_err() {
            pending.waiters.remove(&id);
            return Err(InvokeError::ConnectionLost);
        }
        Ok(rx)
    }

    /// Completes the waiter of `id`; `false` when no invocation with that id is pending.
    fn complete(&self, id: &str, outcome: Result<(), InvokeError>) -> bool {
        let waiter = self.pending.lock_or_panic().waiters.remove(id);
        match waiter {
            Some(tx) => {
                let _ = tx.send(outcome);
                true
            }
            None => false,
        }
    }

    /// Fails every pending invocation and refuses new ones: the session is gone.
    fn close_pending(&self) {
        let waiters = {
            let mut pending = self.pending.lock_or_panic();
            pending.closed = true;
            std::mem::take(&mut pending.waiters)
        };

        for (_, tx) in waiters {
            let _ = tx.send(Err(InvokeError::ConnectionLost));
        }
    }
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SessionEnd {
    /// Stopped locally (`close`): nothing more to do.
    Stopped,
    /// Socket loss, read/write failure, or the server timeout.
    Lost(String),
    /// A hub `Close` message.
    ServerClose { error: Option<String>, allow_reconnect: Option<bool> },
}

impl SessionEnd {
    /// Unexpected drops reconnect; a server `Close` only with `allowReconnect = true`.
    fn reconnects(&self) -> bool {
        match self {
            SessionEnd::Stopped => false,
            SessionEnd::Lost(_) => true,
            SessionEnd::ServerClose { allow_reconnect, .. } => *allow_reconnect == Some(true),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The connection
// ------------------------------------------------------------------------------------------------

/// What a connection is created with: the inputs of its connection headers.
#[derive(Debug, Clone)]
pub(crate) struct ConnectionParams {
    pub env: Environment,
    /// `DCG-PartnerId`: the first partner the connection was created for; partners that later
    /// share the connection do not change it.
    pub partner_id: Option<String>,
    /// The trace context of the request that caused the connection to be created; its
    /// `traceparent` / `tracestate` / `DCG-TraceState` go on every start of the connection.
    pub trace: TraceContext,
}

struct Inner {
    state: HubState,
    /// Bumped by every transition that invalidates in-flight start or reconnect work.
    epoch: u64,
    session: Option<Arc<Session>>,
    reconnect: Option<CancellationToken>,
    /// Cancelled (and replaced) by `close`: stops in-flight starts, reconnects and sessions.
    stop: CancellationToken,
    /// The 429 breaker of the start path is open until this instant.
    start_breaker_until: Option<Instant>,
    /// Removed from the connection map: never opened again.
    disposed: bool,
}

pub(crate) struct HubConnection {
    params: ConnectionParams,
    shared: Arc<RelayShared>,
    presence: Arc<Presence>,
    /// How long the start path's 429 breaker stays open: `15 s + rand[0, 30) s`, drawn once when
    /// the connection is created (a reconnect attempt after a 429 draws its own delay,
    /// [`reconnect_delay`]).
    breaker_delay: Duration,
    open_lock: tokio::sync::Mutex<()>,
    transition: tokio::sync::Mutex<()>,
    inner: Mutex<Inner>,
}

/// The HTTP 429 delay: the base delay plus a random whole number of seconds in `[0, variance)`.
fn draw_breaker_delay(cfg: &RelayConfig) -> Duration {
    let variance_secs = cfg.too_many_requests_variance.as_secs();
    let jitter_secs = if variance_secs == 0 { 0 } else { rand::rng().random_range(0..variance_secs) };
    cfg.too_many_requests_base + Duration::from_secs(jitter_secs)
}

/// Automatic reconnect: the delay before the next reconnect attempt after `previous_attempts`
/// attempts (`None`: the schedule is exhausted). After an attempt that failed with HTTP 429 it is
/// `15 s + rand[0, 30) s`, drawn anew for each such attempt, whatever the index.
fn reconnect_delay(cfg: &RelayConfig, previous_attempts: usize, last_was_429: bool) -> Option<Duration> {
    if last_was_429 { Some(draw_breaker_delay(cfg)) } else { cfg.reconnect_delays.get(previous_attempts).copied() }
}

/// The configured keepalive / server timeout, applied only while timeout > keepalive.
fn effective_timeouts(cfg: &RelayConfig) -> (Duration, Duration) {
    if cfg.server_timeout > cfg.keep_alive {
        (cfg.keep_alive, cfg.server_timeout)
    } else {
        (LIBRARY_KEEP_ALIVE, LIBRARY_SERVER_TIMEOUT)
    }
}

/// `DCG-HeartBeatFrequency`: the frequency in seconds as a decimal number (`15` by default).
pub(crate) fn heartbeat_frequency_header(frequency: Duration) -> String {
    format!("{}", frequency.as_secs_f64())
}

fn ws_header(name: &str, value: &str) -> Result<(HeaderName, HeaderValue), StartError> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|e| StartError::Protocol(format!("header name {name:?}: {e}")))?;
    let value = HeaderValue::from_str(value).map_err(|e| StartError::Protocol(format!("header {name} value: {e}")))?;
    Ok((name, value))
}

fn completion_invocation_id(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.get(2)?.as_str().map(str::to_string),
        _ => None,
    }
}

impl HubConnection {
    pub(super) fn new(params: ConnectionParams, shared: Arc<RelayShared>, presence: Arc<Presence>) -> Arc<Self> {
        let breaker_delay = draw_breaker_delay(&shared.config.relay);
        Arc::new(Self {
            params,
            shared,
            presence,
            breaker_delay,
            open_lock: tokio::sync::Mutex::new(()),
            transition: tokio::sync::Mutex::new(()),
            inner: Mutex::new(Inner {
                state: HubState::Disconnected,
                epoch: 0,
                session: None,
                reconnect: None,
                stop: CancellationToken::new(),
                start_breaker_until: None,
                disposed: false,
            }),
        })
    }

    pub(super) fn env(&self) -> Environment {
        self.params.env
    }

    pub(super) fn shared(&self) -> &Arc<RelayShared> {
        &self.shared
    }

    pub(super) fn presence(&self) -> &Arc<Presence> {
        &self.presence
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock_or_panic()
    }

    pub(super) fn state(&self) -> HubState {
        self.inner().state
    }

    /// Reports a state change; the line that logs it is the transition's own.
    fn emit_state(&self, state: HubState) {
        self.shared.emit(RelayEvent::HubState { env: self.params.env, state });
    }

    /// The identification headers, `DCG-PartnerId`, `DCG-HeartBeatFrequency` and the trace headers.
    /// `DCG-HubRegion` is never sent: every connection of the engine is PC-initiated (the
    /// engine does not implement the phone-initiated region). The bearer is added by the caller.
    pub(super) fn connection_headers(&self) -> Vec<(&'static str, String)> {
        let mut headers: Vec<(&'static str, String)> =
            self.shared.ident.headers().iter().map(|(k, v)| (*k, (*v).to_string())).collect();

        if let Some(partner) = &self.params.partner_id {
            headers.push((h::DCG_PARTNER_ID, partner.clone()));
        }
        headers.push((
            h::DCG_HEARTBEAT_FREQUENCY,
            heartbeat_frequency_header(self.shared.config.relay.heartbeat_frequency),
        ));

        let trace = &self.params.trace;
        headers.push((h::TRACEPARENT, trace.traceparent()));
        headers.push((h::TRACESTATE, trace.tracestate()));
        headers.push((h::DCG_TRACE_STATE, trace.tracestate()));
        headers
    }
}

#[cfg(test)]
mod tests;
