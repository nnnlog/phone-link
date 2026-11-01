//! What the relay reports: partners disconnecting and the hub's state.

use super::{PartnerSessions, PartnerState, SessionPhase, Step, is_persistent};
use phonelink_core::seams::{ConnectionFailure, RelayEvent};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{ConnectReason, DcgClient, Environment, HubState, PartnerDisconnectReason};
use std::collections::HashSet;
use tracing::{debug, info};

impl PartnerSessions {
    pub(super) fn on_relay_event(&self, event: RelayEvent) {
        match event {
            RelayEvent::PartnerConnected(partner) => self.mark_connected(&partner),
            RelayEvent::PartnerDisconnected { partner, reason } => self.on_partner_disconnected(&partner, reason),
            RelayEvent::HubState { env, state } => self.on_hub_state(env, state),
            RelayEvent::OnConnected { .. } => {}
        }
    }

    /// Reconnection gating on a partner's removal: a connected partner removed for a reason that may
    /// reconnect is interrupted and its first attempt starts at once; while its hub connection is
    /// `Reconnecting` (an automatic reconnect) it pauses instead.
    pub(super) fn on_partner_disconnected(&self, partner: &DcgClient, reason: PartnerDisconnectReason) {
        let failure = match reason {
            PartnerDisconnectReason::LocalHubDisconnect => ConnectionFailure::ConnectionDropped,
            other => ConnectionFailure::PartnerDisconnect(other),
        };

        if self.hub.is_partner_connected(partner) {
            // Events are applied asynchronously: an attempt may already have brought the partner back.
            debug!(partner = %partner, ?reason, "stale partner-disconnect event; the partner is in the hub's set again");
            return;
        }

        let hub_state = self.hub.hub_state(partner.env);
        match self.record_partner_left(partner, reason, &failure, hub_state) {
            Some((true, action)) => self.emit_disconnected(partner, failure, action),
            Some((false, action)) => info!(partner = %partner, ?reason, action, "connected partner left the hub"),
            None => {}
        }
    }

    /// Applies a partner's removal under the partners lock: whether a `Disconnected` is to be
    /// broadcast and what the connection does next, or `None` for an unknown partner or one that was
    /// not connected.
    fn record_partner_left(
        &self,
        partner: &DcgClient,
        reason: PartnerDisconnectReason,
        failure: &ConnectionFailure,
        hub_state: HubState,
    ) -> Option<(bool, &'static str)> {
        let mut map = self.partners.lock_or_panic();
        let Some(s) = map.get_mut(partner) else {
            debug!(partner = %partner, ?reason, "unknown partner left the hub");
            return None;
        };

        if !s.announced && s.phase != SessionPhase::Connected {
            debug!(partner = %partner, ?reason, phase = ?s.phase, "partner left while not connected; nothing to interrupt");
            return None;
        }

        let announce = std::mem::replace(&mut s.announced, false);
        s.last_failure = Some(failure.clone());
        let action = if s.phase == SessionPhase::Connected {
            self.interrupt_connected(s, partner, reason, failure, hub_state)
        } else {
            "none"
        };
        s.notify();
        Some((announce, action))
    }

    /// A connected partner left the hub (under the partners lock): it stops when it may not reconnect
    /// (`stop`), pauses while the hub reconnects (`pause`, or `pause_until_back` when it waits for the
    /// partner to re-appear), else its first attempt starts at once (`attempt`).
    fn interrupt_connected(
        &self,
        s: &mut PartnerState,
        partner: &DcgClient,
        reason: PartnerDisconnectReason,
        failure: &ConnectionFailure,
        hub_state: HubState,
    ) -> &'static str {
        let reconnect = self.reconnects(s, reason);
        if (!reconnect || hub_state == HubState::Reconnecting) && s.stop_driver() {
            // A cancelled attempt resets the delays.
            self.reset_delays(s);
        }

        if !reconnect {
            s.phase = SessionPhase::Idle;
            s.stops += 1;
            s.stop_failure = Some(failure.clone());
            "stop"
        } else if hub_state == HubState::Reconnecting {
            // Removed with the hub's other partners, the partner re-announces itself once the hub
            // is back; removed for another reason, it needs a wake, so its attempts resume then.
            let resume = reason != PartnerDisconnectReason::LocalHubDisconnect;
            s.phase = SessionPhase::Paused;
            s.resume_on_reconnected = resume;
            if resume { "pause" } else { "pause_until_back" }
        } else {
            self.restart_attempts(s, partner);
            "attempt"
        }
    }

    /// The disconnected partner's first attempt starts at once.
    fn restart_attempts(&self, s: &mut PartnerState, partner: &DcgClient) {
        s.phase = SessionPhase::Attempting;
        // The delays stay. A driver still running from before the partner joined by itself
        // carries on: its retry wait ends, and behind its attempt the next follows at once.
        match s.driver.as_mut() {
            Some(driver) if matches!(driver.step, Step::Backoff { .. }) => driver.current.cancel(),
            Some(driver) => driver.queued = true,
            None => self.spawn_driver(partner, s, ConnectReason::AutomaticReconnect, None),
        }
    }

    /// Whether a partner that left for `reason` reconnects automatically: a hub notice only for an
    /// intentional-disconnect capable partner with automatic reconnects on and the session not kept.
    fn reconnects(&self, s: &PartnerState, reason: PartnerDisconnectReason) -> bool {
        let relay = &self.config.relay;
        match reason {
            PartnerDisconnectReason::NoticeFromHub => {
                let capable = s.capable_from_metadata || s.capable_from_disconnect;
                capable && !relay.keep_session_on_peer_disconnect && relay.automatic_reconnects_enabled
            }
            other => other.should_reconnect(),
        }
    }

    /// The reactions to the hub connection's automatic-reconnect events, for the partners of `env`:
    /// `Reconnecting` pauses every attempting partner (a connected one is paused by its removal,
    /// [`Self::on_partner_disconnected`]); `Connected` resumes the attempts of the partners paused while
    /// attempting; `Disconnected` after `Connected` or `Reconnecting` (`Closed`) interrupts every
    /// attempting partner and restarts the attempts of every paused one at once — after `Connecting` it
    /// is a failed open, which interrupts nothing. A `Reconnecting` or `Connected` event the connection
    /// has already left is stale and skipped; a close happened whatever followed it.
    pub(super) fn on_hub_state(&self, env: Environment, state: HubState) {
        let live = self.hub.hub_state(env);
        let previous = self.hub_states.lock_or_panic().insert(env, state);

        match state {
            HubState::Reconnecting | HubState::Connected if live != state => {
                debug!(%env, ?state, ?live, "stale hub-state event; the connection has left that state");
            }
            HubState::Reconnecting => self.pause_attempting(env),
            HubState::Connected => self.resume_paused(env),
            HubState::Disconnected => {
                if matches!(previous, Some(HubState::Connected | HubState::Reconnecting)) {
                    self.interrupt_attempting(env);
                }
                self.restart_paused(env);
            }
            HubState::Connecting => {}
        }
    }

    /// The established hub connection of `env` closed: every attempting partner is interrupted (its
    /// last failure becomes `ConnectionDropped` unless it is persistent) — a retry wait ends and the
    /// next attempt starts at once; behind a running attempt the next one follows at once. The delays
    /// stay.
    pub(super) fn interrupt_attempting(&self, env: Environment) {
        let mut map = self.partners.lock_or_panic();
        for (partner, s) in map.iter_mut() {
            if partner.env != env || s.phase != SessionPhase::Attempting {
                continue;
            }
            let Some(driver) = s.driver.as_mut() else {
                continue;
            };

            info!(partner = %partner, step = ?driver.step, "hub connection closed; the partner's attempts are interrupted and the next starts at once");
            match driver.step {
                Step::Backoff { .. } => driver.current.cancel(),
                _ => driver.queued = true,
            }
            if !is_persistent(s.last_failure.as_ref()) {
                s.last_failure = Some(ConnectionFailure::ConnectionDropped);
            }
            s.notify();
        }
    }

    /// The hub connection of `env` reconnects: attempting partners stop their attempts (cancelling an
    /// in-flight wake, which resets the delays as any cancelled attempt does) and wait
    /// (`ConnectingExternally`).
    pub(super) fn pause_attempting(&self, env: Environment) {
        let mut map = self.partners.lock_or_panic();
        for (partner, s) in map.iter_mut() {
            if partner.env != env || s.phase != SessionPhase::Attempting {
                continue;
            }

            let step = s.driver.as_ref().map(|d| d.step);
            info!(partner = %partner, ?step, "hub reconnecting; the partner's attempts pause until it is back");
            if s.stop_driver() {
                self.reset_delays(s);
            }
            s.phase = SessionPhase::Paused;
            s.resume_on_reconnected = true;
            s.notify();
        }
    }

    /// The hub connection of `env` is `Connected` again: the partners paused while attempting resume
    /// their attempts at once; the ones paused while connected keep waiting to re-appear.
    pub(super) fn resume_paused(&self, env: Environment) {
        let mut map = self.partners.lock_or_panic();
        let resumed: Vec<DcgClient> = map
            .iter()
            .filter(|(p, s)| p.env == env && s.phase == SessionPhase::Paused && s.resume_on_reconnected)
            .map(|(p, _)| p.clone())
            .collect();

        for partner in resumed {
            let s = map.get_mut(&partner).expect("collected from the map");
            info!(partner = %partner, "hub connected again; the partner's paused attempts resume");
            s.phase = SessionPhase::Attempting;
            s.resume_on_reconnected = false;
            self.spawn_driver(&partner, s, ConnectReason::AutomaticReconnect, None);
            s.notify();
        }
    }

    /// The hub connection of `env` closed (`Closed`): every paused partner is interrupted and restarts
    /// its own attempts, the first at once.
    pub(super) fn restart_paused(&self, env: Environment) {
        let mut map = self.partners.lock_or_panic();
        let paused: Vec<DcgClient> = map
            .iter()
            .filter(|(p, s)| p.env == env && s.phase == SessionPhase::Paused)
            .map(|(p, _)| p.clone())
            .collect();

        for partner in paused {
            let s = map.get_mut(&partner).expect("collected from the map");
            info!(partner = %partner, "hub connection closed; the paused partner's attempts restart at once");
            s.phase = SessionPhase::Attempting;
            s.resume_on_reconnected = false;
            self.spawn_driver(&partner, s, ConnectReason::AutomaticReconnect, None);
            s.notify();
        }
    }

    /// After a lagged event stream: align every known partner with the hub's partner set, then apply
    /// the live state of each of their hub connections (a missed `Reconnecting`, `Connected` or close).
    pub(super) fn resync(&self) {
        let known: Vec<DcgClient> = self.partners.lock_or_panic().keys().cloned().collect();
        for partner in &known {
            let on_hub = self.hub.is_partner_connected(partner);
            match (on_hub, self.phase(partner)) {
                (
                    true,
                    SessionPhase::Idle | SessionPhase::Attempting | SessionPhase::Paused | SessionPhase::Stopped,
                ) => self.mark_connected(partner),
                (false, SessionPhase::Connected) => {
                    self.on_partner_disconnected(partner, PartnerDisconnectReason::LocalHubDisconnect)
                }
                _ => {}
            }
        }

        let envs: HashSet<Environment> = known.iter().map(|p| p.env).collect();
        for env in envs {
            self.on_hub_state(env, self.hub.hub_state(env));
        }
    }
}
