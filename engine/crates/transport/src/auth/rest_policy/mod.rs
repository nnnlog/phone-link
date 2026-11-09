//! The resilience policy every DCG REST call goes through, outermost layer first:
//!
//! 1. a concurrency limit per service client;
//! 2. the total timeout of the call;
//! 3. retries of a transient attempt (a reply with status ≥ 500, 408 or 429, no reply, or the attempt
//!    timeout) after a jittered exponential back-off, or after the reply's `Retry-After`;
//! 4. a circuit breaker per service client, one sample per attempt;
//! 5. the attempt timeout;
//! 6. inside each attempt, resends of a 503, 504 or 429 reply (`Retry-Attempt: <n>`).
//!
//! Every attempt and resend sends the same request. The reply body is read by the caller after the
//! policy returned, so neither timeout covers it.
//!
//! Differs from Phone Link on Windows: a resend's `Retry-After` date is measured against UTC
//! (Windows measures it against the PC's local clock), and an HTTP-date in another form than
//! RFC 1123 counts as no `Retry-After`.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use tokio::time::Instant;

use phonelink_core::config::DcgRestConfig;
use phonelink_core::sync::MutexExt;
use phonelink_core::time::{days_from_civil, days_in_month};

/// The header a resend carries: the resend's number, from 1.
pub const RETRY_ATTEMPT: &str = "Retry-Attempt";

/// The service client a DCG REST call belongs to; each keeps its own concurrency count and circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceClient {
    /// `UserServiceEnvironment`.
    EnvironmentDiscovery,
    /// `GenerateNonce`, `CreateIdentity`, `SignIn`, `RotateKeys`.
    Auth,
    /// `EnrollDevice`, `SetDeviceInfo`, `GetDeviceInfoList`, `UnlinkDevice`, `DeleteDeviceInfo`.
    DeviceManagement,
    /// `GetAccountCert`.
    DeviceAuthProxy,
    /// `CloudRelay/Relay`.
    CloudRelay,
    /// `Dispatcher/Wake`.
    Dispatcher,
    /// The four `DevicePairingProxy/PairingSessions` calls.
    DevicePairingProxy,
}

impl ServiceClient {
    pub const ALL: [ServiceClient; 7] = [
        ServiceClient::EnvironmentDiscovery,
        ServiceClient::Auth,
        ServiceClient::DeviceManagement,
        ServiceClient::DeviceAuthProxy,
        ServiceClient::CloudRelay,
        ServiceClient::Dispatcher,
        ServiceClient::DevicePairingProxy,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// One step of a call, for the tap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A send: its attempt (from 0), its resend number (`Retry-Attempt`, `None` for the attempt's
    /// first send), the wait before it, and the reply's status or the error.
    Sent { attempt: u32, resend: Option<u32>, waited: Duration, outcome: Result<u16, String> },
    /// The attempt timeout ended the attempt.
    AttemptTimedOut { attempt: u32 },
    /// The circuit was open: the attempt was not sent.
    CircuitOpen { attempt: u32 },
}

impl Step {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Step::Sent { attempt, resend, waited, outcome } => serde_json::json!({
                "attempt": attempt,
                "resend": resend,
                "waited_ms": waited.as_millis() as u64,
                "status": outcome.as_ref().ok(),
                "error": outcome.as_ref().err(),
            }),
            Step::AttemptTimedOut { attempt } => serde_json::json!({ "attempt": attempt, "timed_out": true }),
            Step::CircuitOpen { attempt } => serde_json::json!({ "attempt": attempt, "circuit_open": true }),
        }
    }
}

/// Which timeout ended a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Total(Duration),
    /// The last attempt's timeout.
    Attempt(Duration),
}

/// How the policy ended a call without a reply.
#[derive(Debug)]
pub enum PolicyError {
    /// The last attempt got no reply.
    NoReply(reqwest::Error),
    TimedOut(Bound),
    /// The service client's circuit is open; `retry_after` is the rest of the break.
    BrokenCircuit {
        retry_after: Duration,
    },
    /// The service client already has `limit` calls in flight; nothing was sent.
    ConcurrencyLimit {
        limit: usize,
    },
    /// A 503, 504 or 429 still answered after every resend of an attempt.
    ResendsExhausted {
        status: u16,
        sends: u32,
    },
    /// A 503, 504 or 429 whose `Retry-After` is a negative number of seconds.
    NegativeRetryAfter {
        value: String,
    },
}

/// The outcome of one attempt.
enum Attempt {
    Reply(reqwest::Response),
    NoReply(reqwest::Error),
    TimedOut(Duration),
    CircuitOpen(Duration),
    ResendsExhausted { status: u16, sends: u32 },
    NegativeRetryAfter(String),
}

impl Attempt {
    /// A reply with status ≥ 500, 408 or 429, a request that got no reply, or the attempt timeout.
    /// The HTTP client's own timeout, a request that could not be built and a redirect failure are
    /// not.
    fn is_transient(&self) -> bool {
        match self {
            Attempt::Reply(r) => is_transient_status(r.status().as_u16()),
            Attempt::NoReply(e) => !(e.is_builder() || e.is_timeout() || e.is_redirect()),
            Attempt::TimedOut(_) => true,
            Attempt::CircuitOpen(_) | Attempt::ResendsExhausted { .. } | Attempt::NegativeRetryAfter(_) => false,
        }
    }

    fn into_result(self) -> Result<reqwest::Response, PolicyError> {
        match self {
            Attempt::Reply(r) => Ok(r),
            Attempt::NoReply(e) => Err(PolicyError::NoReply(e)),
            Attempt::TimedOut(d) => Err(PolicyError::TimedOut(Bound::Attempt(d))),
            Attempt::CircuitOpen(retry_after) => Err(PolicyError::BrokenCircuit { retry_after }),
            Attempt::ResendsExhausted { status, sends } => Err(PolicyError::ResendsExhausted { status, sends }),
            Attempt::NegativeRetryAfter(value) => Err(PolicyError::NegativeRetryAfter { value }),
        }
    }
}

/// The statuses whose reply counts as a transient attempt: ≥ 500, 408, 429.
pub fn is_transient_status(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// The statuses resent inside an attempt: 503, 504, 429.
pub fn is_resent_status(status: u16) -> bool {
    matches!(status, 503 | 504 | 429)
}

// ------------------------------------------------------------------------------------------------
// Delays
// ------------------------------------------------------------------------------------------------

const TICK_NANOS: u128 = 100;
const MAX_TICKS: f64 = i64::MAX as f64 - 1000.0;

/// The back-off before retry `attempt + 1` (`attempt` from 0): decorrelated jitter over `base`.
///
/// `prev` carries the curve value between the retries of one call (0 before the first); `r` is
/// uniform in [0, 1). The waits before retries 1, 2, 3 add up to [0, 1.38) · `base`,
/// [1.38, 2.84) · `base`, [2.84, 5.70) · `base`.
pub fn backoff_delay(attempt: u32, base: Duration, prev: &mut f64, r: f64) -> Duration {
    assert!((0.0..1.0).contains(&r), "jitter {r} outside [0, 1)");
    if base.is_zero() {
        return Duration::ZERO;
    }

    let t = f64::from(attempt) + r;
    let next = 2f64.powf(t) * (4.0 * t).sqrt().tanh();

    if next.is_infinite() {
        *prev = next;
        return ticks(MAX_TICKS);
    }

    let rise = next - *prev;
    *prev = next;
    let base_ticks = (base.as_nanos() / TICK_NANOS) as f64;
    ticks((rise * (2925.0 / 4095.0) * base_ticks).min(MAX_TICKS))
}

fn ticks(t: f64) -> Duration {
    assert!(t >= 0.0, "negative back-off {t}");
    Duration::from_nanos(((t as i64) as u64).saturating_mul(TICK_NANOS as u64))
}

/// The retry's `Retry-After`: one header value that is delta-seconds (decimal digits within the
/// 32-bit range) or an RFC 1123 date, the date minus `now` (0 when past).
///
/// Anything else — no header, two of them, another form — is no `Retry-After`.
pub fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let mut values = headers.get_all(RETRY_AFTER).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }

    let text = value.to_str().ok()?.trim_matches([' ', '\t']);
    if text.bytes().next().is_some_and(|b| b.is_ascii_digit()) {
        if !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let seconds: i32 = text.parse().ok()?;
        return Some(Duration::from_secs(seconds as u64));
    }

    let date = parse_rfc1123(text)?;
    Some(date.duration_since(now).unwrap_or(Duration::ZERO))
}

/// The wait before resend `k` (from 0) of a 503, 504 or 429 reply.
///
/// The first `Retry-After` value as whole seconds when it is a 32-bit decimal integer; else, when
/// it is an RFC 1123 date, the seconds component (−59…59, truncated) of the date minus `now` when
/// that is positive, else `delay · 2^k`; else, for any other `Retry-After`, `delay`; without one,
/// `delay · 2^k`; at most `max`.
///
/// A negative integer is an error carrying the value.
pub fn resend_wait(
    headers: &HeaderMap,
    k: u32,
    delay: Duration,
    max: Duration,
    now: SystemTime,
) -> Result<Duration, String> {
    let exponential = delay.saturating_mul(2u32.saturating_pow(k));
    let wait = match headers.get(RETRY_AFTER) {
        None => exponential,
        Some(value) => match value.to_str().ok() {
            Some(text) => match parse_integer(text) {
                Some(seconds) if seconds < 0 => return Err(text.to_string()),
                Some(seconds) => Duration::from_secs(seconds as u64),
                None => match parse_rfc1123(text) {
                    Some(date) => match seconds_component(date, now) {
                        s if s > 0 => Duration::from_secs(s as u64),
                        _ => exponential,
                    },
                    None => delay,
                },
            },
            None => delay,
        },
    };
    Ok(wait.min(max))
}

/// A 32-bit signed decimal integer with an optional sign and surrounding white space.
fn parse_integer(text: &str) -> Option<i32> {
    let t = text.trim_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.strip_prefix('+').unwrap_or(t).parse().ok()
}

/// The seconds component of `date − now`, truncated toward zero in 100 ns ticks: −59…59.
fn seconds_component(date: SystemTime, now: SystemTime) -> i64 {
    let ticks = |t: SystemTime| -> i128 {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => (d.as_nanos() / TICK_NANOS) as i128,
            Err(e) => -((e.duration().as_nanos() / TICK_NANOS) as i128),
        }
    };

    let whole_seconds = (ticks(date) - ticks(now)) / 10_000_000;
    (whole_seconds % 60) as i64
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// `ddd, dd MMM yyyy HH:mm:ss GMT` (day and month names in any case, the day name matching the
/// date, `GMT` as written), read as UTC.
pub fn parse_rfc1123(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    if b.len() != 29 || &b[3..5] != b", " || b[7] != b' ' || b[11] != b' ' || b[16] != b' ' || &b[25..] != b" GMT" {
        return None;
    }
    if b[19] != b':' || b[22] != b':' {
        return None;
    }

    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let s = &text[range];
        s.bytes().all(|c| c.is_ascii_digit()).then(|| s.parse().ok()).flatten()
    };
    let name = |range: std::ops::Range<usize>, names: &[&str]| -> Option<usize> {
        names.iter().position(|n| n.eq_ignore_ascii_case(&text[range.clone()]))
    };

    let weekday = name(0..3, &DAYS)?;
    let day = num(5..7)?;
    let month = name(8..11, &MONTHS)? as i64 + 1;
    let year = num(12..16)?;
    let (hour, minute, second) = (num(17..19)?, num(20..22)?, num(23..25)?);
    if year < 1 || day < 1 || day > days_in_month(year, month) || hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    if (days + 4).rem_euclid(7) as usize != weekday {
        return None;
    }

    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }
}

// ------------------------------------------------------------------------------------------------
// The circuit breaker
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug)]
struct Bucket {
    started: Instant,
    successes: u32,
    failures: u32,
}

/// The sampled outcomes: 10 buckets over the sampling duration, or one bucket reset after it when
/// the duration is under 200 ms.
#[derive(Debug)]
enum Health {
    Rolling { buckets: VecDeque<Bucket>, sampling: Duration, bucket: Duration },
    Single { current: Bucket, sampling: Duration },
}

impl Health {
    fn new(sampling: Duration, now: Instant) -> Self {
        if sampling < Duration::from_millis(200) {
            Health::Single { current: Bucket { started: now, successes: 0, failures: 0 }, sampling }
        } else {
            Health::Rolling { buckets: VecDeque::new(), sampling, bucket: sampling / 10 }
        }
    }

    fn current(&mut self, now: Instant) -> &mut Bucket {
        match self {
            Health::Rolling { buckets, sampling, bucket } => {
                if buckets.back().is_none_or(|b| now.duration_since(b.started) >= *bucket) {
                    buckets.push_back(Bucket { started: now, successes: 0, failures: 0 });
                }
                while buckets.front().is_some_and(|b| now.duration_since(b.started) >= *sampling) {
                    buckets.pop_front();
                }
                buckets.back_mut().expect("the current bucket is younger than the sampling duration")
            }
            Health::Single { current, sampling } => {
                if now.duration_since(current.started) >= *sampling {
                    *current = Bucket { started: now, successes: 0, failures: 0 };
                }
                current
            }
        }
    }

    fn success(&mut self, now: Instant) {
        self.current(now).successes += 1;
    }

    fn failure(&mut self, now: Instant) {
        self.current(now).failures += 1;
    }

    /// `(samples, failure ratio)`.
    fn info(&mut self, now: Instant) -> (u32, f64) {
        self.current(now);
        let (s, f) = match self {
            Health::Rolling { buckets, .. } => {
                buckets.iter().fold((0, 0), |(s, f), b| (s + b.successes, f + b.failures))
            }
            Health::Single { current, .. } => (current.successes, current.failures),
        };

        let total = s + f;
        (total, if total == 0 { 0.0 } else { f64::from(f) / f64::from(total) })
    }

    fn reset(&mut self, now: Instant) {
        match self {
            Health::Rolling { buckets, .. } => buckets.clear(),
            Health::Single { current, .. } => *current = Bucket { started: now, successes: 0, failures: 0 },
        }
    }
}

#[derive(Debug)]
struct Breaker {
    state: CircuitState,
    /// End of the current break (open) or of the half-open trial's break.
    blocked_until: Option<Instant>,
    health: Health,
    failure_ratio: f64,
    minimum_throughput: u32,
    break_duration: Duration,
}

impl Breaker {
    fn new(cfg: &DcgRestConfig, now: Instant) -> Self {
        Self {
            state: CircuitState::Closed,
            blocked_until: None,
            health: Health::new(cfg.breaker_sampling, now),
            failure_ratio: cfg.breaker_failure_ratio,
            minimum_throughput: cfg.breaker_minimum_throughput,
            break_duration: cfg.breaker_break,
        }
    }

    fn until(&self, now: Instant) -> Instant {
        now.checked_add(self.break_duration).unwrap_or_else(|| now + Duration::from_secs(100 * 365 * 86_400))
    }

    /// May an attempt go out? An open circuit whose break is over lets this attempt through as the
    /// half-open trial; otherwise an open or half-open circuit refuses it with the rest of the break.
    fn before_attempt(&mut self, now: Instant) -> Result<(), Duration> {
        let mut trial = false;
        if self.state == CircuitState::Open && self.blocked_until.is_none_or(|u| now >= u) {
            self.blocked_until = Some(self.until(now));
            self.state = CircuitState::HalfOpen;
            trial = true;
        }

        match self.state {
            CircuitState::Closed => Ok(()),
            CircuitState::HalfOpen if trial => Ok(()),
            CircuitState::Open | CircuitState::HalfOpen => {
                Err(self.blocked_until.map_or(Duration::ZERO, |u| u.saturating_duration_since(now)))
            }
        }
    }

    fn on_outcome(&mut self, transient: bool, now: Instant) {
        if transient {
            let should_break = match self.state {
                CircuitState::Closed => {
                    self.health.failure(now);
                    let (samples, ratio) = self.health.info(now);
                    samples >= self.minimum_throughput && ratio >= self.failure_ratio
                }
                CircuitState::Open => {
                    self.health.failure(now);
                    false
                }
                CircuitState::HalfOpen => false,
            };

            if self.state == CircuitState::HalfOpen || (self.state == CircuitState::Closed && should_break) {
                self.blocked_until = Some(self.until(now));
                self.state = CircuitState::Open;
            }
        } else {
            self.health.success(now);

            if self.state == CircuitState::HalfOpen {
                self.state = CircuitState::Closed;
                self.blocked_until = None;
                self.health.reset(now);
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The policy
// ------------------------------------------------------------------------------------------------

struct Pipeline {
    in_flight: AtomicUsize,
    breaker: Mutex<Breaker>,
}

/// One call's place in its service client's concurrency count.
struct Permit<'a>(&'a AtomicUsize);

impl<'a> Permit<'a> {
    fn acquire(count: &'a AtomicUsize, limit: usize) -> Option<Self> {
        let mut current = count.load(Ordering::SeqCst);
        loop {
            if current >= limit {
                return None;
            }
            match count.compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => return Some(Permit(count)),
                Err(actual) => current = actual,
            }
        }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An attempt the circuit let through: its outcome is recorded once; an attempt that never ends —
/// the total timeout fired or the caller stopped waiting — counts as a success.
struct Sample<'a> {
    breaker: &'a Mutex<Breaker>,
    recorded: bool,
}

impl Sample<'_> {
    fn record(mut self, transient: bool) {
        self.recorded = true;
        self.breaker.lock_or_panic().on_outcome(transient, Instant::now());
    }
}

impl Drop for Sample<'_> {
    fn drop(&mut self) {
        if !self.recorded {
            self.breaker.lock_or_recover().on_outcome(false, Instant::now());
        }
    }
}

/// The policy of one control plane: its configuration and, per service client, the concurrency
/// count and the circuit.
pub struct RestPolicy {
    cfg: DcgRestConfig,
    pipelines: Vec<Pipeline>,
}

impl std::fmt::Debug for RestPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestPolicy").field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl RestPolicy {
    /// Panics on a configuration the policy cannot run: an attempt timeout longer than the total
    /// one, a sampling duration shorter than two attempt timeouts, a failure ratio outside (0, 1],
    /// a minimum throughput under 2, or a zero concurrency limit.
    pub fn new(cfg: DcgRestConfig) -> Self {
        if let Some(attempt_timeout) = cfg.attempt_timeout {
            assert!(attempt_timeout <= cfg.total_timeout, "attempt timeout exceeds the total timeout: {cfg:?}");
            assert!(
                cfg.breaker_sampling >= attempt_timeout.saturating_mul(2),
                "breaker sampling shorter than two attempt timeouts: {cfg:?}"
            );
        }

        assert!(
            cfg.breaker_failure_ratio > 0.0 && cfg.breaker_failure_ratio <= 1.0,
            "breaker failure ratio outside (0, 1]: {cfg:?}"
        );
        assert!(cfg.breaker_minimum_throughput >= 2, "breaker minimum throughput under 2: {cfg:?}");
        assert!(cfg.concurrency_limit > 0, "zero concurrency limit: {cfg:?}");

        let now = Instant::now();
        let pipelines = ServiceClient::ALL
            .iter()
            .map(|_| Pipeline { in_flight: AtomicUsize::new(0), breaker: Mutex::new(Breaker::new(&cfg, now)) })
            .collect();
        Self { cfg, pipelines }
    }

    pub fn config(&self) -> &DcgRestConfig {
        &self.cfg
    }

    /// The state of a service client's circuit.
    pub fn circuit_state(&self, client: ServiceClient) -> CircuitState {
        self.pipelines[client.index()].breaker.lock_or_panic().state
    }

    /// Runs one call: `send(None)` sends the request, `send(Some(n))` its `n`-th resend (with
    /// `Retry-Attempt: n`). Returns the reply whose body the caller reads, or why there is none,
    /// with every step taken.
    pub async fn execute<F, Fut>(
        &self,
        client: ServiceClient,
        send: F,
    ) -> (Result<reqwest::Response, PolicyError>, Vec<Step>)
    where
        F: Fn(Option<u32>) -> Fut + Sync,
        Fut: Future<Output = Result<reqwest::Response, reqwest::Error>> + Send,
    {
        let pipeline = &self.pipelines[client.index()];
        let Some(_permit) = Permit::acquire(&pipeline.in_flight, self.cfg.concurrency_limit) else {
            return (Err(PolicyError::ConcurrencyLimit { limit: self.cfg.concurrency_limit }), Vec::new());
        };

        let steps = Mutex::new(Vec::new());
        let result = match tokio::time::timeout(self.cfg.total_timeout, self.attempts(pipeline, &send, &steps)).await {
            Ok(result) => result,
            Err(_) => Err(PolicyError::TimedOut(Bound::Total(self.cfg.total_timeout))),
        };

        (result, steps.into_inner().expect("lock poisoned by an earlier panic"))
    }

    async fn attempts<F, Fut>(
        &self,
        pipeline: &Pipeline,
        send: &F,
        steps: &Mutex<Vec<Step>>,
    ) -> Result<reqwest::Response, PolicyError>
    where
        F: Fn(Option<u32>) -> Fut + Sync,
        Fut: Future<Output = Result<reqwest::Response, reqwest::Error>> + Send,
    {
        let mut prev = 0.0;
        let mut waited = Duration::ZERO;
        let mut attempt = 0;

        loop {
            let outcome = self.attempt(pipeline, attempt, waited, send, steps).await;
            if attempt >= self.cfg.retries || !outcome.is_transient() {
                return outcome.into_result();
            }

            let mut delay = backoff_delay(attempt, self.cfg.retry_base_delay, &mut prev, rand::random::<f64>());
            if let Attempt::Reply(reply) = &outcome
                && let Some(after) = retry_after(reply.headers(), SystemTime::now())
            {
                delay = after;
            }

            drop(outcome);
            tokio::time::sleep(delay).await;
            waited = delay;
            attempt += 1;
        }
    }

    async fn attempt<F, Fut>(
        &self,
        pipeline: &Pipeline,
        attempt: u32,
        waited: Duration,
        send: &F,
        steps: &Mutex<Vec<Step>>,
    ) -> Attempt
    where
        F: Fn(Option<u32>) -> Fut + Sync,
        Fut: Future<Output = Result<reqwest::Response, reqwest::Error>> + Send,
    {
        let admitted = pipeline.breaker.lock_or_panic().before_attempt(Instant::now());
        if let Err(retry_after) = admitted {
            steps.lock_or_panic().push(Step::CircuitOpen { attempt });
            return Attempt::CircuitOpen(retry_after);
        }

        let sample = Sample { breaker: &pipeline.breaker, recorded: false };
        let resends = self.resends(attempt, waited, send, steps);
        let outcome = match self.cfg.attempt_timeout {
            None => resends.await,
            Some(bound) => match tokio::time::timeout(bound, resends).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    steps.lock_or_panic().push(Step::AttemptTimedOut { attempt });
                    Attempt::TimedOut(bound)
                }
            },
        };

        sample.record(outcome.is_transient());
        outcome
    }

    async fn resends<F, Fut>(&self, attempt: u32, waited: Duration, send: &F, steps: &Mutex<Vec<Step>>) -> Attempt
    where
        F: Fn(Option<u32>) -> Fut + Sync,
        Fut: Future<Output = Result<reqwest::Response, reqwest::Error>> + Send,
    {
        let sent = |resend: Option<u32>, waited: Duration, result: &Result<reqwest::Response, reqwest::Error>| {
            let outcome = match result {
                Ok(reply) => Ok(reply.status().as_u16()),
                Err(e) => Err(e.to_string()),
            };
            steps.lock_or_panic().push(Step::Sent { attempt, resend, waited, outcome });
        };

        let first = send(None).await;
        sent(None, waited, &first);
        let mut reply = match first {
            Ok(reply) => reply,
            Err(e) => return Attempt::NoReply(e),
        };
        if !is_resent_status(reply.status().as_u16()) {
            return Attempt::Reply(reply);
        }

        for k in 0..self.cfg.resends {
            let wait = match resend_wait(
                reply.headers(),
                k,
                self.cfg.resend_delay,
                self.cfg.resend_max_delay,
                SystemTime::now(),
            ) {
                Ok(wait) => wait,
                Err(value) => return Attempt::NegativeRetryAfter(value),
            };

            drop(reply);
            tokio::time::sleep(wait).await;
            let next = send(Some(k + 1)).await;
            sent(Some(k + 1), wait, &next);
            reply = match next {
                Ok(reply) => reply,
                Err(e) => return Attempt::NoReply(e),
            };
            if !is_resent_status(reply.status().as_u16()) {
                return Attempt::Reply(reply);
            }
        }

        if self.cfg.resends == 0 {
            return Attempt::Reply(reply);
        }
        Attempt::ResendsExhausted { status: reply.status().as_u16(), sends: self.cfg.resends + 1 }
    }
}

#[cfg(test)]
mod tests;
