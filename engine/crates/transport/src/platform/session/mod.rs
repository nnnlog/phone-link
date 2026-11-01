//! The per-partner connection.
//!
//! It covers connection start policies and session control sequences, with the relay's partner
//! disconnect reasons and reconnection gating, its automatic reconnect (the reactions to the hub
//! connection's events and the per-partner doubling delay), the trace an open carries in its
//! connection headers, and presence; partners are woken through the wake sequence.
//!
//! One state machine per partner [`DcgClient`]. A partner is **connected** while the hub reports it in
//! its partner set and it was not stopped locally; a session the phone ended counts as connected only
//! while the phone is still on the hub and the connection's last failure is not persistent
//! ("externally connected", e.g. disconnect reasons 5 and 6). Connect requests start **attempts** (one
//! at a time per partner): re-announce when the partner is already on the hub, open the hub connection
//! when it is down, otherwise wake the partner. A request waits for a terminal state — connected, the
//! session ended, a local stop, or the attempts stopped — up to its feedback window; a failed attempt
//! is not one.
//!
//! A failed attempt is followed by the next after a retry delay doubled after each failure from
//! `partner_retry_base`; the attempts stop once the doubled delay would reach `partner_retry_fatal`,
//! or at once on a fatal hub open or wake status, keeping the last failure. A retry wait only ever
//! follows a failed attempt: after an **interruption** — the partner removed from the hub for a
//! reason that may reconnect, or its established hub connection closing — the next attempt starts
//! at once (a retry wait in progress ends; behind a running attempt the next one follows at once).
//! The connection keeps the TTL of its attempts' wakes: 20 s, alternating with 45 min after each
//! attempt whose wake ended `DeviceUnreachable`. The delays — the retry delay and that TTL — are reset
//! when an attempt's hub open or wake succeeds, when an attempt is cancelled, when the attempts stop,
//! and when a retry wait runs out with the partner on the hub. They are kept when the partner joins the
//! hub by itself or an attempt finds it there and re-announces it: a retry wait in progress then runs
//! on, and running out with the partner still there it resets the delays and starts no attempt. An
//! attempt that fails after the partner joined by itself waits or stops as any failed attempt, the
//! connection staying connected. An attempt runs on the **extended budget** when, as it starts, the retry delay
//! has grown past its start value or its wake would use the long TTL; a connect request cancels such
//! an attempt and ends a retry wait, and queues behind any other. A wake the fragment layer asks for
//! ([`PartnerConnector`]) uses the default TTL and leaves the connection's TTL alone.
//!
//! While the hub connection of a partner's environment is `Reconnecting` the partner is **paused**
//! (`ConnectingExternally`): its attempts stop — an in-flight wake is cancelled — and none start by
//! themselves, though a connect request still starts one. The pause ends when the partner re-appears
//! on the hub, when the hub is `Connected` again (a partner that was attempting resumes its attempts at
//! once; one that was connected waits to re-appear, since partners re-announce themselves), or when the
//! hub closes (the attempts restart at once).
//!
//! The engine implements the `Default` and `NewSession` start policies only.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::router::Router;
use super::session_control::{is_persistent_disconnect_reason, send_disconnect_request};
use phonelink_core::config::EngineConfig;
use phonelink_core::seams::{
    ConnectOutcome, ConnectRequest, ConnectionFailure, HubControl, PartnerConnector, PartnerSessionControl,
    PlatformRequestStatus, RelayEvent, SessionEvent, StartPolicy, Waker,
};
use phonelink_core::sync::MutexExt;
use phonelink_core::tap::Tap;
use phonelink_core::types::{
    ConnectReason, DcgClient, DcgSendStatus, Environment, HubState, OpenStatus, Opened, TraceContext, WakeOptions,
    WakeStatus, Woken,
};

mod attempts;
mod partner_state;
mod relay_events;

/// `yppCapabilities` flag `IntentionalDisconnect`: the partner is intentional-disconnect capable.
pub const YPP_CAPABILITY_INTENTIONAL_DISCONNECT: u64 = 0x40;

const SESSION_EVENT_CAPACITY: usize = 64;

/// Where a partner's connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    /// No attempt runs: never started, or the attempts stopped (a fatal status, the retry delay's
    /// bound, or a hub-announced disconnect that may not reconnect).
    Idle,
    /// An attempt, or the retry wait before the next one, runs.
    Attempting,
    /// The hub connection is reconnecting (`ConnectingExternally`): no attempts of our own until it is
    /// `Connected` again or closes, or the partner re-appears; a connect request still starts one.
    Paused,
    /// The partner is on the hub. An attempt or a retry wait from before the partner joined by itself
    /// may still run to its end.
    Connected,
    /// Stopped by a local disconnect.
    Stopped,
    /// The phone ended the session (`/PlatformDisconnectRequest`); only a `NewSession` start leaves it.
    SessionEnded { reason: i32 },
}

/// What the driver is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Starting,
    /// The retry wait between attempts (not an attempt).
    Backoff {
        delay: Duration,
    },
    Open,
    Announce,
    Wake,
}

/// The task that runs attempts for one partner.
struct Driver {
    id: u64,
    /// Ends the driver (local stop, session ended, interruption, pause, connected during a retry wait).
    stop: CancellationToken,
    /// Cancels the current step only (a connect request; an interruption ends a retry wait with it);
    /// also cancelled with `stop`.
    current: CancellationToken,
    step: Step,
    /// The running attempt runs on the extended budget: as it started, the retry delay had grown past
    /// its start value or the next wake used the long TTL. Cleared when the hub opens (the delays are
    /// reset and the rest of the attempt is judged anew).
    extended: bool,
    /// A connect request arrived while an attempt not on the extended budget ran, or a close of the
    /// hub connection interrupted a running attempt: the next attempt follows at once.
    queued: bool,
    /// The failure the next retry wait follows; `None` when the next attempt runs at once.
    wait_after: Option<ConnectionFailure>,
    /// The trace context of the connect request that started this driver or its next attempt: a hub
    /// open of these attempts carries it in its connection headers.
    /// `None` for automatic attempts, whose opens get a fresh root context.
    trace: Option<TraceContext>,
}

struct PartnerState {
    phase: SessionPhase,
    /// Meaningful while `phase` is `Paused` (every transition into it sets it): the partner was
    /// attempting, so its attempts resume once the hub is `Connected` again; `false` for a partner that
    /// was connected, which waits to re-appear.
    resume_on_reconnected: bool,
    /// A `SessionEvent::Connected` was broadcast and no `Disconnected` since.
    announced: bool,
    last_failure: Option<ConnectionFailure>,
    /// Counts the times the attempts stopped by themselves; `stop_failure` is the failure of the last.
    /// A waiter that began at a lower count ends with that failure.
    stops: u64,
    stop_failure: Option<ConnectionFailure>,
    pending_new_session: u32,
    pending_ignore_device_disabled: u32,
    retry_delay: Duration,
    /// The TTL of the next wake an attempt sends: the default, or the long TTL after an odd number of
    /// `DeviceUnreachable` attempts since the delays were last reset.
    wake_ttl: Duration,
    capable_from_metadata: bool,
    capable_from_disconnect: bool,
    driver: Option<Driver>,
    changed: watch::Sender<u64>,
}

impl PartnerState {
    fn new(base: Duration, wake_ttl: Duration) -> Self {
        Self {
            phase: SessionPhase::Idle,
            resume_on_reconnected: false,
            announced: false,
            last_failure: None,
            stops: 0,
            stop_failure: None,
            pending_new_session: 0,
            pending_ignore_device_disabled: 0,
            retry_delay: base,
            wake_ttl,
            capable_from_metadata: false,
            capable_from_disconnect: false,
            driver: None,
            changed: watch::channel(0).0,
        }
    }

    fn notify(&self) {
        self.changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Ends the driver; `true` when it was running an attempt (not waiting), which is then cancelled.
    fn stop_driver(&mut self) -> bool {
        match self.driver.take() {
            Some(driver) => {
                driver.stop.cancel();
                driver.current.cancel();
                !matches!(driver.step, Step::Backoff { .. })
            }
            None => false,
        }
    }

    /// Externally connected: the phone ended the session but is still on the hub and the last failure
    /// is not persistent; `on_hub` is the hub's view of the partner.
    fn usable_while_ended(&self, on_hub: bool) -> bool {
        on_hub && !is_persistent(self.last_failure.as_ref())
    }
}

enum AttemptResult {
    /// The partner is on the hub: found there and re-announced (`woken` false) or woken.
    Connected {
        woken: bool,
    },
    Cancelled,
    /// The attempt failed: its failure, and what failed when there is more to say than the status.
    Failed {
        failure: ConnectionFailure,
        cause: Option<String>,
    },
}

/// What [`PartnerSessions::begin_attempt`] hands the attempt it starts.
struct AttemptStart {
    current: CancellationToken,
    /// The trace of the connect request behind the attempt; `None` for an automatic one.
    trace: Option<TraceContext>,
}

/// What [`PartnerSessions::begin_backoff`] decides before an attempt.
enum Backoff {
    /// The next attempt runs at once.
    None,
    /// Wait `delay` first (cut short when the token is cancelled).
    Wait(Duration, CancellationToken),
    /// The driver ends (stopped, replaced, or the retry delay reached its bound).
    Stop,
}

/// What ending a session as the phone asked leaves to report.
enum Ended {
    /// It was reported connected: `Disconnected` is broadcast.
    Announce,
    /// The phone stays on the hub and the last failure is not persistent: still usable.
    ExternallyConnected,
    /// It was never reported connected.
    NotAnnounced,
}

/// How [`PartnerSessions::wait_connected`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    Connected,
    /// The connection reached a terminal state that is not connected: the session ended by the phone,
    /// a local stop, or the attempts stopped (with their last failure).
    Failed(ConnectionFailure),
    TimedOut,
}

/// What a connect request or a wait learns once the connection is decided for it.
enum Decided {
    Connected,
    Failed(ConnectionFailure),
}

impl Decided {
    fn into_connect(self) -> ConnectOutcome {
        match self {
            Decided::Connected => ConnectOutcome::Connected,
            Decided::Failed(failure) => ConnectOutcome::Failed(failure),
        }
    }

    fn into_wait(self) -> WaitOutcome {
        match self {
            Decided::Connected => WaitOutcome::Connected,
            Decided::Failed(failure) => WaitOutcome::Failed(failure),
        }
    }
}

/// Failure statuses that survive a `Default` connect request: LTW disabled (from a wake), a local stop,
/// or a phone disconnect with reason `0`–`4` (an unparsable reason is `0`).
fn is_persistent(failure: Option<&ConnectionFailure>) -> bool {
    match failure {
        Some(ConnectionFailure::Wake(WakeStatus::LtwDisabled) | ConnectionFailure::LocalStop) => true,
        Some(ConnectionFailure::SessionEnded { reason }) => is_persistent_disconnect_reason(*reason),
        _ => false,
    }
}

/// Wake results that stop the attempts (the fatal wake results).
///
/// A Dispatcher `Failure` (any status the Dispatcher mapping does not name — 400, 403 other than
/// LTW disabled, 429, 5xx —, a call without a usable reply that is neither an internet failure nor
/// a timeout, a services-token failure outside the DCG auth manager, and a wake the PC could not
/// build), the device not found, an incompatible region, LTW disabled, and an MSA token that needs
/// the user.
pub fn is_fatal_wake_status(status: WakeStatus) -> bool {
    matches!(
        status,
        WakeStatus::Failure
            | WakeStatus::DeviceNotFoundError
            | WakeStatus::IncompatibleRegionError
            | WakeStatus::LtwDisabled
            | WakeStatus::MsaTokenUserInteractionRequired
    )
}

fn is_fatal(failure: &ConnectionFailure) -> bool {
    match failure {
        ConnectionFailure::Hub(status) => status.is_fatal(),
        ConnectionFailure::Wake(status) => is_fatal_wake_status(*status),
        _ => false,
    }
}

/// Every per-partner connection of the process (`PartnerSessionControl`, and the fragment layer's
/// `PartnerConnector`).
pub struct PartnerSessions {
    me: Weak<PartnerSessions>,
    config: Arc<EngineConfig>,
    hub: Arc<dyn HubControl>,
    waker: Arc<dyn Waker>,
    router: Arc<Router>,
    tap: Tap,
    partners: Mutex<HashMap<DcgClient, PartnerState>>,
    /// The state of each environment's hub connection as the last relay event reported it: a
    /// `Disconnected` that follows `Connected` or `Reconnecting` is a close, one that follows
    /// `Connecting` a failed open.
    hub_states: Mutex<HashMap<Environment, HubState>>,
    events: broadcast::Sender<SessionEvent>,
    next_driver: AtomicU64,
    started: AtomicBool,
}

impl PartnerSessions {
    /// Waits up to `timeout` for `partner`'s connection to reach a terminal state, across its attempts:
    /// `Connected`; `Failed` when the session is ended by the phone or stopped locally (at once when it
    /// already is) or when the attempts stop during the wait; `TimedOut` when the whole `timeout`
    /// passed. A failed attempt does not end the wait. (An App request does not wait past its connect.)
    pub async fn wait_connected(&self, partner: &DcgClient, timeout: Duration) -> WaitOutcome {
        let deadline = tokio::time::Instant::now() + timeout;
        let (stops_seen, mut changed) = {
            let mut map = self.partners.lock_or_panic();
            let s = self.entry(&mut map, partner);
            (s.stops, s.changed.subscribe())
        };

        loop {
            if let Some(decided) = self.evaluate(partner, stops_seen) {
                return decided.into_wait();
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    return self.evaluate(partner, stops_seen).map_or(WaitOutcome::TimedOut, Decided::into_wait);
                }
                changed = changed.changed() => {
                    if changed.is_err() {
                        error!(partner = %partner, "the connection's change notifier is gone; the wait ends");
                        return self.evaluate(partner, stops_seen).map_or(WaitOutcome::TimedOut, Decided::into_wait);
                    }
                }
            }
        }
    }

    pub fn new(
        config: Arc<EngineConfig>,
        hub: Arc<dyn HubControl>,
        waker: Arc<dyn Waker>,
        router: Arc<Router>,
        tap: Tap,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| PartnerSessions {
            me: me.clone(),
            config,
            hub,
            waker,
            router,
            tap,
            partners: Mutex::default(),
            hub_states: Mutex::default(),
            events: broadcast::channel(SESSION_EVENT_CAPACITY).0,
            next_driver: AtomicU64::new(1),
            started: AtomicBool::new(false),
        })
    }

    /// Subscribes to the relay's [`RelayEvent`]s and spawns the loop that applies them (presence,
    /// disconnect gating, hub state). Idempotent; call it inside the tokio runtime.
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }

        let events = self.hub.subscribe();
        {
            // The states the first events follow.
            let mut states = self.hub_states.lock_or_panic();
            for env in Environment::ALL {
                states.insert(env, self.hub.hub_state(env));
            }
        }

        tokio::spawn(relay_event_loop(Arc::downgrade(self), events));
    }

    /// Local disconnect: stop the connection, send the `{}` `/PlatformDisconnectRequest` when connected,
    /// then close the hub connection unless another partner of the environment uses it.
    pub async fn disconnect(&self, partner: &DcgClient) {
        self.disconnect_with(partner, false).await;
    }

    /// [`PartnerSessions::disconnect`] with the caller's choice of `SkipPartnerAck` for the disconnect
    /// request. Bounded by `session_maximum_wait`; the request itself by 30 s. A session the
    /// phone ended stays ended (with its failure); any other connection becomes `Stopped`, and its last
    /// failure becomes a local stop only when it was connected.
    pub async fn disconnect_with(&self, partner: &DcgClient, skip_partner_ack: bool) {
        let work = async {
            let connected = self.is_connected(partner);
            let announce = {
                let mut map = self.partners.lock_or_panic();
                let s = self.entry(&mut map, partner);
                if s.stop_driver() {
                    self.reset_delays(s);
                }

                let failure = match s.phase {
                    SessionPhase::SessionEnded { reason } => {
                        debug!(partner = %partner, reason, "local disconnect of a session the phone ended; it stays ended");
                        ConnectionFailure::SessionEnded { reason }
                    }
                    phase => {
                        if phase == SessionPhase::Connected {
                            s.last_failure = Some(ConnectionFailure::LocalStop);
                        }
                        s.phase = SessionPhase::Stopped;
                        ConnectionFailure::LocalStop
                    }
                };
                s.notify();
                std::mem::replace(&mut s.announced, false).then_some(failure)
            };

            if let Some(failure) = announce {
                self.emit_disconnected(partner, failure, "local_stop");
            }

            if connected {
                let timeout = self.config.platform.send_disconnect_request_timeout;
                let response = send_disconnect_request(&self.router, partner, skip_partner_ack, timeout).await;
                if response.status != PlatformRequestStatus::Success {
                    warn!(partner = %partner, status = ?response.status, error = response.status.cause(), "/PlatformDisconnectRequest not delivered; the local disconnect proceeds");
                }
            }

            if self.env_in_use(partner) {
                debug!(partner = %partner, "hub connection kept: another partner of the environment uses it");
            } else {
                self.hub.close(partner.env).await;
            }
        };

        if tokio::time::timeout(self.config.platform.session_maximum_wait, work).await.is_err() {
            warn!(partner = %partner, "local disconnect exceeded session_maximum_wait; abandoned, the hub connection may stay open");
        }
    }

    /// Records the partner's `yppCapabilities` (device-list `customData` or connect response): bit `0x40`
    /// marks it capable of intentional disconnects, and once marked it stays capable — a later value
    /// without the bit does not clear it. Call after each connect and device-list refresh.
    pub fn set_partner_ypp_capabilities(&self, partner: &DcgClient, ypp_capabilities: u64) {
        let mut map = self.partners.lock_or_panic();
        let s = self.entry(&mut map, partner);
        if ypp_capabilities & YPP_CAPABILITY_INTENTIONAL_DISCONNECT != 0 {
            s.capable_from_metadata = true;
        }
    }

    /// Capable from the device metadata or marked by an inbound `/PlatformDisconnectRequest`.
    pub fn is_intentional_disconnect_capable(&self, partner: &DcgClient) -> bool {
        let map = self.partners.lock_or_panic();
        map.get(partner).is_some_and(|s| s.capable_from_metadata || s.capable_from_disconnect)
    }

    pub fn phase(&self, partner: &DcgClient) -> SessionPhase {
        let map = self.partners.lock_or_panic();
        map.get(partner).map_or(SessionPhase::Idle, |s| s.phase)
    }

    pub fn last_failure(&self, partner: &DcgClient) -> Option<ConnectionFailure> {
        let map = self.partners.lock_or_panic();
        map.get(partner).and_then(|s| s.last_failure.clone())
    }

    /// Every known partner that is connected (the MSAEP publisher's targets).
    pub fn connected_partners(&self) -> Vec<DcgClient> {
        let known: Vec<DcgClient> = self.partners.lock_or_panic().keys().cloned().collect();
        known.into_iter().filter(|p| self.is_connected(p)).collect()
    }

    /// The pending `(DCG-RequestNewSession, IgnoreDeviceDisabledStatus)` counters (diagnostics).
    pub fn pending_wake_options(&self, partner: &DcgClient) -> (u32, u32) {
        let map = self.partners.lock_or_panic();
        map.get(partner).map_or((0, 0), |s| (s.pending_new_session, s.pending_ignore_device_disabled))
    }

    /// Inbound `/PlatformDisconnectRequest`: mark the partner intentional-disconnect capable and, unless
    /// `keep_session_on_peer_disconnect`, end the session (terminal; no automatic reconnect or wake).
    /// With a reason that is not persistent (5, 6) and the phone still on the hub the connection stays
    /// usable, so no `Disconnected` is broadcast.
    pub(crate) fn on_remote_disconnect_request(&self, partner: &DcgClient, reason: i32) {
        let on_hub = self.hub.is_partner_connected(partner);
        let ended = {
            let mut map = self.partners.lock_or_panic();
            let s = self.entry(&mut map, partner);
            s.capable_from_disconnect = true;
            if self.config.relay.keep_session_on_peer_disconnect {
                info!(partner = %partner, reason, "the phone sent /PlatformDisconnectRequest; the session is kept (keep_session_on_peer_disconnect)");
                return;
            }
            self.end_session_locked(s, reason, on_hub)
        };

        match ended {
            Ended::Announce => {
                self.emit_disconnected(partner, ConnectionFailure::SessionEnded { reason }, "session_ended");
            }
            Ended::ExternallyConnected => {
                info!(partner = %partner, reason, "the phone ended the session but stays on the hub; externally connected");
            }
            Ended::NotAnnounced => {
                debug!(partner = %partner, reason, on_hub, "the phone ended a session that was not reported connected");
            }
        }
    }

    /// Ends the partner's session as the phone asked (under the partners lock).
    fn end_session_locked(&self, s: &mut PartnerState, reason: i32, on_hub: bool) -> Ended {
        if s.stop_driver() {
            self.reset_delays(s);
        }

        s.phase = SessionPhase::SessionEnded { reason };
        s.last_failure = Some(ConnectionFailure::SessionEnded { reason });
        s.notify();

        if s.usable_while_ended(on_hub) {
            Ended::ExternallyConnected
        } else if std::mem::replace(&mut s.announced, false) {
            Ended::Announce
        } else {
            Ended::NotAnnounced
        }
    }
}

async fn relay_event_loop(sessions: Weak<PartnerSessions>, mut events: broadcast::Receiver<RelayEvent>) {
    loop {
        let event = events.recv().await;
        let Some(sessions) = sessions.upgrade() else {
            return;
        };

        match event {
            Ok(event) => sessions.on_relay_event(event),
            Err(RecvError::Lagged(missed)) => {
                warn!(missed, "relay events lagged; re-aligning partners with the hub's partner set and state");
                sessions.resync();
            }
            Err(RecvError::Closed) => {
                debug!("relay event stream closed");
                return;
            }
        }
    }
}

/// The result of a [`PartnerSessions::spawn_open`] task; a task that failed (the open panicked)
/// counts as a retryable `Failure`, with the panic as its cause.
fn joined_open(joined: Result<Opened, JoinError>) -> Opened {
    joined.unwrap_or_else(|e| Opened {
        status: OpenStatus::Failure,
        cause: Some(format!("the hub open task failed: {e}")),
    })
}

#[async_trait]
impl PartnerSessionControl for PartnerSessions {
    /// The connection start policies `Default` and `NewSession`. On a connected partner both send the
    /// one-way `/PlatformActivateSessionRequest` and report connected, so the App layer calls this only
    /// while the partner is not connected. Otherwise the request waits up to its feedback window for a
    /// terminal state. The request's trace context goes to a hub open of the attempt it starts.
    async fn connect(&self, partner: &DcgClient, request: ConnectRequest) -> ConnectOutcome {
        let on_hub = self.hub.is_partner_connected(partner);
        let ended = matches!(self.phase(partner), SessionPhase::SessionEnded { .. });
        if !ended && self.is_connected(partner) {
            self.mark_connected(partner);
            self.spawn_activate(partner);
            return ConnectOutcome::Connected;
        }

        let (stops_seen, mut changed) = {
            let mut map = self.partners.lock_or_panic();
            let s = self.entry(&mut map, partner);
            if request.ignore_device_disabled {
                s.pending_ignore_device_disabled += 1;
            }

            if let SessionPhase::SessionEnded { reason } = s.phase {
                if request.policy == StartPolicy::Default {
                    if !is_persistent(s.last_failure.as_ref()) {
                        s.last_failure = None;
                    }
                    if s.usable_while_ended(on_hub) {
                        debug!(partner = %partner, reason, "Default connect of a session the phone ended; still on the hub, externally connected");
                        return ConnectOutcome::Connected;
                    }
                    debug!(partner = %partner, reason, "Default connect ignored; the phone ended the session");
                    return ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason });
                }
            }

            match request.policy {
                StartPolicy::Default => {
                    if !is_persistent(s.last_failure.as_ref()) {
                        s.last_failure = None;
                    }
                }
                StartPolicy::NewSession => {
                    s.last_failure = None;
                    s.pending_new_session += 1;
                }
            }

            match s.driver.as_mut() {
                // The next attempt runs for this request: its open carries this request's trace.
                Some(driver) if matches!(driver.step, Step::Backoff { .. }) => {
                    debug!(partner = %partner, step = ?driver.step, "connect request ends the retry wait; the next attempt starts at once");
                    driver.current.cancel();
                    driver.trace = Some(request.trace.clone());
                }
                Some(driver) if driver.extended => {
                    debug!(partner = %partner, step = ?driver.step, "connect request cancels the attempt running on the extended budget");
                    driver.current.cancel();
                    driver.trace = Some(request.trace.clone());
                }
                Some(driver) => {
                    driver.queued = true;
                    driver.trace = Some(request.trace.clone());
                }
                None => self.spawn_driver(partner, s, ConnectReason::ConnectRequested, Some(request.trace.clone())),
            }

            s.phase = SessionPhase::Attempting;
            s.notify();
            (s.stops, s.changed.subscribe())
        };

        self.await_outcome(partner, stops_seen, request.feedback_window, &mut changed).await
    }

    /// The partner is on the hub and was not stopped locally; a session the phone ended counts only
    /// while its last failure is not persistent (externally connected).
    fn is_connected(&self, partner: &DcgClient) -> bool {
        if !self.hub.is_partner_connected(partner) {
            return false;
        }

        let map = self.partners.lock_or_panic();
        map.get(partner).is_none_or(|s| match s.phase {
            SessionPhase::Stopped => false,
            SessionPhase::SessionEnded { .. } => s.usable_while_ended(true),
            _ => true,
        })
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }
}

#[async_trait]
impl PartnerConnector for PartnerSessions {
    /// The wake path of the fragment layer's stop-and-wait sender: open the hub when it is down — the
    /// open carrying the message's trace context, and running on when this call gives up or is dropped
    /// — then wake with the message's own wake options — `DCG-RequestNewSession` only from
    /// `keep_session_on_peer_disconnect`, no pending counter touched — and the default TTL, the
    /// connection's TTL neither used nor alternated, within `max_wait` (expiry cancels the wake and
    /// reports `TimedOut`).
    async fn connect_for_send(
        &self,
        target: &DcgClient,
        ignore_device_disabled: bool,
        max_wait: Duration,
        trace: &TraceContext,
    ) -> Result<(), DcgSendStatus> {
        let on_hub =
            |hub: &dyn HubControl| hub.hub_state(target.env) == HubState::Connected && hub.is_partner_connected(target);
        if on_hub(self.hub.as_ref()) {
            return Ok(());
        }

        let deadline = tokio::time::Instant::now() + max_wait;
        if self.hub.hub_state(target.env) != HubState::Connected {
            // Neither the deadline nor a caller dropping this future abandons the open (`spawn_open`).
            let open = self.spawn_open(target, ConnectReason::SendFragment, trace.clone());
            match tokio::time::timeout_at(deadline, open).await {
                Err(_) => return Err(DcgSendStatus::TimedOut),
                Ok(joined) => {
                    let opened = joined_open(joined);
                    if opened.status != OpenStatus::Success {
                        return Err(DcgSendStatus::open_failed(opened));
                    }
                }
            }

            if on_hub(self.hub.as_ref()) {
                return Ok(());
            }
        }

        let options = WakeOptions {
            request_new_session: self.config.relay.keep_session_on_peer_disconnect,
            ignore_device_disabled,
        };
        let ttl = self.config.auth.wake.ttl_default;
        let cancel = CancellationToken::new();
        let waker = self.waker.clone();
        let wake_target = target.clone();
        let wake_cancel = cancel.clone();
        let mut wake = tokio::spawn(async move { waker.wake(&wake_target, options, ttl, wake_cancel).await });

        tokio::select! {
            woken = &mut wake => match woken {
                Ok(woken) if woken.status == WakeStatus::Success => Ok(()),
                Ok(woken) => Err(DcgSendStatus::wake_failed(woken)),
                Err(e) => Err(DcgSendStatus::wake_failed(Woken {
                    status: WakeStatus::Failure,
                    cause: Some(format!("the wake task failed: {e}")),
                })),
            },
            _ = tokio::time::sleep_until(deadline) => {
                cancel.cancel();
                Err(DcgSendStatus::TimedOut)
            }
        }
    }
}
