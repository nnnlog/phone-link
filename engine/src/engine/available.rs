//! The Mac's availability on the relay hub: from the phone's selection until the Mac locks, the
//! phone leaves the account, or the engine stops, the connection's hub connection is kept open and
//! the phone hears the Mac announce itself (`SendConnectedAsync`), whatever the per-partner
//! connection is doing.
//!
//! Windows delivers a phone-initiated wake to Phone Link through its push channel (LTW asks the
//! Dispatcher to wake it, and Phone Link on Windows has registered one). The engine implements no
//! push channel — nothing on the Mac can receive such a wake — so a Mac that does not run the app
//! still cannot be woken. What Windows has *without* a wake, and what this adds instead, is that
//! LTW finds the Mac on the hub by itself: it connects to the same relay environment, the Mac's
//! open connection announces itself, and the traffic LTW then starts (its first contact
//! auto-assigned to that connection, its `OnPartnerConnected` answered) makes the hub report
//! `PartnerConnected`, which marks the session connected — the App layer's *became connected*
//! trigger answers with its own connect.
//!
//! [`Available`] holds the target, set and cleared by the core's own reactions; [`Runner`], spawned
//! once per core, sees it and keeps the hub connection open and announced. Lock and sleep pause it
//! ([`EngineCore::system_event`]); unlock and wake resume it — the runner also consults the session
//! lock (`Edges::lock`) before every open and announce, so a round never runs for a Mac the engine
//! sees as locked (`Unknown` counts as locked). Stopping the engine and the account's removal pause
//! it too. The App layer's own disconnects (`LockOrSleep`, the per-phone switch, a phone changed)
//! stop their session's use of the connection as they already do; the availability puts the
//! connection back, not the session.

use std::sync::Arc;

use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

use crate::edge::{LockState, SessionLock};
use crate::platform::PartnerSessions;
use crate::relay::RelayManager;
use crate::seams::{HubControl, RelayEvent};
use crate::types::{ConnectReason, DcgClient, HubSendResult, HubState, OpenStatus, TraceContext};
use super::EngineCore;

/// The engine's availability target: when `Some(phone)`, the runner keeps `phone`'s hub connection
/// open and announced; when `None`, it keeps nothing and waits.
pub(super) struct Available {
    target: watch::Sender<Option<DcgClient>>,
}

impl Available {
    pub(super) fn new() -> Self {
        let (target, _) = watch::channel(None);
        Self { target }
    }

    /// The engine runs with `phone`; the runner makes it available.
    pub(super) fn offer(&self, phone: DcgClient) {
        self.target.send_replace(Some(phone));
    }

    /// Nothing is available until the next offer (the Mac locked or slept, the phone left the
    /// account's linked devices, the engine stops, or no phone is selected).
    pub(super) fn pause(&self) {
        self.target.send_replace(None);
    }

    /// The receiver the [`Runner`] is built with; subscribe before the first offer so the first
    /// target is not missed.
    pub(super) fn subscribe(&self) -> watch::Receiver<Option<DcgClient>> {
        self.target.subscribe()
    }
}

/// The task that keeps the current target's hub connection open and announced.
pub(super) struct Runner {
    relay: Arc<RelayManager>,
    /// The partners' sessions: while a session of the target is not idle, the connection belongs to
    /// it (a driver opens it, the partner holds it), and the availability stands down, so a session
    /// open's trace keeps naming its own trigger.
    sessions: Arc<PartnerSessions>,
    lock: Arc<dyn SessionLock>,
    target: watch::Receiver<Option<DcgClient>>,
    events: broadcast::Receiver<RelayEvent>,
}

impl Runner {
    /// Builds from the engine core's pieces.
    pub(super) fn new(
        relay: Arc<RelayManager>,
        sessions: Arc<PartnerSessions>,
        lock: Arc<dyn SessionLock>,
        target: watch::Receiver<Option<DcgClient>>,
        events: broadcast::Receiver<RelayEvent>,
    ) -> Self {
        Self { relay, sessions, lock, target, events }
    }

    /// The [`Runner`]: forever, wait for a target, then keep it available until the target is
    /// paused or a new one (another phone) takes its place. The core dropping `Available` ends
    /// this.
    pub(super) async fn run(mut self) {
        loop {
            let phone = loop {
                let current = self.target.borrow_and_update().clone();
                match current {
                    Some(phone) => break phone,
                    None => {
                        if self.target.changed().await.is_err() {
                            debug!("availability ended; the engine core is gone");
                            return;
                        }
                    }
                }
            };

            info!(phone = %phone, "the Mac is available to the phone on the relay hub");
            if self.keep_available(phone).await {
                return;
            }
        }
    }

    /// Keeps `phone` available: every round re-reads the lock, the session's phase and the
    /// connection's state — while a session of the phone is not idle the connection belongs to it
    /// (a driver opens it, the partner holds it), so nothing is opened or announced; a
    /// `Disconnected` connection of a session the engine sees unlocked and idle is opened
    /// (`PersistentConnection`, an engine root trace context), a connected one announces itself —
    /// then waits for what asks to run the round again: the target gone or replaced, the connection
    /// closed or reconnecting, `OnConnected` after a reconnect brought it back, or the session's
    /// phase changing (its start or stop takes or returns the connection).
    /// `true` ends the runner (`Available`'s sender was dropped).
    async fn keep_available(&mut self, phone: DcgClient) -> bool {
        loop {
            if self.lock.lock_state() == LockState::Unlocked
                && self.sessions.is_idle(&phone)
            {
                match self.relay.hub_state(phone.env) {
                    HubState::Disconnected | HubState::Connecting => self.ensure_open(&phone).await,
                    HubState::Connected => self.announce(&phone).await,
                    // Reconnecting on its own: `OnConnected` follows, and the next round announces.
                    HubState::Reconnecting => {}
                }
            }

            let replaced = tokio::select! {
                changed = self.target.changed() => {
                    match changed {
                        // The sender is gone: the runner ends, whatever the wait had.
                        Err(_) => return true,
                        Ok(()) => match self.target.borrow_and_update().clone() {
                            Some(next) if next.is_same_client(&phone) => false,
                            _ => true,
                        },
                    }
                }
                event = self.events.recv() => match event {
                    Err(broadcast::error::RecvError::Closed) => return true,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        warn!(missed, "relay events lagged; the availability re-reads the connection's state");
                        continue;
                    }
                    Ok(event) => match event {
                        RelayEvent::HubState { env, state } if env == phone.env => {
                            // The connection's own open rounds itself off; `OnConnected` follows.
                            !matches!(state as HubState, HubState::Connecting | HubState::Reconnecting)
                        }
                        // After a reconnect of this connection the announce is sent again.
                        RelayEvent::OnConnected { env, .. } if env == phone.env => true,
                        // Another environment's events, and the partner's own comings and goings
                        // (their `OnConnected` reply is the presence's), ask for no round.
                        _ => false,
                    },
                },
                // A session start or stop takes or returns the connection; it alone does not
                // change the hub's state, so it is its own reason to round again.
                _ = self.sessions.phase_changed(&phone) => true,
            };

            if !replaced {
                continue;
            }
            return match self.target.borrow_and_update().clone() {
                // Paused: the outer loop waits for the next offer.
                None => false,
                Some(_) => {
                    debug!(phone = %phone, "the availability target changed; the runner takes the new one");
                    false
                }
            };
        }
    }

    /// Opens the connection (a second `open` of an open connection returns by its state) and
    /// announces when it succeeded.
    async fn ensure_open(&mut self, phone: &DcgClient) {
        let trace = TraceContext::new_root(Vec::new());
        let opened = self.relay.open_with_trace(phone, ConnectReason::PersistentConnection, trace).await;
        match opened.status {
            OpenStatus::Success => self.announce(phone).await,
            other => {
                warn!(
                    phone = %phone,
                    status = ?other,
                    error = opened.cause.as_deref().unwrap_or_default(),
                    "the Mac's hub connection could not be opened for its availability; it is tried again"
                );
            }
        }
    }

    /// `SendConnectedAsync(phone)`: the phone hears the Mac on the hub. A send the closed
    /// connection refused is logged and dropped; the open of the next round replaces it.
    async fn announce(&mut self, phone: &DcgClient) {
        match self.relay.send_connected_to(phone).await {
            HubSendResult::Success => {
                debug!(phone = %phone, "the Mac announced itself to the phone (SendConnectedAsync)");
            }
            HubSendResult::NotConnected => {
                debug!(phone = %phone, "announce not sent; the connection closed during it");
            }
            other => {
                debug!(phone = %phone, result = ?other, "announce not delivered; it is sent again later");
            }
        }
    }
}

impl EngineCore {
    /// Offers what the current state allows: the selected phone, only while the engine sees the
    /// Mac unlocked (`Unknown` counts as locked). The reactions call this after their own change:
    /// a phone selected, unlocked, woken, the phone unlinked, and every lock and sleep.
    pub(super) fn refresh_availability(&self) {
        let available = self.available.get_or_init(Available::new);
        match self.phone().filter(|_| self.edges.lock.lock_state() == LockState::Unlocked) {
            Some(phone) => available.offer(phone),
            None => available.pause(),
        }
    }

    /// Stops keeping anything open until the next offer (the engine cannot answer the phone any
    /// more): the engine stopping, the account removal, and a phone that left the account's linked
    /// devices.
    pub(super) fn pause_availability(&self) {
        self.available.get_or_init(Available::new).pause();
    }

    /// Spawns the availability task: one per core, in the runtime it was built in.
    pub(super) fn spawn_available_runner(self: &Arc<Self>) {
        let available = self.available.get_or_init(Available::new);
        let runner = Runner::new(
            self.relay.clone(),
            self.sessions.clone(),
            self.edges.lock.clone(),
            available.subscribe(),
            self.relay.subscribe(),
        );
        tokio::spawn(runner.run());
    }
}
