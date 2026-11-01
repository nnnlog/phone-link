//! Partner presence and the inbound hub methods: hand-off to the DCG layer, the partner set and the
//! `SendConnectedAsync` exchange, the `OnConnected` payload, the heartbeat window, partner disconnect
//! reasons and reconnection gating, and unknown inbound traffic.
//!
//! One [`Presence`] per environment holds the partner set of that environment's connection, one
//! heartbeat window per partner, and the last `OnConnected` payload. Mutations coming from a
//! session's reader carry that session's id and are ignored once the session was deactivated, so a
//! late frame never repopulates a set the `Reconnecting` / `Closed` reaction just cleared.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use rmpv::Value;
use tokio::sync::watch;
use tokio::task::AbortHandle;

use super::connection::{HubConnection, InvokeError};
use super::{ConnectionKey, RelayShared};
use phonelink_core::codec::hub::{
    self, HubCodecError, MultiplexPacket, OnConnectedPayload, StringValueError, TraceContextPacket, method,
};
use phonelink_core::seams::{HubRegionTimeout, RelayEvent};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, Environment, PartnerDisconnectReason, TraceContext};

struct PartnerEntry {
    /// Consecutive expired windows since the last arming.
    misses: u32,
    /// Identifies the arming the running window belongs to.
    generation: u64,
    window: Option<AbortHandle>,
}

impl Drop for PartnerEntry {
    fn drop(&mut self) {
        if let Some(window) = self.window.take() {
            window.abort();
        }
    }
}

/// Inbound conditions that recur with every message while they last: each is logged at `warn` the
/// first time in a session and at `debug` after, so a phone or hub that keeps causing one does not
/// flood the log.
#[derive(Default)]
struct Noted {
    unmatched_heartbeat: bool,
    nil_session_id: bool,
    no_packet_sink: bool,
}

#[derive(Default)]
struct PresenceInner {
    /// The session whose reader may change the set; `None` while not connected.
    active_session: Option<u64>,
    /// Reset when a session becomes active.
    noted: Noted,
    /// Ordered by id so the suffix match of `OnPartnerHeartBeat` is deterministic.
    partners: BTreeMap<String, PartnerEntry>,
    next_generation: u64,
}

/// The partner set, heartbeat windows and `OnConnected` payload of one environment.
pub(crate) struct Presence {
    env: Environment,
    shared: Arc<RelayShared>,
    me: Weak<Presence>,
    inner: Mutex<PresenceInner>,
    /// Bumped on every change of the partner set (waiters of `wait_partner`).
    version: watch::Sender<u64>,
    on_connected: watch::Sender<Option<OnConnectedPayload>>,
}

impl Presence {
    pub(super) fn new(env: Environment, shared: Arc<RelayShared>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            env,
            shared,
            me: me.clone(),
            inner: Mutex::new(PresenceInner::default()),
            version: watch::channel(0).0,
            on_connected: watch::channel(None).0,
        })
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, PresenceInner> {
        self.inner.lock_or_panic()
    }

    fn client(&self, id: &str) -> DcgClient {
        DcgClient::new(id, self.env)
    }

    fn bump(&self) {
        self.version.send_modify(|v| *v += 1);
    }

    /// `HeartBeatFrequency + grace`; `None` when the frequency is `0` (no windows run).
    fn window(&self) -> Option<Duration> {
        let cfg = &self.shared.config.relay;
        if cfg.heartbeat_frequency.is_zero() { None } else { Some(cfg.heartbeat_frequency + cfg.heartbeat_grace) }
    }

    pub(super) fn activate(&self, session_id: u64) {
        let mut inner = self.inner();
        inner.active_session = Some(session_id);
        inner.noted = Noted::default();
    }

    /// `true` the first time in the active session that `pick` selects its flag (which it then sets).
    fn first_in_session(&self, pick: impl FnOnce(&mut Noted) -> &mut bool) -> bool {
        !std::mem::replace(pick(&mut self.inner().noted), true)
    }

    /// The one line of a partner's removal: `warn` when the engine removed it for want of answers,
    /// `info` when the hub, the PC or the connection did.
    fn log_removed(&self, id: &str, reason: PartnerDisconnectReason) {
        match reason {
            PartnerDisconnectReason::TooManyAckTimeouts | PartnerDisconnectReason::ExceededMissedHeartbeats => {
                tracing::warn!(env = %self.env, partner = id, disconnect_kind = ?reason, "partner removed");
            }
            PartnerDisconnectReason::NoticeFromHub
            | PartnerDisconnectReason::LocalHubDisconnect
            | PartnerDisconnectReason::SystemReset => {
                tracing::info!(env = %self.env, partner = id, disconnect_kind = ?reason, "partner removed");
            }
        }
    }

    pub(super) fn is_active(&self, session_id: u64) -> bool {
        self.inner().active_session == Some(session_id)
    }

    /// The `Reconnecting` / `Closed` reaction: no session may change the set any more; every
    /// partner is removed with `reason`; `forget_payload` also forgets the `OnConnected` payload. The
    /// hub connection's own line tells why; the removals are its consequence.
    pub(super) fn deactivate(&self, reason: PartnerDisconnectReason, forget_payload: bool) {
        let mut inner = self.inner();
        inner.active_session = None;

        let removed = std::mem::take(&mut inner.partners);
        let any = !removed.is_empty();
        if any {
            tracing::debug!(env = %self.env, partner_count = removed.len(), disconnect_kind = ?reason, "partners removed");
        }

        for (id, entry) in removed {
            drop(entry);
            self.shared.emit(RelayEvent::PartnerDisconnected { partner: self.client(&id), reason });
        }

        if forget_payload {
            self.on_connected.send_replace(None);
        }
        if any {
            self.bump();
        }
    }

    /// Re-arms `id`'s window and resets its miss counter (`id` must be in the set).
    fn arm(&self, inner: &mut PresenceInner, id: &str) {
        let window = self.window();
        inner.next_generation += 1;
        let generation = inner.next_generation;

        let entry = inner.partners.get_mut(id).expect("arm() is only called for a partner in the set");
        entry.misses = 0;
        entry.generation = generation;

        if let Some(old) = entry.window.take() {
            old.abort();
        }
        if let Some(window) = window {
            let task = tokio::spawn(heartbeat_window(self.me.clone(), id.to_string(), generation, window));
            entry.window = Some(task.abort_handle());
        }
    }

    /// Adds `id` to the set (re-arming its window when `arm`); `Some(entered)` tells whether it was
    /// new, `None` that the session is no longer active and nothing changed.
    pub(super) fn add(&self, session_id: u64, id: &str, arm: bool) -> Option<bool> {
        let mut inner = self.inner();
        if inner.active_session != Some(session_id) {
            return None;
        }

        let entered = !inner.partners.contains_key(id);
        if entered {
            inner.partners.insert(id.to_string(), PartnerEntry { misses: 0, generation: 0, window: None });
        }

        if arm {
            self.arm(&mut inner, id);
        }

        if entered {
            tracing::info!(env = %self.env, partner = id, "partner connected");
            self.shared.emit(RelayEvent::PartnerConnected(self.client(id)));
            self.bump();
        }
        Some(entered)
    }

    /// Removes `id` with `reason`; with `Some(session_id)` only while that session is active.
    pub(super) fn remove(&self, session_id: Option<u64>, id: &str, reason: PartnerDisconnectReason) -> bool {
        let mut inner = self.inner();
        if let Some(session_id) = session_id {
            if inner.active_session != Some(session_id) {
                return false;
            }
        }

        if inner.partners.remove(id).is_none() {
            return false;
        }

        self.log_removed(id, reason);
        self.shared.emit(RelayEvent::PartnerDisconnected { partner: self.client(id), reason });
        self.bump();
        true
    }

    /// `OnPartnerHeartBeat(suffix)`: the partner whose id ends with `suffix` (ordinal) is re-armed;
    /// an unmatched suffix is logged and dropped; with frequency `0` heartbeats are ignored.
    pub(super) fn heartbeat(&self, session_id: u64, suffix: &str, connection_id: Option<&str>) {
        let mut inner = self.inner();
        if inner.active_session != Some(session_id) {
            return;
        }
        if self.window().is_none() {
            tracing::debug!(env = %self.env, suffix, "heartbeat ignored; HeartBeatFrequency is 0");
            return;
        }

        let matched = inner.partners.keys().find(|id| id.ends_with(suffix)).cloned();
        match matched {
            Some(id) => {
                tracing::trace!(env = %self.env, partner = id, connection_id, "partner heartbeat");
                self.arm(&mut inner, &id);
            }
            None => {
                let first = !std::mem::replace(&mut inner.noted.unmatched_heartbeat, true);
                self.log_unmatched_heartbeat(first, suffix, connection_id);
            }
        }
    }

    /// An `OnPartnerHeartBeat` whose suffix matches no partner: it repeats with every heartbeat
    /// while it lasts, so only the session's `first` one is a `warn`.
    fn log_unmatched_heartbeat(&self, first: bool, suffix: &str, connection_id: Option<&str>) {
        if first {
            tracing::warn!(env = %self.env, suffix, connection_id, "heartbeat for no connected partner dropped");
        } else {
            tracing::debug!(env = %self.env, suffix, connection_id, "heartbeat for no connected partner dropped");
        }
    }

    /// Stores the `OnConnected` payload (resolved region) and wakes its waiters.
    pub(super) fn set_on_connected(&self, session_id: u64, payload: OnConnectedPayload) -> bool {
        let inner = self.inner();
        if inner.active_session != Some(session_id) {
            return false;
        }

        let region = payload.region_name.clone();
        self.on_connected.send_replace(Some(payload));
        self.shared.emit(RelayEvent::OnConnected { env: self.env, region });
        drop(inner);
        true
    }

    /// One expired window of `id` (arming `generation`); `true` while the window keeps running.
    fn window_expired(&self, id: &str, generation: u64) -> bool {
        let threshold = self.shared.config.relay.missed_heartbeats_threshold;
        let mut inner = self.inner();
        let Some(entry) = inner.partners.get_mut(id) else { return false };
        if entry.generation != generation {
            return false;
        }

        entry.misses += 1;
        let misses = entry.misses;
        if misses < threshold {
            tracing::debug!(env = %self.env, partner = id, misses, "missed heartbeat window");
            return true;
        }

        // This task is the window: let it end by itself instead of aborting it.
        entry.window = None;
        inner.partners.remove(id);
        self.log_removed(id, PartnerDisconnectReason::ExceededMissedHeartbeats);
        self.shared.emit(RelayEvent::PartnerDisconnected {
            partner: self.client(id),
            reason: PartnerDisconnectReason::ExceededMissedHeartbeats,
        });
        drop(inner);
        self.bump();
        false
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.inner().partners.contains_key(id)
    }

    pub(super) fn partners(&self) -> Vec<String> {
        self.inner().partners.keys().cloned().collect()
    }

    pub(super) fn on_connected(&self) -> Option<OnConnectedPayload> {
        self.on_connected.borrow().clone()
    }

    /// Waits up to `timeout` for the `OnConnected` payload.
    pub(super) async fn wait_on_connected(&self, timeout: Duration) -> Result<OnConnectedPayload, HubRegionTimeout> {
        let mut rx = self.on_connected.subscribe();
        match tokio::time::timeout(timeout, rx.wait_for(Option::is_some)).await {
            Ok(Ok(payload)) => Ok(payload.clone().expect("wait_for returned a payload")),
            _ => Err(HubRegionTimeout),
        }
    }

    /// Waits up to `timeout` for `id` to be in the set.
    pub(super) async fn wait_partner(&self, id: &str, timeout: Duration) -> bool {
        let mut rx = self.version.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            if self.contains(id) {
                return true;
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => continue,
                _ => return self.contains(id),
            }
        }
    }
}

/// The heartbeat window of one arming: after each `window` without a re-arm the miss counter grows;
/// at `missed_heartbeats_threshold` consecutive misses the partner is removed with
/// `ExceededMissedHeartbeats`. The engine's choice: the window repeats until the threshold instead
/// of firing once.
async fn heartbeat_window(presence: Weak<Presence>, id: String, generation: u64, window: Duration) {
    loop {
        tokio::time::sleep(window).await;
        let Some(presence) = presence.upgrade() else { return };
        if !presence.window_expired(&id, generation) {
            return;
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Inbound hub methods
// ------------------------------------------------------------------------------------------------

/// Why an inbound hub invocation could not be bound to its handler. It is logged and dropped; the
/// connection stays up.
#[derive(Debug, thiserror::Error)]
enum BindingError {
    #[error("expected {expected} arguments, got {got}")]
    Arity { expected: usize, got: usize },
    #[error(transparent)]
    Value(#[from] StringValueError),
    #[error("{0} is nil or empty")]
    MissingId(&'static str),
    #[error("no handler is registered for hub method {0:?}")]
    UnknownMethod(String),
    #[error(transparent)]
    Codec(#[from] HubCodecError),
    #[error("{0}: the packet is dropped and the presence is untouched")]
    PacketTraceRejected(HubCodecError),
    #[error("a nil trace context: the sender is marked connected, the packet is dropped")]
    NilPacketTrace,
    #[error("{0}: the reply was sent, the partner is not added")]
    PartnerTraceRejected(HubCodecError),
}

fn arity(arguments: &[Value], expected: usize) -> Result<(), BindingError> {
    if arguments.len() == expected { Ok(()) } else { Err(BindingError::Arity { expected, got: arguments.len() }) }
}

/// A partner id the method cannot do without: a non-empty string.
fn bind_id(value: &Value, what: &'static str) -> Result<String, BindingError> {
    match hub::opt_string(value, what)? {
        Some(id) if !id.is_empty() => Ok(id),
        _ => Err(BindingError::MissingId(what)),
    }
}

/// Handles one inbound invocation on the reader of session `session_id`. Wrong argument counts or
/// types are binding failures and an unregistered method is ignored; both are logged and dropped,
/// the connection stays up.
pub(crate) fn dispatch(
    conn: &Arc<HubConnection>,
    session_id: u64,
    target: &str,
    arguments: Vec<Value>,
    invocation_id: Option<String>,
) {
    let env = conn.env();
    if let Some(id) = &invocation_id {
        tracing::debug!(env = %env, raw_target = target, id, "inbound invocation carries an id; hub methods return nothing");
    }

    let result = match target {
        method::ON_CONNECTED => on_connected(conn, session_id, &arguments),
        method::ON_RECEIVE_MESSAGE => on_receive_message(conn, session_id, &arguments, false),
        method::ON_RECEIVE_SESSION_BASED_MESSAGE => on_receive_message(conn, session_id, &arguments, true),
        method::ON_PARTNER_CONNECTED => on_partner_connected(conn, session_id, &arguments),
        method::ON_PARTNER_DISCONNECTED => on_partner_disconnected(conn, session_id, &arguments),
        method::ON_PARTNER_HEARTBEAT => on_partner_heartbeat(conn, session_id, &arguments),
        other => Err(BindingError::UnknownMethod(other.to_string())),
    };

    if let Err(error) = result {
        let bytes = hub::to_bytes(&Value::Array(arguments));
        conn.shared().undecodable(env, &format!("{target} invocation"), &error.to_string(), Some(&bytes));
    }
}

fn on_connected(conn: &Arc<HubConnection>, session_id: u64, arguments: &[Value]) -> Result<(), BindingError> {
    arity(arguments, 1)?;
    let payload = OnConnectedPayload::from_value(&arguments[0])?;

    tracing::info!(
        env = %conn.env(),
        region = payload.region_name.as_deref(),
        partner_count = payload.partners.as_ref().map_or(0, Vec::len),
        "OnConnected received; its Partners list is informational only"
    );
    conn.presence().set_on_connected(session_id, payload);
    Ok(())
}

/// `OnReceiveMessage(sender, ctx, data)` / `OnReceiveSessionBasedMessage(sender, ctx, data,
/// connectionSessionId)`: mark the sender connected (re-arming its window only for the
/// session-less method), auto-assign an unknown sender to this connection, and hand the packet to
/// the DCG layer in arrival order.
///
/// The trace context (a map of 4) decides first: one the receiver
/// rejects drops the packet and leaves the presence untouched — no partner added, no window
/// re-armed, no auto-assignment; a nil one still marks the sender connected, but the packet is
/// dropped.
fn on_receive_message(
    conn: &Arc<HubConnection>,
    session_id: u64,
    arguments: &[Value],
    session_based: bool,
) -> Result<(), BindingError> {
    arity(arguments, if session_based { 4 } else { 3 })?;
    let sender = bind_id(&arguments[0], "sender")?;
    let ctx = TraceContextPacket::from_value(&arguments[1])?;
    let packet = MultiplexPacket::from_value(&arguments[2])?;

    let trace = match ctx.as_ref().map(TraceContextPacket::to_inbound_context) {
        Some(Ok(trace)) => Some(trace),
        Some(Err(e)) => return Err(BindingError::PacketTraceRejected(e)),
        None => None,
    };
    let connection_session_id = if session_based { session_based_id(conn, &arguments[3], &sender)? } else { None };

    let env = conn.env();
    let Some(entered) = conn.presence().add(session_id, &sender, !session_based) else {
        tracing::debug!(env = %env, sender, "packet from a replaced session dropped");
        return Ok(());
    };

    let Some(trace) = trace else {
        return Err(BindingError::NilPacketTrace);
    };
    deliver_packet(conn, sender, trace, packet, connection_session_id, entered);
    Ok(())
}

/// The `connectionSessionId` of `OnReceiveSessionBasedMessage` (a nil one is logged).
fn session_based_id(conn: &HubConnection, value: &Value, sender: &str) -> Result<Option<String>, BindingError> {
    let id = hub::opt_string(value, "connectionSessionId")?;

    if id.is_none() {
        if conn.presence().first_in_session(|n| &mut n.nil_session_id) {
            tracing::warn!(env = %conn.env(), sender, "OnReceiveSessionBasedMessage with a nil connectionSessionId");
        } else {
            tracing::debug!(env = %conn.env(), sender, "OnReceiveSessionBasedMessage with a nil connectionSessionId");
        }
    }
    Ok(id)
}

/// Hands an accepted inbound packet to the DCG layer, first reporting a sender that contacted the
/// PC before any partner connection existed for it.
fn deliver_packet(
    conn: &HubConnection,
    sender: String,
    trace: TraceContext,
    packet: MultiplexPacket,
    connection_session_id: Option<String>,
    entered: bool,
) {
    let env = conn.env();
    let shared = conn.shared();
    let key = ConnectionKey { partner: sender.clone(), env, connection_session_id: connection_session_id.clone() };
    let new_key = shared.keys.lock_or_panic().insert(key);

    if new_key && connection_session_id.is_none() && !entered {
        // The phone contacted the PC first: report it so a partner connection is created for it.
        tracing::info!(env = %env, sender, "inbound sender auto-assigned to this connection");
        shared.emit(RelayEvent::PartnerConnected(DcgClient::new(sender.clone(), env)));
    }

    match shared.packet_sink() {
        Some(sink) => sink.on_packet(DcgClient::new(sender, env), trace, packet, connection_session_id),
        None => {
            if conn.presence().first_in_session(|n| &mut n.no_packet_sink) {
                tracing::error!(env = %env, sender, "inbound packet dropped; no packet sink is set");
            } else {
                tracing::debug!(env = %env, sender, "inbound packet dropped; no packet sink is set");
            }
        }
    }
}

/// `OnPartnerConnected(sourceId, ctx, region)`: reply `SendConnectedAsync(sourceId)` at once, then
/// add the partner and re-arm its window. `region` may be any `MessagePack` value (string, map, nil)
/// and is only logged. A trace context the receiver rejects still gets the reply, but the partner
/// is neither added nor re-armed (a nil one is fine).
fn on_partner_connected(conn: &Arc<HubConnection>, session_id: u64, arguments: &[Value]) -> Result<(), BindingError> {
    arity(arguments, 3)?;
    let source = bind_id(&arguments[0], "sourceId")?;
    let ctx = TraceContextPacket::from_value(&arguments[1])?;

    let env = conn.env();
    let region = match &arguments[2] {
        Value::String(s) => s.as_str().unwrap_or("null or not specified").to_string(),
        Value::Nil => "null or not specified".to_string(),
        other => hub::value_to_json(other).to_string(),
    };

    if !conn.presence().is_active(session_id) {
        tracing::debug!(env = %env, source, "OnPartnerConnected from a replaced session dropped");
        return Ok(());
    }

    tracing::debug!(env = %env, partner = source, raw_region = region, "OnPartnerConnected received");
    match conn.start_send_connected(&source) {
        Ok(completion) => {
            let partner = source.clone();
            tokio::spawn(async move {
                // A reply lost with its session is the session's end, which the hub connection logs.
                match completion.await {
                    Ok(Ok(())) => tracing::debug!(env = %env, partner, "SendConnectedAsync completed"),
                    Ok(Err(InvokeError::ConnectionLost)) | Err(_) => {
                        tracing::debug!(env = %env, partner, "SendConnectedAsync reply lost with its session");
                    }
                    Ok(Err(error)) => tracing::warn!(env = %env, partner, ?error, "SendConnectedAsync reply failed"),
                }
            });
        }
        // The session is ending (the hub connection logs that): the reply cannot be queued.
        Err(error) => tracing::debug!(env = %env, partner = source, ?error, "SendConnectedAsync reply not sent"),
    }

    if let Some(Err(e)) = ctx.as_ref().map(TraceContextPacket::to_inbound_context) {
        return Err(BindingError::PartnerTraceRejected(e));
    }
    conn.presence().add(session_id, &source, true);
    Ok(())
}

fn on_partner_disconnected(
    conn: &Arc<HubConnection>,
    session_id: u64,
    arguments: &[Value],
) -> Result<(), BindingError> {
    arity(arguments, 1)?;
    let source = bind_id(&arguments[0], "sourceId")?;
    if !conn.presence().remove(Some(session_id), &source, PartnerDisconnectReason::NoticeFromHub) {
        tracing::debug!(env = %conn.env(), source, "OnPartnerDisconnected for a partner not in the set");
    }
    Ok(())
}

fn on_partner_heartbeat(conn: &Arc<HubConnection>, session_id: u64, arguments: &[Value]) -> Result<(), BindingError> {
    arity(arguments, 3)?;
    let suffix = bind_id(&arguments[0], "sourceId")?;
    let connection_id = hub::opt_string(&arguments[1], "connectionId")?;
    // `sentTime` (arguments[2]) is accepted whatever its shape and never read.
    conn.presence().heartbeat(session_id, &suffix, connection_id.as_deref());
    Ok(())
}
