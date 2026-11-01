//! The connection attempts to a partner: the driver, back-off, waking and the outcome.

use super::{
    AttemptResult, AttemptStart, Backoff, Decided, Driver, PartnerSessions, PartnerState, SessionPhase, Step, is_fatal,
    joined_open,
};
use phonelink_core::seams::{ConnectOutcome, ConnectionFailure};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{
    ConnectReason, DcgClient, HubSendResult, HubState, OpenStatus, Opened, TraceContext, WakeOptions, WakeStatus,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

impl PartnerSessions {
    /// Starts a driver for `partner`, whose first attempt runs at once: a retry wait only ever follows
    /// a failed attempt.
    pub(super) fn spawn_driver(
        &self,
        partner: &DcgClient,
        state: &mut PartnerState,
        reason: ConnectReason,
        trace: Option<TraceContext>,
    ) {
        let Some(me) = self.me.upgrade() else {
            return;
        };

        let id = self.next_driver.fetch_add(1, Ordering::Relaxed);
        let stop = CancellationToken::new();
        state.driver = Some(Driver {
            id,
            stop: stop.clone(),
            current: CancellationToken::new(),
            step: Step::Starting,
            extended: false,
            queued: false,
            wait_after: None,
            trace,
        });

        let partner = partner.clone();
        tokio::spawn(async move { me.drive(partner, id, stop, reason).await });
    }

    pub(super) async fn drive(
        self: Arc<Self>,
        partner: DcgClient,
        id: u64,
        stop: CancellationToken,
        reason: ConnectReason,
    ) {
        loop {
            match self.begin_backoff(&partner, id) {
                Backoff::Stop => return,
                Backoff::None => {}
                Backoff::Wait(delay, current) => {
                    let ran_out = tokio::select! {
                        biased;
                        _ = stop.cancelled() => return,
                        _ = current.cancelled() => {
                            debug!(partner = %partner, "retry wait ended by a connect request or an interruption");
                            false
                        }
                        _ = tokio::time::sleep(delay) => true,
                    };
                    if self.connected_after_wait(&partner, id, ran_out) {
                        return;
                    }
                }
            }

            let Some(start) = self.begin_attempt(&partner, id) else {
                return;
            };
            let result = self.attempt(&partner, id, reason, &start.current, start.trace).await;

            if stop.is_cancelled() {
                return;
            }
            if !self.finish_attempt(&partner, id, result) {
                return;
            }
        }
    }

    /// The retry wait before the next attempt, if one is due: the delay doubles first, and the attempts
    /// stop once it would reach the bound.
    pub(super) fn begin_backoff(&self, partner: &DcgClient, id: u64) -> Backoff {
        let mut map = self.partners.lock_or_panic();
        let Some(s) = map.get_mut(partner) else {
            return Backoff::Stop;
        };
        let Some(driver) = s.driver.as_mut().filter(|d| d.id == id && !d.stop.is_cancelled()) else {
            return Backoff::Stop;
        };
        let Some(failure) = driver.wait_after.take() else {
            return Backoff::None;
        };

        let queued = driver.queued;
        let Some(delay) = self.double_retry_delay(s) else {
            warn!(partner = %partner, last_failure = ?s.last_failure, "partner retry delay reached the fatal bound; attempts stop");
            self.stop_attempts(s, failure);
            return Backoff::Stop;
        };
        if queued {
            // A connect request arrived before the wait began: its attempt starts at once.
            return Backoff::None;
        }

        let current = CancellationToken::new();
        let driver = s.driver.as_mut().expect("checked above");
        driver.current = current.clone();
        driver.step = Step::Backoff { delay };
        Backoff::Wait(delay, current)
    }

    /// After a retry wait: `true` (the driver ends) when the partner joined by itself meanwhile, the
    /// delays then reset if the wait ran out with the partner still on the hub.
    pub(super) fn connected_after_wait(&self, partner: &DcgClient, id: u64, ran_out: bool) -> bool {
        let on_hub = self.hub.is_partner_connected(partner);
        let mut map = self.partners.lock_or_panic();
        let Some(s) = map.get_mut(partner) else {
            return true;
        };

        if s.phase != SessionPhase::Connected {
            return false;
        }

        if s.driver.as_ref().is_some_and(|d| d.id == id) {
            s.driver = None;
            if ran_out && on_hub {
                debug!(partner = %partner, "retry wait ran out with the partner on the hub: the delays reset");
                self.reset_delays(s);
            }
        }
        true
    }

    /// Starts an attempt, judging its budget: extended when the retry delay has grown past its start
    /// value or the next wake uses the long TTL.
    pub(super) fn begin_attempt(&self, partner: &DcgClient, id: u64) -> Option<AttemptStart> {
        let base = self.config.relay.partner_retry_base;
        let default_ttl = self.config.auth.wake.ttl_default;
        let mut map = self.partners.lock_or_panic();
        let s = map.get_mut(partner)?;
        let extended = s.retry_delay > base || s.wake_ttl > default_ttl;
        let driver = s.driver.as_mut().filter(|d| d.id == id && !d.stop.is_cancelled())?;

        driver.queued = false;
        driver.extended = extended;
        let current = CancellationToken::new();
        driver.current = current.clone();
        driver.step = Step::Starting;
        Some(AttemptStart { current, trace: driver.trace.clone() })
    }

    pub(super) fn set_step(&self, partner: &DcgClient, id: u64, step: Step) {
        let mut map = self.partners.lock_or_panic();
        if let Some(driver) = map.get_mut(partner).and_then(|s| s.driver.as_mut()).filter(|d| d.id == id) {
            driver.step = step;
        }
    }

    /// The hub connection opened for the attempt: the delays are reset, and the rest of the attempt no
    /// longer runs on the extended budget.
    pub(super) fn on_opened(&self, partner: &DcgClient, id: u64) {
        let mut map = self.partners.lock_or_panic();
        let Some(s) = map.get_mut(partner) else {
            return;
        };

        self.reset_delays(s);
        if let Some(driver) = s.driver.as_mut().filter(|d| d.id == id) {
            driver.extended = false;
        }
    }

    /// Runs [`HubControl::open`] in a task of its own, so that neither a cancelled attempt nor a
    /// caller that stops waiting abandons it midway: an open dropped after it started would leave the
    /// relay's connection half-started (`Connecting`) for every later open. Dropping the handle only
    /// detaches it.
    pub(super) fn spawn_open(
        &self,
        partner: &DcgClient,
        reason: ConnectReason,
        trace: TraceContext,
    ) -> JoinHandle<Opened> {
        let hub = self.hub.clone();
        let partner = partner.clone();
        tokio::spawn(async move { hub.open(&partner, reason, &trace).await })
    }

    /// The options and TTL of a wake an attempt builds: each pending counter gives one wake its flag,
    /// and the TTL is the connection's next one.
    pub(super) fn take_wake_params(&self, partner: &DcgClient) -> (WakeOptions, Duration) {
        let mut map = self.partners.lock_or_panic();
        let s = self.entry(&mut map, partner);

        let options = WakeOptions {
            request_new_session: self.config.relay.keep_session_on_peer_disconnect || s.pending_new_session > 0,
            ignore_device_disabled: s.pending_ignore_device_disabled > 0,
        };

        s.pending_new_session = s.pending_new_session.saturating_sub(1);
        s.pending_ignore_device_disabled = s.pending_ignore_device_disabled.saturating_sub(1);
        (options, s.wake_ttl)
    }

    /// One attempt: re-announce when the partner is on the hub; open the hub when it is down (then
    /// evaluate again); else wake the partner and wait for it. `trace` is the trace context of the
    /// connect request behind the attempt; an open carries it, or a fresh root context when the
    /// attempt is automatic. A `Cancelled` open or wake is a cancelled attempt, whoever cancelled it.
    pub(super) async fn attempt(
        &self,
        partner: &DcgClient,
        id: u64,
        reason: ConnectReason,
        current: &CancellationToken,
        mut trace: Option<TraceContext>,
    ) -> AttemptResult {
        let mut opened = false;
        loop {
            if current.is_cancelled() {
                return AttemptResult::Cancelled;
            }

            let hub_state = self.hub.hub_state(partner.env);
            if hub_state == HubState::Connected && self.hub.is_partner_connected(partner) {
                return self.reannounce(partner, id, current).await;
            }

            if hub_state != HubState::Connected {
                if opened {
                    return AttemptResult::Failed {
                        failure: ConnectionFailure::Hub(OpenStatus::Failure),
                        cause: Some(format!(
                            "the hub reported a successful open but is {hub_state:?} (broken invariant)"
                        )),
                    };
                }

                if let Some(result) = self.open_hub(partner, id, reason, current, &mut trace).await {
                    return result;
                }
                opened = true;
                continue;
            }

            return self.wake_partner(partner, id, current).await;
        }
    }

    /// The attempt's announce step: the partner is on the hub, so it is re-announced and reported
    /// connected whatever the send's result.
    async fn reannounce(&self, partner: &DcgClient, id: u64, current: &CancellationToken) -> AttemptResult {
        self.set_step(partner, id, Step::Announce);
        let sent = tokio::select! {
            biased;
            _ = current.cancelled() => return AttemptResult::Cancelled,
            sent = self.hub.send_connected(partner) => sent,
        };

        if sent != HubSendResult::Success {
            warn!(partner = %partner, result = ?sent, "re-announce failed; the partner is in the hub's set, reporting connected");
        }
        AttemptResult::Connected { woken: false }
    }

    /// The attempt's open step: opens the hub with the attempt's trace (taken once); `None` once it
    /// opened, else how the attempt ends.
    async fn open_hub(
        &self,
        partner: &DcgClient,
        id: u64,
        reason: ConnectReason,
        current: &CancellationToken,
        trace: &mut Option<TraceContext>,
    ) -> Option<AttemptResult> {
        self.set_step(partner, id, Step::Open);

        // At most one open per attempt (`opened`), so the trace is taken once.
        let open_trace = trace.take().unwrap_or_else(|| TraceContext::new_root(Vec::new()));
        let mut open = self.spawn_open(partner, reason, open_trace);
        let opened = tokio::select! {
            biased;
            // A cancelled attempt leaves its open running (`Self::spawn_open`).
            _ = current.cancelled() => return Some(AttemptResult::Cancelled),
            joined = &mut open => joined_open(joined),
        };

        match opened.status {
            OpenStatus::Success => {}
            OpenStatus::Cancelled => return Some(AttemptResult::Cancelled),
            other => {
                return Some(AttemptResult::Failed { failure: ConnectionFailure::Hub(other), cause: opened.cause });
            }
        }

        self.on_opened(partner, id);
        None
    }

    /// The attempt's wake step: wakes the partner and waits for it.
    async fn wake_partner(&self, partner: &DcgClient, id: u64, current: &CancellationToken) -> AttemptResult {
        let (options, ttl) = self.take_wake_params(partner);
        self.set_step(partner, id, Step::Wake);
        debug!(partner = %partner, ?options, ?ttl, "waking the partner");
        let woken = self.waker.wake(partner, options, ttl, current.clone()).await;

        match woken.status {
            WakeStatus::Success => AttemptResult::Connected { woken: true },
            WakeStatus::Cancelled => AttemptResult::Cancelled,
            other => AttemptResult::Failed { failure: ConnectionFailure::Wake(other), cause: woken.cause },
        }
    }

    /// Applies the attempt's result; `false` when the driver ends.
    pub(super) fn finish_attempt(&self, partner: &DcgClient, id: u64, result: AttemptResult) -> bool {
        match result {
            AttemptResult::Connected { woken } => self.finish_connected(partner, id, woken),
            AttemptResult::Cancelled => {
                let mut map = self.partners.lock_or_panic();
                let Some(s) = map.get_mut(partner) else {
                    return false;
                };

                match s.driver.as_mut() {
                    Some(d) if d.id == id && !d.stop.is_cancelled() => {
                        debug!(partner = %partner, "attempt cancelled; the next one runs at once");
                        d.wait_after = None;
                        self.reset_delays(s);
                        true
                    }
                    _ => false,
                }
            }
            AttemptResult::Failed { failure, cause } => self.finish_failed(partner, id, failure, cause.as_deref()),
        }
    }

    /// A connected attempt: the session is marked connected and the driver ends, unless the partner
    /// left again while it ran and a queued attempt follows at once.
    fn finish_connected(&self, partner: &DcgClient, id: u64, woken: bool) -> bool {
        let on_hub = self.hub.is_partner_connected(partner);
        {
            let mut map = self.partners.lock_or_panic();
            let Some(s) = map.get_mut(partner) else {
                return false;
            };
            if woken {
                self.reset_delays(s);
            }

            let Some(driver) = s.driver.as_mut().filter(|d| d.id == id) else {
                return false;
            };
            if driver.queued && !on_hub {
                // The partner left again while the attempt ran: the queued attempt follows at once.
                driver.wait_after = None;
                return true;
            }
        }

        self.mark_connected(partner);

        let mut map = self.partners.lock_or_panic();
        if let Some(s) = map.get_mut(partner) {
            if s.driver.as_ref().is_some_and(|d| d.id == id) {
                s.driver = None;
            }
        }
        false
    }

    /// A failed attempt: a fatal status stops the attempts; any other waits for the doubled retry
    /// delay (a queued attempt runs at once).
    fn finish_failed(&self, partner: &DcgClient, id: u64, failure: ConnectionFailure, cause: Option<&str>) -> bool {
        let mut map = self.partners.lock_or_panic();
        let Some(s) = map.get_mut(partner) else {
            return false;
        };
        let Some(queued) = s.driver.as_ref().filter(|d| d.id == id).map(|d| d.queued) else {
            return false;
        };

        if s.phase == SessionPhase::Connected {
            return self.failed_after_join(s, partner, failure, cause);
        }

        s.last_failure = Some(failure.clone());
        if is_fatal(&failure) {
            warn!(partner = %partner, ?failure, error = cause.unwrap_or_default(), "connection attempt failed with a fatal status; attempts stop");
            self.stop_attempts(s, failure);
            return false;
        }

        warn!(partner = %partner, ?failure, error = cause.unwrap_or_default(), "connection attempt failed");
        if failure == ConnectionFailure::Wake(WakeStatus::DeviceUnreachable) {
            self.alternate_wake_ttl(s);
        }

        if !self.schedule_retry(s, partner, failure, queued) {
            return false;
        }
        s.notify();
        true
    }

    /// After a failed attempt: a queued attempt runs at once with the delay doubled (`false` when
    /// the delay reached the fatal bound and the attempts stop); any other waits the delay.
    fn schedule_retry(
        &self,
        s: &mut PartnerState,
        partner: &DcgClient,
        failure: ConnectionFailure,
        queued: bool,
    ) -> bool {
        if queued {
            // The failure doubles the delay as always; a queued attempt runs without waiting it.
            if self.double_retry_delay(s).is_none() {
                warn!(partner = %partner, ?failure, "partner retry delay reached the fatal bound; attempts stop");
                self.stop_attempts(s, failure);
                return false;
            }
        } else if let Some(driver) = s.driver.as_mut() {
            driver.wait_after = Some(failure);
        }
        true
    }

    /// The partner joined by itself while this attempt failed: a fatal status stops the attempts (the
    /// connection stays connected); any other failure waits as always, and the wait running out with
    /// the partner on the hub resets the delays.
    fn failed_after_join(
        &self,
        s: &mut PartnerState,
        partner: &DcgClient,
        failure: ConnectionFailure,
        cause: Option<&str>,
    ) -> bool {
        warn!(
            partner = %partner,
            ?failure,
            error = cause.unwrap_or_default(),
            "connection attempt failed after the partner joined by itself"
        );

        if is_fatal(&failure) {
            self.stop_attempts(s, failure);
            return false;
        }

        if failure == ConnectionFailure::Wake(WakeStatus::DeviceUnreachable) {
            self.alternate_wake_ttl(s);
        }

        if let Some(driver) = s.driver.as_mut() {
            driver.queued = false;
            driver.wait_after = Some(failure);
        }
        true
    }

    /// Whether the connection is in a terminal state for a waiter that saw `stops_seen` stops when it
    /// began: connected (externally connected included); the session ended by the phone; a local
    /// stop; or the attempts stopped since. `None` while undecided.
    pub(super) fn evaluate(&self, partner: &DcgClient, stops_seen: u64) -> Option<Decided> {
        let on_hub = self.hub.is_partner_connected(partner);
        let map = self.partners.lock_or_panic();
        let s = map.get(partner)?;

        match s.phase {
            SessionPhase::SessionEnded { reason } => {
                return Some(if s.usable_while_ended(on_hub) {
                    Decided::Connected
                } else {
                    Decided::Failed(ConnectionFailure::SessionEnded { reason })
                });
            }
            SessionPhase::Stopped => return Some(Decided::Failed(ConnectionFailure::LocalStop)),
            _ => {}
        }

        if on_hub {
            return Some(Decided::Connected);
        }
        if s.stops > stops_seen {
            return Some(Decided::Failed(s.stop_failure.clone().expect("recorded with every stop")));
        }
        None
    }

    pub(super) async fn await_outcome(
        &self,
        partner: &DcgClient,
        stops_seen: u64,
        window: Duration,
        changed: &mut watch::Receiver<u64>,
    ) -> ConnectOutcome {
        let deadline = tokio::time::Instant::now() + window;
        loop {
            if let Some(decided) = self.evaluate(partner, stops_seen) {
                return decided.into_connect();
            }

            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {
                    return self
                        .evaluate(partner, stops_seen)
                        .map_or_else(|| ConnectOutcome::Pending { last_failure: self.last_failure(partner) }, Decided::into_connect);
                }
                changed = changed.changed() => {
                    if changed.is_err() {
                        return ConnectOutcome::Pending { last_failure: self.last_failure(partner) };
                    }
                }
            }
        }
    }
}
