//! QR/PIN account pairing: the session's state machine over scripted calls (paused time), its wire
//! format over the fake DCG service, and a pairing run by the engine that ends with the new phone
//! offered for selection.

use crate::support::auth_world;
use crate::support::engine_world as world;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use auth_world::harness::{Harness, eventually};
use phonelink_engine::api::events::EngineEvent;
use phonelink_engine::auth::dcg_rest::{RestError, ServiceFailure, ServiceRequestStatus, TransportCause};
use phonelink_engine::auth::pairing::{
    PairingCalls, PairingFlow, PairingManager, PairingResult, PairingStep, PhoneLookup, PinKind, PinProblem,
    RefreshFailure, ServiceState, Session, SessionResponse, create_request_body, run_session,
};
use phonelink_engine::config::PairingConfig;
use phonelink_engine::edge::SecureStore;
use phonelink_engine::edge::standins::FixedReachability;
use phonelink_engine::edge::testing::{ManualClock, RegionHost};
use serde_json::{Value, json};
use world::World;

// ------------------------------------------------------------------------------------------------
// Scripted calls
// ------------------------------------------------------------------------------------------------

enum Reply {
    Body(String),
    Fail(RestError),
    /// Never answers.
    Hang,
}

#[derive(Default)]
struct Calls {
    script: Mutex<VecDeque<(&'static str, Reply)>>,
    made: Mutex<Vec<&'static str>>,
}

impl Calls {
    fn new(script: Vec<(&'static str, Reply)>) -> Arc<Self> {
        Arc::new(Self { script: Mutex::new(script.into()), made: Mutex::new(Vec::new()) })
    }

    fn made(&self) -> Vec<&'static str> {
        self.made.lock().unwrap().clone()
    }

    async fn answer(&self, call: &'static str) -> Result<String, RestError> {
        self.made.lock().unwrap().push(call);
        let next = self.script.lock().unwrap().pop_front();
        let Some((expected, reply)) = next else { panic!("unscripted call {call}") };
        assert_eq!(call, expected, "calls out of order");

        match reply {
            Reply::Body(b) => Ok(b),
            Reply::Fail(e) => Err(e),
            Reply::Hang => std::future::pending().await,
        }
    }
}

#[async_trait]
impl PairingCalls for Calls {
    async fn create(&self, _flow: PairingFlow) -> Result<String, RestError> {
        self.answer("create").await
    }
    async fn get(&self, _id: &str) -> Result<String, RestError> {
        self.answer("get").await
    }
    async fn complete(&self, _id: &str) -> Result<String, RestError> {
        self.answer("complete").await
    }
    async fn refresh_challenge_pin(&self, _id: &str) -> Result<String, RestError> {
        self.answer("refresh").await
    }
}

struct Lookup(bool, Mutex<Vec<String>>);

#[async_trait]
impl PhoneLookup for Lookup {
    async fn learn_phone(&self, id: &str) -> bool {
        self.1.lock().unwrap().push(id.to_string());
        self.0
    }
}

/// The service's clock, equal to the test clock (`ManualClock::fixed`).
const NOW: &str = "2026-10-20T00:00:00Z";

/// A session response in `state` with the common keys and `extra`.
fn session(state: &str, extra: Value) -> String {
    let mut v = json!({
        "pairingSessionId": "sess-1",
        "state": state,
        "currentServiceTime": NOW,
        "pairingSessionExpireTime": "2026-10-20T00:15:00Z",
    });
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v.to_string()
}

fn qr_created() -> Reply {
    Reply::Body(session(
        "PENDING_PAIRING_LINK_VISIT",
        json!({
            "pairingLink": "https://aka.ms/qr?code=abc&vm=1",
            "pairingLinkExpireTime": "2026-10-20T00:10:00Z",
            "pairingLinkForSignInPINFlow": "https://aka.ms/manual",
        }),
    ))
}

fn state(name: &str) -> Reply {
    Reply::Body(session(name, json!({})))
}

fn challenge(state: &str, pin: &str) -> Reply {
    Reply::Body(session(state, json!({ "challengePIN": pin, "challengePINExpireTime": "2026-10-20T00:05:00Z" })))
}

fn paired(phone: &str) -> Reply {
    Reply::Body(session("DEVICE_PAIRED", json!({ "partnerDcgClientId": phone })))
}

fn status(http: u16, status: ServiceRequestStatus) -> Reply {
    Reply::Fail(RestError::Status(Box::new(ServiceFailure {
        call: "Pairing",
        http_status: http,
        status,
        error: None,
        body: String::new(),
    })))
}

fn no_connection() -> Reply {
    Reply::Fail(RestError::Transport { call: "Pairing", cause: TransportCause::Internet, detail: "down".into() })
}

struct Run {
    calls: Arc<Calls>,
    steps: Arc<Mutex<Vec<PairingStep>>>,
    lookup: Arc<Lookup>,
    reachability: Arc<FixedReachability>,
    clock: Arc<ManualClock>,
}

impl Run {
    fn new(script: Vec<(&'static str, Reply)>) -> Self {
        Self {
            calls: Calls::new(script),
            steps: Arc::new(Mutex::new(Vec::new())),
            lookup: Arc::new(Lookup(true, Mutex::new(Vec::new()))),
            reachability: Arc::new(FixedReachability::online()),
            clock: Arc::new(ManualClock::fixed()),
        }
    }

    fn session(&self, flow: PairingFlow) -> Session {
        let steps = self.steps.clone();
        Session {
            config: PairingConfig::default(),
            flow,
            calls: self.calls.clone(),
            lookup: self.lookup.clone(),
            clock: self.clock.clone(),
            reachability: self.reachability.clone(),
            emit: Box::new(move |step| steps.lock().unwrap().push(step)),
        }
    }

    async fn run(&self, flow: PairingFlow) -> PairingResult {
        let (_refresh, handle) = run_session(self.session(flow));
        handle.await.unwrap()
    }

    fn steps(&self) -> Vec<PairingStep> {
        self.steps.lock().unwrap().clone()
    }
}

fn kinds(steps: &[PairingStep]) -> Vec<&'static str> {
    steps
        .iter()
        .map(|s| match s {
            PairingStep::Creating { .. } => "creating",
            PairingStep::WaitingForScan { .. } => "scan",
            PairingStep::WaitingForPin { .. } => "pin",
            PairingStep::PinRefresh { refreshing: false, .. } => "refresh",
            PairingStep::PinRefresh { refreshing: true, .. } => "refreshing",
            PairingStep::WaitingForPermissions => "permissions",
            PairingStep::Completing => "completing",
            PairingStep::Ended(_) => "ended",
        })
        .collect()
}

fn secs_since_epoch(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// ------------------------------------------------------------------------------------------------
// Wire values
// ------------------------------------------------------------------------------------------------

#[test]
fn the_create_body_names_the_flow_and_all_permissions_and_omits_the_unset_keys() {
    let qr: Value = serde_json::from_str(&create_request_body(PairingFlow::QrCode)).unwrap();
    assert_eq!(qr, json!({ "flow": "QRLINK_CHALLENGEPIN_SIGNIN", "optionalPermission": ["ALL"] }));
    let pin: Value = serde_json::from_str(&create_request_body(PairingFlow::ManualPin)).unwrap();
    assert_eq!(pin["flow"], "SIGNIN_PIN");
}

#[test]
fn states_parse_ignoring_case_and_unknown_or_missing_ones_end_the_session() {
    assert_eq!(ServiceState::parse("device_paired"), Some(ServiceState::DevicePaired));
    assert_eq!(ServiceState::parse("Pending_Pairing_Link_Visit"), Some(ServiceState::PendingPairingLinkVisit));
    assert_eq!(ServiceState::parse("SOMETHING_NEW"), None);
    assert_eq!(SessionResponse::parse(r#"{"state":"NOPE"}"#).unwrap_err(), PairingResult::ServiceError);
    assert_eq!(SessionResponse::parse(r#"{"state":7}"#).unwrap_err(), PairingResult::ServiceError);
    assert_eq!(SessionResponse::parse(r#"{"state":""}"#).unwrap_err(), PairingResult::ServiceError);
    assert_eq!(SessionResponse::parse(r#"{}"#).unwrap_err(), PairingResult::ServiceError);
    assert_eq!(SessionResponse::parse("").unwrap_err(), PairingResult::InternalError);
    assert_eq!(SessionResponse::parse("[1]").unwrap_err(), PairingResult::InternalError);
}

// ------------------------------------------------------------------------------------------------
// The state machine
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_qr_flow_runs_from_the_code_to_the_paired_phone() {
    let r = Run::new(vec![
        ("create", qr_created()),
        ("get", state("PENDING_PAIRING_LINK_VISIT")),
        ("get", challenge("PENDING_CHALLENGE_PIN_VALIDATE", "4821")),
        ("get", state("PENDING_CHALLENGE_PIN_VALIDATE")),
        ("get", state("READY_FOR_COMPLETE")),
        ("complete", state("PENDING_PAIRING_PERMISSION_APPROVE")),
        ("get", state("PENDING_PAIRING_PERMISSION_APPROVE")),
        ("get", paired("phone-1")),
    ]);

    let started = tokio::time::Instant::now();
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::Success { phone_id: "phone-1".into() });
    assert_eq!(
        kinds(&r.steps()),
        ["creating", "scan", "pin", "permissions", "permissions", "completing", "ended"],
        "{:?}",
        r.steps()
    );

    let PairingStep::WaitingForScan { qr_code, manual_link } = &r.steps()[1] else { unreachable!() };
    assert_eq!(qr_code.value, "https://aka.ms/qr?code=abc&vm=1", "the link verbatim");
    assert_eq!(qr_code.expires_in_minutes, 10);
    assert_eq!(manual_link, "https://aka.ms/manual");

    let PairingStep::WaitingForPin { pin, kind } = &r.steps()[2] else { unreachable!() };
    assert_eq!((pin.value.as_str(), *kind, pin.expires_in_minutes), ("4821", PinKind::Challenge, 5));
    assert_eq!(r.lookup.1.lock().unwrap().as_slice(), ["phone-1"]);

    // Waits of 2 s after each response that keeps the phase (link visit, PIN, permissions) and after
    // the `Complete` answer that asks for permissions.
    assert_eq!(started.elapsed(), Duration::from_secs(8));
}

#[tokio::test(start_paused = true)]
async fn the_manual_flow_shows_the_pairing_pin() {
    let r = Run::new(vec![
        (
            "create",
            Reply::Body(session(
                "PENDING_PAIRING_PIN_VALIDATE",
                json!({ "pairingPIN": "ABC123", "pairingPINExpireTime": "2026-10-20T00:03:00Z" }),
            )),
        ),
        ("get", state("PENDING_PAIRING_PIN_VALIDATE")),
        ("get", paired("phone-2")),
    ]);

    assert_eq!(r.run(PairingFlow::ManualPin).await, PairingResult::Success { phone_id: "phone-2".into() });
    let PairingStep::WaitingForPin { pin, kind } = &r.steps()[1] else { panic!("{:?}", r.steps()) };
    assert_eq!((pin.value.as_str(), *kind), ("ABC123", PinKind::Pairing));
}

#[tokio::test(start_paused = true)]
async fn expiries_move_onto_the_macs_clock_by_the_measured_skew() {
    let r = Run::new(vec![("create", qr_created()), ("get", Reply::Hang)]);

    // The Mac's clock runs 90 s ahead of the service's.
    r.clock.advance(Duration::from_secs(90));
    let (_refresh, handle) = run_session(r.session(PairingFlow::QrCode));
    tokio::time::sleep(Duration::from_millis(10)).await;
    handle.abort();

    let PairingStep::WaitingForScan { qr_code, .. } = &r.steps()[1] else { panic!("{:?}", r.steps()) };
    assert_eq!(secs_since_epoch(qr_code.expires_at), 1_792_454_400 + 600 + 90);
    assert_eq!(qr_code.expires_in_minutes, 10);
}

#[tokio::test(start_paused = true)]
async fn the_phone_on_another_account_ends_the_session_as_mismatched() {
    let r = Run::new(vec![("create", qr_created()), ("get", state("PAIRING_ACCOUNT_MISMATCHED"))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::MsaTokenMismatched);
}

#[tokio::test(start_paused = true)]
async fn a_state_the_phase_does_not_accept_ends_the_session_without_a_retry() {
    for unexpected in ["READY_FOR_COMPLETE", "PAIRING_FAILED", "PAIRING_REQUIRED_PERMISSION_DENIED", "SOMETHING_NEW"] {
        let r = Run::new(vec![("create", qr_created()), ("get", state(unexpected))]);
        assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::ServiceError, "{unexpected}");
        assert_eq!(r.calls.made(), ["create", "get"]);
    }

    let r = Run::new(vec![("create", qr_created()), ("get", Reply::Body("oops".into()))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::InternalError);

    // The wrong flow's first state.
    let r = Run::new(vec![("create", state("PENDING_PAIRING_PIN_VALIDATE"))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::ServiceError);
}

#[tokio::test(start_paused = true)]
async fn a_shown_value_must_be_present_and_valid_for_a_minute() {
    let short = Reply::Body(session(
        "PENDING_PAIRING_LINK_VISIT",
        json!({
            "pairingLink": "x",
            "pairingLinkExpireTime": "2026-10-20T00:00:20Z",
            "pairingLinkForSignInPINFlow": "y",
        }),
    ));
    let r = Run::new(vec![("create", short)]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::ServiceError);

    let no_manual = Reply::Body(session(
        "PENDING_PAIRING_LINK_VISIT",
        json!({ "pairingLink": "x", "pairingLinkExpireTime": "2026-10-20T00:10:00Z" }),
    ));
    let r = Run::new(vec![("create", no_manual)]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::ServiceError);

    let r = Run::new(vec![("create", qr_created()), ("get", challenge("PENDING_CHALLENGE_PIN_VALIDATE", ""))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::ServiceError, "an empty challenge PIN");
}

#[tokio::test(start_paused = true)]
async fn expiries_end_the_session() {
    for expired in ["PAIRING_LINK_EXPIRED", "PAIRING_SESSION_EXPIRED"] {
        let r = Run::new(vec![("create", qr_created()), ("get", state(expired))]);
        assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::PairingSessionExpired);
    }
}

#[tokio::test(start_paused = true)]
async fn recoverable_failures_are_retried_within_the_attempts() {
    // Create: three attempts.
    let r = Run::new(vec![
        ("create", status(503, ServiceRequestStatus::ServiceError)),
        ("create", no_connection()),
        ("create", qr_created()),
        ("get", paired("p")),
    ]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::Success { phone_id: "p".into() });

    let r = Run::new(vec![
        ("create", status(503, ServiceRequestStatus::ServiceError)),
        ("create", status(429, ServiceRequestStatus::TooManyRequests)),
        ("create", status(401, ServiceRequestStatus::UnauthorizedRequest)),
    ]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::MsaTokenInvalid);
    assert_eq!(r.calls.made().len(), 3);

    // A poll step: two attempts, counted again after a response.
    let r = Run::new(vec![
        ("create", qr_created()),
        ("get", no_connection()),
        ("get", state("PENDING_PAIRING_LINK_VISIT")),
        ("get", no_connection()),
        ("get", no_connection()),
    ]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::NetworkUnavailable);
    assert_eq!(r.calls.made().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn some_failures_are_never_retried() {
    let r = Run::new(vec![("create", status(401, ServiceRequestStatus::MsaUnauthorizedRequest))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::MsaTokenInvalid);

    let r = Run::new(vec![("create", status(400, ServiceRequestStatus::BadRequest))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::InternalError);

    // The same failure while offline is retried, and ends as a network failure.
    let r = Run::new(vec![
        ("create", status(400, ServiceRequestStatus::BadRequest)),
        ("create", status(400, ServiceRequestStatus::BadRequest)),
        ("create", status(400, ServiceRequestStatus::BadRequest)),
    ]);
    r.reachability.0.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::NetworkUnavailable);
    assert_eq!(r.calls.made().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn the_step_bounds_end_the_session() {
    let r = Run::new(vec![("create", Reply::Hang)]);
    let started = tokio::time::Instant::now();
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::TimeoutError);
    assert_eq!(started.elapsed(), Duration::from_secs(30), "no second create attempt");

    let r = Run::new(vec![("create", qr_created()), ("get", Reply::Hang)]);
    let started = tokio::time::Instant::now();
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::TimeoutError);
    assert_eq!(started.elapsed(), Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn a_failed_complete_is_retried_as_a_poll() {
    let r = Run::new(vec![
        ("create", qr_created()),
        ("get", challenge("PENDING_CHALLENGE_PIN_VALIDATE", "1111")),
        ("get", state("READY_FOR_COMPLETE")),
        ("complete", status(500, ServiceRequestStatus::ServiceError)),
        ("get", paired("p")),
    ]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::Success { phone_id: "p".into() });
}

#[tokio::test(start_paused = true)]
async fn an_expired_challenge_pin_waits_for_the_users_refresh() {
    let r = Run::new(vec![
        ("create", qr_created()),
        ("get", challenge("PENDING_CHALLENGE_PIN_VALIDATE", "1111")),
        ("get", state("CHALLENGE_PIN_EXPIRED")),
        ("refresh", status(403, ServiceRequestStatus::ForbiddenDcgRequest)),
        ("refresh", challenge("PENDING_CHALLENGE_PIN_VALIDATE", "2222")),
        ("get", paired("p")),
    ]);

    let (refresh, handle) = run_session(r.session(PairingFlow::QrCode));
    let steps = r.steps.clone();
    let waiting =
        || steps.lock().unwrap().last().is_some_and(|s| matches!(s, PairingStep::PinRefresh { refreshing: false, .. }));
    assert!(eventually(Duration::from_secs(10), waiting).await);

    let polled = r.calls.made().len();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(r.calls.made().len(), polled, "no polling while waiting for a refresh");

    refresh.send(()).unwrap();
    assert!(eventually(Duration::from_secs(10), || r.calls.made().len() == polled + 1).await);
    assert!(eventually(Duration::from_secs(10), waiting).await);

    let Some(PairingStep::PinRefresh { pin, problem, failure, .. }) = r.steps().last().cloned() else { unreachable!() };
    assert_eq!(
        (pin.value.as_str(), problem, failure),
        ("1111", PinProblem::Expired, Some(RefreshFailure::ForbiddenDcgRequest))
    );

    refresh.send(()).unwrap();
    assert_eq!(handle.await.unwrap(), PairingResult::Success { phone_id: "p".into() });
    let new_pin = r.steps().into_iter().rev().find_map(|s| match s {
        PairingStep::WaitingForPin { pin, .. } => Some(pin.value),
        _ => None,
    });
    assert_eq!(new_pin.as_deref(), Some("2222"));
}

#[tokio::test(start_paused = true)]
async fn a_wrong_challenge_pin_waits_until_the_session_expires() {
    let r = Run::new(vec![("create", qr_created()), ("get", challenge("CHALLENGE_PIN_VALIDATE_FAILED", "3333"))]);
    let started = tokio::time::Instant::now();
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::PairingSessionExpired);
    // The session expires 15 min after the create response.
    assert_eq!(started.elapsed(), Duration::from_secs(15 * 60));
    assert!(r.steps().iter().any(|s| matches!(s, PairingStep::PinRefresh { problem: PinProblem::Wrong, .. })));
}

#[tokio::test(start_paused = true)]
async fn a_paired_phone_must_be_named_and_found() {
    let r = Run::new(vec![("create", qr_created()), ("get", paired(""))]);
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::PhoneNotFound);
    let mut r = Run::new(vec![("create", qr_created()), ("get", paired("p"))]);
    r.lookup = Arc::new(Lookup(false, Mutex::new(Vec::new())));
    assert_eq!(r.run(PairingFlow::QrCode).await, PairingResult::PhoneNotFound);
}

// ------------------------------------------------------------------------------------------------
// The wire, over the fake DCG service
// ------------------------------------------------------------------------------------------------

fn fast_pairing() -> PairingConfig {
    PairingConfig { poll_interval: Duration::from_millis(20), ..PairingConfig::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sessions_go_to_the_pairing_service_with_the_account_token() {
    let h = Harness::new().await;
    let (auth, _) = h.provisioned().await;
    let manager = PairingManager::new(
        auth,
        fast_pairing(),
        h.clock.clone(),
        h.reachability.clone(),
        Arc::new(RegionHost { name: "mac".into(), region: "kr".into() }),
    );

    h.dcg.take_requests();
    h.dcg.script(
        "PairingSessions",
        200,
        &session(
            "PENDING_PAIRING_LINK_VISIT",
            json!({
                "pairingLink": "https://aka.ms/qr",
                "pairingLinkExpireTime": "2026-10-20T00:10:00Z",
                "pairingLinkForSignInPINFlow": "https://aka.ms/manual",
            }),
        ),
    );
    h.dcg.script("sess-1", 200, &session("READY_FOR_COMPLETE", json!({})));

    let ended = Arc::new(Mutex::new(Vec::new()));
    let seen = ended.clone();
    let lookup = Arc::new(Lookup(true, Mutex::new(Vec::new())));
    manager.start(PairingFlow::QrCode, lookup, Arc::new(move |n, step| seen.lock().unwrap().push((n, step)))).unwrap();

    assert!(
        eventually(Duration::from_secs(10), || ended
            .lock()
            .unwrap()
            .iter()
            .any(|(_, s)| matches!(s, PairingStep::Ended(_))))
        .await
    );
    assert_eq!(
        ended.lock().unwrap().last().unwrap().1,
        PairingStep::Ended(PairingResult::ServiceError),
        "READY_FOR_COMPLETE before a PIN"
    );

    let requests = h.dcg.take_requests();
    let lines: Vec<String> = requests.iter().map(|r| r.line()).collect();
    assert_eq!(
        lines,
        [
            "POST /DevicePairingProxy/PairingSessions?Api-Version=3.0.0",
            "GET /DevicePairingProxy/PairingSessions/sess-1?Api-Version=3.0.0",
        ]
    );

    let create = &requests[0];
    assert_eq!(create.json(), json!({ "flow": "QRLINK_CHALLENGEPIN_SIGNIN", "optionalPermission": ["ALL"] }));
    assert_eq!(create.header("clientRegionName"), Some("KR"));
    assert_eq!(requests[1].header("clientRegionName"), None, "the create call only");

    for r in &requests {
        assert!(r.header("authorization").is_some_and(|v| v.starts_with("Bearer ")), "{}", r.line());
        assert_eq!(r.header("authorization-type"), Some("MSA"));
        assert_eq!(r.header("dcg-token"), None);
        assert!(r.header("dcg-appid").is_some(), "the DCG client headers");
        assert!(r.header("traceparent").is_some());
    }

    assert_eq!(requests[0].header("traceparent"), requests[1].header("traceparent"), "one trace context per session");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_session_cancels_the_running_one() {
    let h = Harness::new().await;
    let (auth, _) = h.provisioned().await;
    let manager = PairingManager::new(
        auth,
        fast_pairing(),
        h.clock.clone(),
        h.reachability.clone(),
        Arc::new(RegionHost { name: "mac".into(), region: "us".into() }),
    );

    h.dcg.script(
        "PairingSessions",
        200,
        &session(
            "PENDING_PAIRING_LINK_VISIT",
            json!({
                "pairingLink": "https://aka.ms/qr",
                "pairingLinkExpireTime": "2026-10-20T00:10:00Z",
                "pairingLinkForSignInPINFlow": "https://aka.ms/manual",
            }),
        ),
    );
    h.dcg.script_times("sess-1", 1000, 200, &session("PENDING_PAIRING_LINK_VISIT", json!({})));

    let steps = Arc::new(Mutex::new(Vec::new()));
    let seen = steps.clone();
    let observer: phonelink_engine::auth::pairing::PairingObserver =
        Arc::new(move |n, step| seen.lock().unwrap().push((n, step)));
    let lookup = Arc::new(Lookup(true, Mutex::new(Vec::new())));
    let first = manager.start(PairingFlow::QrCode, lookup.clone(), observer.clone()).unwrap();
    assert!(
        eventually(Duration::from_secs(10), || manager
            .last_step()
            .is_some_and(|(_, s)| matches!(s, PairingStep::WaitingForScan { .. })))
        .await
    );

    h.dcg.script("PairingSessions", 200, "not json");
    let second = manager.start(PairingFlow::ManualPin, lookup, observer).unwrap();
    assert!(second > first);
    assert!(
        eventually(Duration::from_secs(10), || {
            let steps = steps.lock().unwrap();
            steps.contains(&(first, PairingStep::Ended(PairingResult::UserCanceled)))
                && steps.contains(&(second, PairingStep::Ended(PairingResult::InternalError)))
        })
        .await
    );

    assert_eq!(manager.last_step(), Some((second, PairingStep::Ended(PairingResult::InternalError))));
    manager.cancel();
    assert!(!manager.refresh_challenge_pin(), "no session runs");
}

// ------------------------------------------------------------------------------------------------
// The engine
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_phone_is_learned_from_the_refreshed_device_list() {
    let w = World::new(None).await;
    w.h.store.delete(phonelink_engine::engine::SELECTED_PHONE_KEY.into()).unwrap();

    let mut config = w.config();
    config.auth.pairing = fast_pairing();
    let engine = phonelink_engine::engine::EngineCore::build(
        config,
        w.h.edges(),
        World::feature_edges(),
        phonelink_engine::tap::MemoryTap::new(),
        w.h.core_events(),
    )
    .unwrap();
    engine.start(None).await.unwrap();

    let galaxy = w.h.phone("Galaxy");
    let galaxy_id = galaxy.id.clone();
    w.h.dcg.script(
        "PairingSessions",
        200,
        &session(
            "PENDING_PAIRING_PIN_VALIDATE",
            json!({
                "pairingPIN": "778899",
                "pairingPINExpireTime": "2026-10-20T00:10:00Z",
            }),
        ),
    );
    w.h.dcg.script("sess-1", 200, &session("DEVICE_PAIRED", json!({ "partnerDcgClientId": galaxy_id })));

    // The phone has joined the account's device list; the engine learns it only from a refresh.
    w.h.dcg.add_peer(galaxy.item.clone());
    assert!(!engine.phones().iter().any(|p| p.client.id == galaxy_id));
    w.h.events.take();
    let lists_before = w.h.dcg.count("GetDeviceInfoList");

    let number = engine.start_pairing(PairingFlow::ManualPin).unwrap();
    let ended = |events: &[EngineEvent]| {
        events.iter().find_map(|e| match e {
            EngineEvent::Pairing { update } if update.session == number => match &update.step {
                phonelink_engine::api::pairing::PairingStep::Ended { result } => Some(result.clone()),
                _ => None,
            },
            _ => None,
        })
    };

    let events = w.h.events.clone();
    let mut seen = Vec::new();
    assert!(
        eventually(Duration::from_secs(20), || {
            seen.extend(events.take());
            ended(&seen).is_some()
        })
        .await
    );
    assert_eq!(
        ended(&seen),
        Some(phonelink_engine::api::pairing::PairingResult::Success { phone_id: galaxy_id.clone() })
    );
    assert!(engine.phones().iter().any(|p| p.client.id == galaxy_id), "offered for selection");
    assert!(
        seen.iter().any(|e| matches!(e, EngineEvent::Pairing { update } if matches!(&update.step,
            phonelink_engine::api::pairing::PairingStep::WaitingForPin { pin, .. } if pin.value == "778899"))),
        "the PIN was shown"
    );

    let refreshes = w.h.dcg.count("GetDeviceInfoList") - lists_before;
    assert_eq!(refreshes, 1, "a forced refresh of the (only) identity's device list");
}
