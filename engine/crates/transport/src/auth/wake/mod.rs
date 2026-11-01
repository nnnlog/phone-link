//! The wake sequence over `/Dispatcher/Wake`.
//!
//! It covers de-duplication per target, the `SendConnectedAsync` flush, the `OnConnected` region
//! wait, the Dispatcher provider (priority normal → high, `ttl`, the `data` dictionary with the
//! `DCG-CryptoWakeJwt` signed by the trust key), the partner wait, the cancel wake, the
//! `DeviceUnreachable` retries, and the circuit breaker.
//!
//! [`WakeService`]
//! implements [`Waker`] over a [`WakeHub`]. The TTL is the caller's: the 20 s ↔ 45 min alternation
//! belongs to the per-partner connection (`crate::platform::session`), and a send-path wake uses
//! the default TTL.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use p384::ecdsa::SigningKey;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use phonelink_core::config::WakeConfig;
use phonelink_core::seams::{WakeHub, Waker};
use phonelink_core::types::{DcgClient, Environment, HubSendResult, TraceContext, WakeOptions, WakeStatus, Woken};

use super::crypto;
use super::dcg_rest::{PolicyFailure, RestError, ServiceRequestStatus, TransportCause, WakeBody};
use super::{AuthError, AuthService};
use phonelink_core::codec::json::OrderedStringMap;
use phonelink_core::sync::MutexExt;

/// `collapseKey` of every wake.
pub const COLLAPSE_KEY: &str = "YPPWake";
/// `Scope` claim of the wake JWT.
pub const WAKE_SCOPE: &str = "wake";
pub const PRIORITY_NORMAL: &str = "normal";
pub const PRIORITY_HIGH: &str = "high";

fn flag(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

/// `(dcgClientId, environment)`, the id folded to lower case.
type TargetKey = (String, Environment);

fn key(target: &DcgClient) -> TargetKey {
    (target.id.to_ascii_lowercase(), target.env)
}

impl AuthService {
    /// The signer of the wake JWT for `target`: the relationship's `self_client_id`, the trust key whose
    /// CN it names, and the PC's own `dcgClientId`.
    fn wake_signer(&self, target: &DcgClient) -> Result<(String, SigningKey, String), AuthError> {
        let now = self.now();
        let mut st = self.state.lock_or_panic();
        self.prune_trust_locked(&mut st);

        let index = st
            .trust
            .trusted_index_by_dcg_client(target, now, &self.policy)
            .ok_or_else(|| AuthError::NotProvisioned("no crypto-trust relationship with the target".to_owned()))?;
        let self_client_id = st.trust.get(index).self_client_id.clone();

        let slot = st.env(target.env);
        let own_id = slot
            .identity
            .as_ref()
            .map(|i| i.device_id.clone())
            .ok_or_else(|| AuthError::NotProvisioned(format!("no identity in {}", target.env)))?;
        let pair = slot
            .trust_keys
            .as_ref()
            .filter(|p| p.cn().eq_ignore_ascii_case(&self_client_id))
            .ok_or_else(|| AuthError::NotProvisioned("no trust key for this PC".to_owned()))?;
        Ok((self_client_id, pair.key.clone(), own_id))
    }

    /// The `WakeBody` of one Dispatcher attempt: `data` keys in the order Phone Link on Windows
    /// sends, then `DCG-CryptoWakeJwt` over the others serialized as one compact JSON object
    /// string.
    pub fn wake_body(
        &self,
        target: &DcgClient,
        options: WakeOptions,
        ttl: Duration,
        priority: &str,
        hub_region: Option<&str>,
        cancel_wake: bool,
    ) -> Result<WakeBody, AuthError> {
        let (self_client_id, key, own_id) = self.wake_signer(target)?;

        let mut data = OrderedStringMap::default();
        data.push("DCG-Environment", target.env.name());
        data.push("IgnoreDeviceDisabledStatus", flag(options.ignore_device_disabled));
        // `keep_session_on_peer_disconnect` forces it on every wake.
        let force_new_session = self.config.relay.keep_session_on_peer_disconnect;
        data.push("DCG-RequestNewSession", flag(options.request_new_session || force_new_session));
        if let Some(region) = hub_region {
            data.push("DCG-HubRegion", region);
        }
        if cancel_wake {
            data.push("DCG-NoOp", "True");
        }

        let jwt = crypto::data_jwt(&key, &self_client_id, &data.to_json(), &own_id, WAKE_SCOPE, self.now())?;
        data.push("DCG-CryptoWakeJwt", jwt);

        Ok(WakeBody {
            device_id: target.id.clone(),
            ttl: ttl.as_secs(),
            priority: priority.to_string(),
            collapse_key: COLLAPSE_KEY.to_string(),
            data,
        })
    }
}

impl AuthService {
    /// A data JWT for `target` with the wake JWT's signer and claims: `iss` = the relationship's
    /// self client id, `Data` = `data` as one compact JSON object in its order, `SourceId` = this
    /// PC's `dcgClientId` in `target`'s environment, `Scope` = `scope`.
    pub fn signed_data_jwt(
        &self,
        target: &DcgClient,
        data: &OrderedStringMap,
        scope: &str,
    ) -> Result<String, AuthError> {
        let (self_client_id, key, own_id) = self.wake_signer(target)?;
        Ok(crypto::data_jwt(&key, &self_client_id, &data.to_json(), &own_id, scope, self.now())?)
    }
}

/// The wake status of a failed `/Dispatcher/Wake` (the wake effects of the DCG status mapping).
///
/// A transport failure whose cause is the network is `InternetError` and a timed-out one
/// `Cancelled`; every status the mapping names no other result for — a transport failure of no
/// class among them — is `Failure`.
pub fn dispatcher_status(e: &RestError) -> WakeStatus {
    use ServiceRequestStatus as S;
    match e.status() {
        Some(S::InternetError) => WakeStatus::InternetError,
        Some(S::Cancelled) => WakeStatus::Cancelled,
        Some(S::NotFoundError) => WakeStatus::DeviceNotFoundError,
        Some(S::UnsupportedMarketError) => WakeStatus::IncompatibleRegionError,
        Some(s) if s.is_unauthorized() => WakeStatus::Unauthorized,
        Some(S::AccessTokenRetrievalError) => WakeStatus::Unauthorized,
        Some(S::LtwDisabled) => WakeStatus::LtwDisabled,
        Some(S::MsaTokenUserInteractionRequired) => WakeStatus::MsaTokenUserInteractionRequired,
        _ => WakeStatus::Failure,
    }
}

/// The wake status of a services-token failure (the token is part of the Dispatcher call). An MSA
/// token that needs the user is `MsaTokenUserInteractionRequired`, no connectivity `InternetError`;
/// a token the DCG auth layer could not produce — DCG answered `GenerateNonce` / `SignIn` with an
/// HTTP error or not in time, its circuit was open, the identity is gone or was rejected, the device
/// was unlinked — is
/// `Unauthorized`; any other failure (a reply that broke off or broke the contract, the MSA token
/// provider, the key store, crypto) is `Failure`.
fn token_status(e: &AuthError) -> WakeStatus {
    match e {
        AuthError::UserInteractionRequired(_) => WakeStatus::MsaTokenUserInteractionRequired,
        e if e.is_internet_unavailable() => WakeStatus::InternetError,
        AuthError::Rest(rest) => match **rest {
            RestError::Status(_)
            | RestError::Transport { cause: TransportCause::TimedOut, .. }
            | RestError::Policy { failure: PolicyFailure::BrokenCircuit { .. }, .. } => WakeStatus::Unauthorized,
            _ => WakeStatus::Failure,
        },
        AuthError::Unauthorized(_)
        | AuthError::InvalidIdentity
        | AuthError::UnlinkedDevice
        | AuthError::NotProvisioned(_) => WakeStatus::Unauthorized,
        _ => WakeStatus::Failure,
    }
}

/// What made a wake fail, beyond its status (the wake's end line carries it).
#[derive(Debug, thiserror::Error)]
enum WakeCause {
    #[error("cannot sign a wake JWT for the target: {0}")]
    Signer(#[source] AuthError),
    #[error("SendConnectedAsync before the wake answered {0:?}")]
    Flush(HubSendResult),
    #[error("no OnConnected payload within the wait")]
    NoRegion,
    #[error("no services token: {0}")]
    Token(#[source] AuthError),
    #[error("building the wake: {0}")]
    Body(#[source] AuthError),
    #[error("Dispatcher/Wake: {0}")]
    Dispatcher(#[source] RestError),
}

/// How a wake ended: its status, and what made it fail when something did beyond the phone not
/// connecting.
type WakeEnd = (WakeStatus, Option<WakeCause>);

/// The wake's end line, logged once per wake by the task that runs it, for a wake that brought the
/// partner: a failed wake, with its cause, goes to the connection attempt, which logs it.
fn log_wake_end(target: &DcgClient, (status, _): &WakeEnd) {
    if *status == WakeStatus::Success {
        tracing::info!(%target, "partner woken");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BreakerState {
    Closed,
    Open {
        until: Instant,
    },
    /// One trial is in flight.
    HalfOpen,
}

/// The circuit breaker of one `(target, wake type)`: opens when `DeviceUnreachable` results reach
/// 100 % of the sampling window with at least the minimum throughput, stays open for the break,
/// then half-opens for one trial.
#[derive(Debug)]
struct Breaker {
    state: BreakerState,
    results: VecDeque<(Instant, bool)>,
}

impl Breaker {
    fn new() -> Self {
        Self { state: BreakerState::Closed, results: VecDeque::new() }
    }

    fn allow(&mut self, now: Instant) -> bool {
        match self.state {
            BreakerState::Closed => true,
            BreakerState::Open { until } if now >= until => {
                self.state = BreakerState::HalfOpen;
                true
            }
            BreakerState::Open { .. } | BreakerState::HalfOpen => false,
        }
    }

    /// Records one attempt's result; `true` when it opened the circuit.
    fn record(&mut self, now: Instant, failure: bool, cfg: &WakeConfig) -> bool {
        match self.state {
            BreakerState::HalfOpen => {
                self.results.clear();
                self.state =
                    if failure { BreakerState::Open { until: now + cfg.breaker_break } } else { BreakerState::Closed };
                failure
            }
            BreakerState::Closed => {
                self.results.push_back((now, failure));
                while self.results.front().is_some_and(|(t, _)| now.duration_since(*t) > cfg.breaker_sampling_window) {
                    self.results.pop_front();
                }

                let open = self.results.len() >= cfg.breaker_minimum_throughput as usize
                    && self.results.iter().all(|(_, f)| *f);
                if open {
                    self.results.clear();
                    self.state = BreakerState::Open { until: now + cfg.breaker_break };
                }
                open
            }
            BreakerState::Open { .. } => false,
        }
    }
}

#[derive(Default)]
struct WakeInner {
    breakers: HashMap<TargetKey, Breaker>,
    in_flight: HashMap<TargetKey, watch::Receiver<Option<Woken>>>,
}

/// The engine's [`Waker`]: the Dispatcher provider only (the Bluetooth provider is not implemented).
pub struct WakeService {
    auth: Arc<AuthService>,
    hub: Arc<dyn WakeHub>,
    inner: Mutex<WakeInner>,
    weak: Weak<WakeService>,
}

impl WakeService {
    pub fn new(auth: Arc<AuthService>, hub: Arc<dyn WakeHub>) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self { auth, hub, inner: Mutex::new(WakeInner::default()), weak: weak.clone() })
    }

    fn cfg(&self) -> &WakeConfig {
        &self.auth.config.auth.wake
    }

    fn online(&self) -> bool {
        self.auth.edges.reachability.is_internet_available()
    }

    /// The sequence for one target after de-duplication — flush, region wait, Dispatcher attempts
    /// with their partner waits and retries — every Dispatcher attempt with `ttl`.
    async fn run(
        &self,
        target: &DcgClient,
        options: WakeOptions,
        ttl: Duration,
        cancel: &CancellationToken,
    ) -> WakeEnd {
        if let Err(e) = self.auth.wake_signer(target) {
            return (WakeStatus::Failure, Some(WakeCause::Signer(e)));
        }

        let region = match self.flush_and_region(target, cancel).await {
            Ok(region) => region,
            Err(end) => return end,
        };

        self.dispatcher_attempts(target, options, ttl, region.as_deref(), cancel).await
    }

    /// Flushes the hub's connected message, then waits for the hub region; the region (`None` when
    /// absent or empty), or the status the wake ends with.
    async fn flush_and_region(
        &self,
        target: &DcgClient,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, WakeEnd> {
        // Flush.
        let flush = tokio::select! {
            r = self.hub.flush_connected(target) => r,
            _ = cancel.cancelled() => return Err((WakeStatus::Cancelled, None)),
        };

        if flush != HubSendResult::Success {
            let status = if flush == HubSendResult::InternetError || !self.online() {
                WakeStatus::TransportInternetError
            } else {
                WakeStatus::TransportNetworkError
            };
            return Err((status, Some(WakeCause::Flush(flush))));
        }

        // Region.
        let region = tokio::select! {
            r = self.hub.hub_region(target.env, self.cfg().on_connected_wait) => r,
            _ = cancel.cancelled() => return Err((WakeStatus::Cancelled, None)),
        };

        match region {
            Ok(r) => Ok(r.filter(|r| !r.is_empty())),
            Err(_) => Err((WakeStatus::TransportNetworkError, Some(WakeCause::NoRegion))),
        }
    }

    /// The Dispatcher attempts, retried while the partner stays unreachable, each gated by the
    /// cancellation and the circuit breaker; the last attempt's status.
    async fn dispatcher_attempts(
        &self,
        target: &DcgClient,
        options: WakeOptions,
        ttl: Duration,
        region: Option<&str>,
        cancel: &CancellationToken,
    ) -> WakeEnd {
        // Dispatcher attempts, retried while the partner stays unreachable.
        let retries = self.cfg().retries;
        let mut end = (WakeStatus::Failure, None);

        for attempt in 0..=retries {
            if cancel.is_cancelled() {
                end = (WakeStatus::Cancelled, None);
                break;
            }
            if !self.breaker_allows(target) {
                end = (WakeStatus::WakeBrokenCircuitError, None);
                break;
            }

            let priority = if attempt == 0 { PRIORITY_NORMAL } else { PRIORITY_HIGH };
            end = self.dispatcher_attempt(target, options, ttl, priority, region, cancel).await;
            let failure = end.0 == WakeStatus::DeviceUnreachable && self.online();
            self.breaker_record(target, failure);

            if end.0 != WakeStatus::DeviceUnreachable {
                break;
            }
            if attempt < retries {
                tracing::warn!(%target, attempt, "the partner did not connect after the wake; waking it again");
            }
        }
        end
    }

    fn breaker_allows(&self, target: &DcgClient) -> bool {
        let mut inner = self.inner.lock_or_panic();
        inner.breakers.entry(key(target)).or_insert_with(Breaker::new).allow(Instant::now())
    }

    fn breaker_record(&self, target: &DcgClient, failure: bool) {
        let cfg = self.cfg().clone();
        let mut inner = self.inner.lock_or_panic();
        if inner.breakers.entry(key(target)).or_insert_with(Breaker::new).record(Instant::now(), failure, &cfg) {
            tracing::info!(
                %target,
                break_secs = cfg.breaker_break.as_secs(),
                "wake circuit opened; the target's wakes are refused for the break"
            );
        }
    }

    async fn dispatcher_attempt(
        &self,
        target: &DcgClient,
        options: WakeOptions,
        ttl: Duration,
        priority: &str,
        region: Option<&str>,
        cancel: &CancellationToken,
    ) -> WakeEnd {
        let services = match self.auth.services_token_for(target.env, false).await {
            Ok(t) => t,
            Err(e) => return (token_status(&e), Some(WakeCause::Token(e))),
        };

        if cancel.is_cancelled() {
            // Nothing was sent yet, so there is no wake to cancel.
            return (WakeStatus::Cancelled, None);
        }

        if let Err(end) = self.send_wake(target, options, ttl, priority, region, &services).await {
            return end;
        }

        let wait = ttl.max(self.cfg().minimum_partner_wait);
        tokio::select! {
            connected = self.hub.wait_partner_connected(target, wait) => {
                (if connected { WakeStatus::Success } else { WakeStatus::DeviceUnreachable }, None)
            }
            _ = cancel.cancelled() => {
                self.send_cancel_wake(target, options, region, &services).await;
                (WakeStatus::Cancelled, None)
            }
        }
    }

    /// Builds and sends one `Dispatcher/Wake`; the status the attempt ends with when either fails.
    async fn send_wake(
        &self,
        target: &DcgClient,
        options: WakeOptions,
        ttl: Duration,
        priority: &str,
        region: Option<&str>,
        services: &str,
    ) -> Result<(), WakeEnd> {
        let body = self
            .auth
            .wake_body(target, options, ttl, priority, region, false)
            .map_err(|e| (WakeStatus::Failure, Some(WakeCause::Body(e))))?;

        let trace = TraceContext::new_root(Vec::new());
        self.auth
            .rest
            .wake(target.env, services, &body, &trace)
            .await
            .map_err(|e| (dispatcher_status(&e), Some(WakeCause::Dispatcher(e))))
    }

    /// The cancel wake: `ttl = 1 s`, `priority = "high"`, `DCG-NoOp = "True"`.
    async fn send_cancel_wake(&self, target: &DcgClient, options: WakeOptions, region: Option<&str>, services: &str) {
        let body = match self.auth.wake_body(target, options, self.cfg().ttl_cancel, PRIORITY_HIGH, region, true) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(%target, error = %e, "cancel wake not built; the phone may still wake");
                return;
            }
        };

        let trace = TraceContext::new_root(Vec::new());
        if let Err(e) = self.auth.rest.wake(target.env, services, &body, &trace).await {
            tracing::warn!(%target, error = %e, "cancel wake failed; the phone may still wake");
        }
    }
}

async fn await_result(rx: &mut watch::Receiver<Option<Woken>>, cancel: Option<&CancellationToken>) -> Option<Woken> {
    loop {
        if let Some(woken) = rx.borrow_and_update().clone() {
            return Some(woken);
        }

        let changed = match cancel {
            Some(c) => tokio::select! {
                r = rx.changed() => r,
                _ = c.cancelled() => return Some(WakeStatus::Cancelled.into()),
            },
            None => rx.changed().await,
        };
        if changed.is_err() {
            return None;
        }
    }
}

#[async_trait]
impl Waker for WakeService {
    /// De-duplicates: a wake in flight for the same `(dcgClientId, environment)` is awaited
    /// (until the caller's own cancellation) instead of repeated — with the options and TTL of the
    /// wake in flight.
    async fn wake(&self, target: &DcgClient, options: WakeOptions, ttl: Duration, cancel: CancellationToken) -> Woken {
        let k = key(target);
        let Some(this) = self.weak.upgrade() else { return WakeStatus::Failure.into() };

        let (mut rx, started) = {
            let mut inner = self.inner.lock_or_panic();
            match inner.in_flight.get(&k) {
                Some(rx) => (rx.clone(), false),
                None => {
                    let (tx, rx) = watch::channel(None);
                    inner.in_flight.insert(k.clone(), rx.clone());
                    let target = target.clone();
                    let token = cancel.clone();
                    tokio::spawn(async move {
                        let end = this.run(&target, options, ttl, &token).await;
                        log_wake_end(&target, &end);
                        this.inner.lock_or_panic().in_flight.remove(&key(&target));
                        let _ = tx.send(Some(Woken { status: end.0, cause: end.1.map(|c| c.to_string()) }));
                    });
                    (rx, true)
                }
            }
        };

        let own_cancel = if started { None } else { Some(&cancel) };
        match await_result(&mut rx, own_cancel).await {
            Some(status) => status,
            None => {
                // The wake task ended without a result (it panicked): forget it.
                let mut inner = self.inner.lock_or_panic();
                if inner.in_flight.get(&k).is_some_and(|r| r.same_channel(&rx)) {
                    inner.in_flight.remove(&k);
                }
                Woken { status: WakeStatus::Failure, cause: Some("the wake task panicked".into()) }
            }
        }
    }

    fn reset_breakers(&self) {
        self.inner.lock_or_panic().breakers.clear();
    }
}

#[cfg(test)]
mod tests;
