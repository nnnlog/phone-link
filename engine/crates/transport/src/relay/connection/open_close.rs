//! Opening and closing the connection.

use super::{Established, HubConnection, Session, SessionEnd, StartError, reconnect_delay, run_session};
use phonelink_core::types::{ConnectReason, HubState, OpenStatus, Opened, PartnerDisconnectReason};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

impl HubConnection {
    /// `Open(connectReason)`, the open sequence: `Success` when already connected; abandons a library
    /// reconnect; otherwise starts under the retry policy and maps the final failure to an
    /// `OpenStatus`, with its cause for the caller, which logs it. A `close` during the start
    /// answers `Cancelled`.
    ///
    /// Cancellation-safe. The sequence runs in a task of its own, serialized per connection;
    /// dropping this future (the caller's `select!` picked something else, or its timeout fired) is
    /// the caller's cancellation: the task abandons an in-flight start, reports
    /// `Disconnected` and only then releases the serialization to the next `Open`.
    pub(crate) async fn open(self: &Arc<Self>, reason: ConnectReason) -> Opened {
        let caller = CancellationToken::new();
        let gone = caller.clone().drop_guard();

        // The task logs within the caller's span.
        let task = self.clone().open_serialized(reason, caller).instrument(tracing::Span::current());
        let joined = tokio::spawn(task).await;
        gone.disarm();

        match joined {
            Ok(status) => status,
            Err(error) => match error.try_into_panic() {
                Ok(panic) => std::panic::resume_unwind(panic),
                // Only a runtime that shuts down cancels the task.
                Err(_) => OpenStatus::Cancelled.into(),
            },
        }
    }

    /// The open sequence, holding the per-connection serialization until the state it entered
    /// (`Connecting`) has been left again. `caller` is cancelled when the caller stopped waiting.
    pub(super) async fn open_serialized(self: Arc<Self>, reason: ConnectReason, caller: CancellationToken) -> Opened {
        let env = self.params.env;
        let _serial = tokio::select! {
            biased;
            serial = self.open_lock.lock() => serial,
            _ = caller.cancelled() => return OpenStatus::Cancelled.into(),
        };

        let (epoch, stop) = match self.enter_connecting(&caller).await {
            Ok(entered) => entered,
            Err(status) => return status.into(),
        };

        tracing::info!(env = %env, trigger = ?reason, "hub connection opening");
        let result = self.start_for_caller(&stop, &caller).await;

        let _transition = self.transition.lock().await;
        if self.inner().epoch != epoch {
            // `close` ran meanwhile and already reported the connection Disconnected; a transport
            // that did start is dropped here.
            return OpenStatus::Cancelled.into();
        }
        self.apply_start_result(result)
    }

    /// The start under the retry policy, abandoned (`Cancelled`) when the caller of Open stops
    /// waiting.
    async fn start_for_caller(
        &self,
        stop: &CancellationToken,
        caller: &CancellationToken,
    ) -> Result<Established, StartError> {
        tokio::select! {
            biased;
            result = self.start_with_retry(stop) => result,
            _ = caller.cancelled() => Err(StartError::Cancelled),
        }
    }

    /// Under the transition lock: enters `Connecting` and returns the epoch and stop token of the
    /// start, or the status the open answers at once (disposed, caller gone, already connected). An
    /// Open while `Reconnecting` abandons the library reconnect first.
    async fn enter_connecting(&self, caller: &CancellationToken) -> Result<(u64, CancellationToken), OpenStatus> {
        let _transition = self.transition.lock().await;
        if self.inner().disposed {
            // The manager closed and removed this connection while the open was queued.
            return Err(OpenStatus::Cancelled);
        }
        if caller.is_cancelled() {
            // Nobody waits for this open any more: leave the connection as it is.
            return Err(OpenStatus::Cancelled);
        }

        match self.state() {
            HubState::Connected => return Err(OpenStatus::Success),
            HubState::Reconnecting => self.abandon_reconnect(),
            HubState::Disconnected => {}
            HubState::Connecting => {
                // Only a serialized Open enters Connecting, and it leaves it before it releases
                // the serialization (also when its caller gave up).
                panic!("relay: a serialized Open found the hub connection Connecting")
            }
        }

        let (epoch, stop) = {
            let mut inner = self.inner();
            inner.state = HubState::Connecting;
            (inner.epoch, inner.stop.clone())
        };
        self.emit_state(HubState::Connecting);
        Ok((epoch, stop))
    }

    /// Under the transition lock, in the start's epoch: installs the established session, or
    /// reports the failed start `Disconnected` and maps it to an `OpenStatus` with the start's error
    /// as its cause. A `Cancelled` start here is the caller's: a `close` moves the epoch on and
    /// reports the abandoned open itself.
    fn apply_start_result(self: &Arc<Self>, result: Result<Established, StartError>) -> Opened {
        let env = self.params.env;
        match result {
            Ok(established) => {
                self.install_session(established);
                OpenStatus::Success.into()
            }
            Err(error) => {
                let status = error.open_status(self.shared.internet_available());
                if error == StartError::Cancelled {
                    tracing::info!(env = %env, "hub connection open abandoned; its caller stopped waiting");
                }
                self.inner().state = HubState::Disconnected;
                self.emit_state(HubState::Disconnected);
                Opened { status, cause: (error != StartError::Cancelled).then(|| error.to_string()) }
            }
        }
    }

    /// [`HubConnection::close`] for good: the connection was removed from the connection map and
    /// any later `open` on it answers `Cancelled`.
    pub(crate) async fn dispose(&self) {
        self.inner().disposed = true;
        self.close().await;
    }

    /// Stops the connection when it is not `Disconnected`: cancels an in-flight start or reconnect,
    /// closes the WebSocket, fails pending invocations and applies the `Closed` reaction.
    pub(crate) async fn close(&self) {
        {
            let mut inner = self.inner();
            inner.stop.cancel();
            inner.stop = CancellationToken::new();
        }

        let _transition = self.transition.lock().await;
        let (state, session, reconnect) = {
            let mut inner = self.inner();
            let state = inner.state;
            inner.state = HubState::Disconnected;
            inner.epoch += 1;
            (state, inner.session.take(), inner.reconnect.take())
        };

        if let Some(token) = reconnect {
            token.cancel();
        }
        if let Some(session) = session {
            session.cancel.cancel();
            session.close_pending();
        }

        let env = self.params.env;
        match state {
            HubState::Disconnected => {}
            HubState::Connecting => {
                tracing::info!(env = %env, "hub connection open abandoned; the connection was closed");
                self.emit_state(HubState::Disconnected);
            }
            HubState::Connected | HubState::Reconnecting => {
                tracing::info!(env = %env, prior_state = ?state, "hub connection closed");
                self.closed_reaction();
            }
        }
    }

    /// Under the transition lock, while `Reconnecting`: stop the reconnect and report `Closed`.
    pub(super) fn abandon_reconnect(&self) {
        let token = {
            let mut inner = self.inner();
            inner.epoch += 1;
            inner.state = HubState::Disconnected;
            inner.reconnect.take()
        };

        if let Some(token) = token {
            token.cancel();
        }

        tracing::info!(env = %self.params.env, "hub reconnect abandoned; Open starts the connection anew");
        self.closed_reaction();
    }

    /// `Reconnecting`: remove all partners (`LocalHubDisconnect`), keep the resolved region.
    pub(super) fn reconnecting_reaction(&self) {
        self.presence.deactivate(PartnerDisconnectReason::LocalHubDisconnect, false);
        self.emit_state(HubState::Reconnecting);
    }

    /// `Closed`: remove all partners (`LocalHubDisconnect`), forget the resolved region and the
    /// `OnConnected` payload, report `Disconnected`. The caller logs why the connection closed.
    pub(super) fn closed_reaction(&self) {
        self.presence.deactivate(PartnerDisconnectReason::LocalHubDisconnect, true);
        self.emit_state(HubState::Disconnected);
    }

    /// Under the transition lock: makes `established` the live session and starts its task.
    pub(super) fn install_session(self: &Arc<Self>, established: Established) {
        let id = self.shared.new_session_id();
        let (tx, rx) = mpsc::unbounded_channel();

        let session = {
            let mut inner = self.inner();
            let session = Arc::new(Session::new(id, tx, inner.stop.child_token()));
            inner.state = HubState::Connected;
            inner.session = Some(session.clone());
            inner.reconnect = None;
            session
        };

        self.presence.activate(id);
        self.emit_state(HubState::Connected);
        tracing::info!(env = %self.params.env, session = id, "hub connection established");
        tokio::spawn(run_session(self.clone(), session, established, rx));
    }

    /// The session task's end: reconnect or close, unless the session was already replaced.
    pub(super) async fn on_session_end(self: &Arc<Self>, session: &Arc<Session>, end: SessionEnd) {
        let _transition = self.transition.lock().await;
        let reconnect = end.reconnects();
        let epoch = {
            let mut inner = self.inner();
            if inner.session.as_ref().map(|s| s.id) != Some(session.id) {
                return;
            }
            inner.session = None;
            inner.epoch += 1;
            inner.state = if reconnect { HubState::Reconnecting } else { HubState::Disconnected };
            inner.epoch
        };

        session.close_pending();
        let env = self.params.env;
        if reconnect {
            tracing::warn!(env = %env, session = session.id, reason = ?end, "hub connection lost; reconnecting");
            self.reconnecting_reaction();

            let token = {
                let mut inner = self.inner();
                let token = inner.stop.child_token();
                inner.reconnect = Some(token.clone());
                token
            };
            tokio::spawn(self.clone().reconnect_loop(epoch, token));
        } else {
            tracing::warn!(env = %env, session = session.id, reason = ?end, "hub connection closed by the hub");
            self.closed_reaction();
        }
    }

    /// Automatic reconnect: delays by the number of attempts already made (`0, 2, 10, 30 s, 1, 3,
    /// 5 min`, then `Closed`) — [`reconnect_delay`]; an attempt that failed with HTTP 429 is followed
    /// by a freshly drawn `15 s + rand[0, 30) s` whatever the index (and still counts toward it). Each
    /// attempt is one start that asks the token provider and re-sends the connection headers; a start
    /// that failed with HTTP 401 invalidates the cached services token and is retried at once, so the
    /// retry's token request auto-refreshes and a revoked token heals in the same slot.
    pub(super) async fn reconnect_loop(self: Arc<Self>, epoch: u64, cancel: CancellationToken) {
        let env = self.params.env;
        let mut previous_attempts = 0usize;
        let mut last_was_429 = false;

        while let Some(delay) = reconnect_delay(&self.shared.config.relay, previous_attempts, last_was_429) {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }

            let result = self.start_once(false, &cancel).await;
            match result {
                Ok(established) => {
                    let _transition = self.transition.lock().await;
                    if self.inner().epoch != epoch || cancel.is_cancelled() {
                        return;
                    }
                    self.install_session(established);
                    return;
                }
                Err(StartError::Cancelled) => return,
                Err(error) => {
                    tracing::warn!(env = %env, %error, attempt = previous_attempts + 1, "hub reconnect attempt failed");
                    last_was_429 = error.is_429();
                    previous_attempts += 1;

                    // A 401 usually means the cached services token is invalid: drop it so the retry's
                    // token request auto-refreshes, instead of pacing the heal over the schedule.
                    if error.is_401() {
                        self.shared.tokens.invalidate_services_token(env).await;
                        match self.start_once(false, &cancel).await {
                            Ok(established) => {
                                let _transition = self.transition.lock().await;
                                if self.inner().epoch != epoch || cancel.is_cancelled() {
                                    return;
                                }
                                self.install_session(established);
                                return;
                            }
                            Err(StartError::Cancelled) => return,
                            Err(refreshed_error) => {
                                tracing::warn!(env = %env, %refreshed_error, attempt = previous_attempts + 1, "hub reconnect failed again after a forced token refresh");
                                last_was_429 = refreshed_error.is_429();
                                previous_attempts += 1;
                            }
                        }
                    }
                }
            }
        }

        let _transition = self.transition.lock().await;
        {
            let mut inner = self.inner();
            if inner.epoch != epoch || cancel.is_cancelled() {
                return;
            }
            inner.state = HubState::Disconnected;
            inner.reconnect = None;
            inner.epoch += 1;
        }

        tracing::warn!(env = %env, attempt_count = previous_attempts, "hub connection closed; the reconnect schedule is exhausted");
        self.closed_reaction();
    }
}
