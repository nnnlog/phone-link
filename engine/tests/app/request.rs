//! Outbound App requests: the request, the response and its correlation, sending, timeouts and
//! retries, and the `/permission` request.

use crate::support::app_world;
use crate::support::common::payload;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_world::*;
use phonelink_engine::app::{
    AppError, AppEvent, AppOutcome, AppRequest, AppResponse, LocalGate, RequestKind, RequestObserver, RequestOptions,
};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::edge::LockState;
use phonelink_engine::seams::{ConnectionFailure, PlatformRequestStatus, StartPolicy};
use phonelink_engine::tap::{Direction, TapLayer};
use phonelink_engine::types::{
    DcgClient, DcgSendStatus, HeaderMap, HubSendResult, MessageType, OpenStatus, PartnerDisconnectReason, TraceContext,
    TransportMessage, WakeStatus,
};

fn feature_values() -> ValueSet {
    ValueSet::new()
        .with("contentType", "deviceStatus")
        .with("correlationVector", "0011223344556677")
        .with("displayName", "feature's own")
        .with("contractVersion", 1.0f64)
        .with("isuserinitiated", true)
}

async fn send(h: &Harness, req: AppRequest) -> AppResponse {
    h.app.request(req).await.unwrap()
}

#[tokio::test(start_paused = true)]
async fn kvp_request_gets_the_four_standard_keys_written_over_the_features() {
    let h = Harness::new();
    let resp = send(&h, AppRequest::kvp("/permission", feature_values())).await;
    assert_eq!(resp.outcome, AppOutcome::Success);

    let req = &h.messenger.requests()[0];
    let vs = payload(req);
    assert_eq!(vs.str("displayName"), Some("test-mac"));
    assert_eq!(vs.str("appVersion"), Some("1.26092.119.0"));
    assert_eq!(vs.str("installationId"), Some(h.app.installation_id()));
    assert_eq!(vs.get("contractVersion"), Some(&Value::Double(3.99)));

    // No legacy keys; the feature's KVP `isuserinitiated: true` stays and makes it user-initiated.
    assert!(!vs.contains_key("transportType"));
    assert!(!vs.contains_key("ignoreResponse"));
    assert_eq!(vs.bool("isuserinitiated"), Some(true));
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    assert_eq!(vs.len(), 7);

    // Envelope: `_requestId` then `_route`, nothing else.
    let keys: Vec<_> = req.message.headers.iter().map(|(k, _)| k).collect();
    assert_eq!(keys, ["_requestId", "_route"]);
}

#[tokio::test(start_paused = true)]
async fn kvp_without_the_key_follows_the_options() {
    let h = Harness::new();
    let vs = ValueSet::new().with("contentType", "x").with("isuserinitiated", "true"); // a String: no effect
    send(&h, AppRequest::kvp("/a", vs)).await;
    let req = &h.messenger.requests()[0];
    assert_eq!(req.priority, 100);
    assert!(!req.options.ignore_device_disabled);

    send(&h, AppRequest::kvp("/a", ValueSet::new()).with_options(RequestOptions::user_initiated())).await;
    let req = &h.messenger.requests()[1];
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    assert!(!payload(req).contains_key("isuserinitiated"));
}

#[tokio::test(start_paused = true)]
async fn legacy_request_writes_its_keys_over_the_features() {
    let h = Harness::new();
    send(&h, AppRequest::legacy("/devicestatus", feature_values())).await;
    let req = &h.messenger.requests()[0];
    let vs = payload(req);

    // The feature's own `isuserinitiated: true` never reaches the wire: the option (false) does.
    assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(false)));
    assert_eq!(vs.get("ignoreResponse"), Some(&Value::Bool(false)));
    assert_eq!(vs.get("transportType"), Some(&Value::Int32(0)));
    assert_eq!(vs.str("displayName"), Some("test-mac"));
    assert_eq!(vs.get("contractVersion"), Some(&Value::Double(3.99)));
    assert_eq!(vs.str("contentType"), Some("deviceStatus"));
    assert_eq!(req.priority, 100);
    assert!(!req.options.ignore_device_disabled);

    let opts = RequestOptions::user_initiated();
    send(&h, AppRequest::legacy("/command/devicestatus", ValueSet::new()).with_options(opts)).await;
    let req = &h.messenger.requests()[1];
    let vs = payload(req);
    assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(true)));
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        [
            "isuserinitiated",
            "ignoreResponse",
            "transportType",
            "displayName",
            "appVersion",
            "installationId",
            "contractVersion"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn priority_and_connection_policy_follow_the_options() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    // Default: priority 100 → Passive, which the engine's connection runs as Default.
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;

    h.sessions.set_connected(false);
    // Priority 0, not user-initiated → Default.
    let opts = RequestOptions { priority: 0, ..Default::default() };
    send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(opts)).await;

    h.sessions.set_connected(false);
    // User-initiated → NewSession + IgnoreDeviceDisabled.
    send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(RequestOptions::user_initiated())).await;

    let connects = h.sessions.connects();
    let policies: Vec<_> = connects.iter().map(|(_, c)| (c.policy, c.ignore_device_disabled)).collect();
    assert_eq!(
        policies,
        [(StartPolicy::Default, false), (StartPolicy::Default, false), (StartPolicy::NewSession, true)]
    );
    assert!(connects.iter().all(|(p, c)| *p == phone() && c.feedback_window == Duration::from_secs(70)));
    let prios: Vec<_> = h.messenger.requests().iter().map(|r| r.priority).collect();
    assert_eq!(prios, [100, 0, 0]);

    // Already connected: no connect is started.
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(h.sessions.connects().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn timeout_and_trace_context() {
    let h = Harness::new();
    h.app.set_foreground(true);
    let mut trace = TraceContext::new_root(vec![("other".into(), "x".into())]);
    trace.flags = 1;
    let opts =
        RequestOptions::default().named("NotificationArrived", "Notifications").with_timeout(Duration::from_secs(10));
    send(&h, AppRequest::legacy("/notifications", ValueSet::new()).with_options(opts).with_trace(trace.clone())).await;

    let req = &h.messenger.requests()[0];
    assert_eq!(req.timeout, Duration::from_secs(10));
    assert_eq!(req.trace.trace_id, trace.trace_id);
    assert_eq!(req.trace.correlation_id, trace.correlation_id);
    assert_eq!(req.trace.flags, 0);
    let state: Vec<(&str, &str)> = req.trace.trace_state.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        state,
        [
            ("ypTriggerId", "NotificationArrived"),
            ("ypScenarioId", "Notifications"),
            ("isuserinitiated", "False"),
            ("isForeground", "True")
        ]
    );

    // Without options: 120 s.
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(h.messenger.requests()[1].timeout, Duration::from_secs(120));
    assert_eq!(h.messenger.requests()[1].trace.correlation_id.len(), 16);
}

#[tokio::test(start_paused = true)]
async fn local_gates_fail_the_request_without_sending() {
    // AsyncTrust: a device-list phone without it, not user-initiated.
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":1}"#));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!((resp.outcome.clone(), resp.local_gate), (AppOutcome::Canceled, Some(LocalGate::AsyncTrust)));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(RequestOptions::user_initiated())).await;
    assert_eq!(resp.outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests().len(), 1);

    // AsyncTrust is the outermost gate: checked before the lock, and it leaves the flag unchanged.
    h.app.update_phone_record(|r| r.all_features_disabled = true);
    h.lock.set(LockState::Locked);
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(resp.local_gate, Some(LocalGate::AsyncTrust));
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    h.lock.set(LockState::Unlocked);

    // Lock: locked and unknown both count as locked, even for user-initiated requests.
    let h = Harness::new();
    for state in [LockState::Locked, LockState::Unknown] {
        h.lock.set(state);
        let resp =
            send(&h, AppRequest::kvp("/a", ValueSet::new()).with_options(RequestOptions::user_initiated())).await;
        assert_eq!(resp.local_gate, Some(LocalGate::Locked), "{state:?}");
        assert_eq!(resp.outcome, AppOutcome::Canceled);
    }
    h.lock.set(LockState::Unlocked);

    // Account not ready.
    h.account.set(false);
    let resp = send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert_eq!(resp.local_gate, Some(LocalGate::AccountNotReady));
    h.account.set(true);
    assert!(h.messenger.requests().is_empty());
    assert!(h.sessions.connects().is_empty());

    // The "all features disabled" flag: the lock gate leaves the flag as it was; the account gate
    // clears it.
    h.app.update_phone_record(|r| r.all_features_disabled = true);
    h.lock.set(LockState::Locked);
    send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert!(h.app.phone_record().unwrap().all_features_disabled, "lock gate: unchanged");
    h.lock.set(LockState::Unlocked);
    h.account.set(false);
    send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert!(!h.app.phone_record().unwrap().all_features_disabled, "account gate: cleared");
    h.account.set(true);

    // No phone selected.
    let h = Harness::without_phone(Default::default(), Default::default());
    let resp = send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert_eq!(resp.local_gate, Some(LocalGate::NoPhone));
}

#[tokio::test(start_paused = true)]
async fn programming_errors_are_errors() {
    let h = Harness::new();
    for r in ["", "nolead", "/internal/response", "/DeviceResourceManager", "/legacy/phonecontent"] {
        assert!(
            matches!(h.app.request(AppRequest::kvp(r, ValueSet::new())).await, Err(AppError::Programming(_))),
            "{r}"
        );
    }
    assert!(h.messenger.requests().is_empty());
}

async fn outcome_of(h: &Harness, response: R) -> (AppOutcome, Option<i32>) {
    h.messenger.answer_all(response.into_platform());
    let resp = send(h, AppRequest::kvp("/a", ValueSet::new())).await;
    (resp.outcome, resp.result)
}

/// Scripted responses for the outcome table.
mod fakes_response {
    use super::*;

    pub enum R {
        Values(ValueSet),
        Raw(Vec<(&'static str, &'static str)>, Vec<u8>),
        Status(PlatformRequestStatus),
    }

    impl R {
        pub fn into_platform(self) -> phonelink_engine::seams::PlatformResponse {
            match self {
                R::Values(vs) => response_with(vs),
                R::Raw(headers, payload) => phonelink_engine::seams::PlatformResponse {
                    status: PlatformRequestStatus::Success,
                    message: Some(TransportMessage::new(
                        MessageType::App,
                        HeaderMap::from_pairs(headers).unwrap(),
                        payload,
                    )),
                },
                R::Status(s) => status_response(s),
            }
        }
    }
}

use fakes_response::R;

#[tokio::test(start_paused = true)]
async fn result_codes_map_to_the_outcome_table() {
    let h = Harness::new();
    let table: [(i32, AppOutcome); 24] = [
        (0, AppOutcome::Success),
        (1, AppOutcome::Failure),
        (2, AppOutcome::Unknown),
        (3, AppOutcome::PhoneContractIncompatible), // no contractVersion → 0 < 3
        (4, AppOutcome::Accepted),
        (5, AppOutcome::Denied),
        (6, AppOutcome::PermanentlyDenied),
        (7, AppOutcome::NeedsPermission),
        (8, AppOutcome::MeteredConnection),
        (9, AppOutcome::NeedsInitialPermission),
        (10, AppOutcome::Success),
        (11, AppOutcome::Success),
        (12, AppOutcome::MeteredSyncDisabled),
        (13, AppOutcome::AllFeaturesDisabled),
        (14, AppOutcome::InvalidRequest),
        (15, AppOutcome::DatabaseFailure),
        (16, AppOutcome::Success),
        (17, AppOutcome::Unknown),
        (18, AppOutcome::DeviceNotSupported),
        (19, AppOutcome::Unknown),
        (20, AppOutcome::AppLaunched),
        (21, AppOutcome::TwoPhoneMode),
        (22, AppOutcome::Unknown),
        (23, AppOutcome::NonDefaultProfile),
    ];

    for (code, outcome) in table {
        assert_eq!(
            outcome_of(&h, R::Values(ValueSet::new().with("result", code))).await,
            (outcome, Some(code)),
            "{code}"
        );
    }

    assert_eq!(outcome_of(&h, R::Values(ValueSet::new().with("result", 77i32))).await.0, AppOutcome::Unknown);
}

#[tokio::test(start_paused = true)]
async fn result_3_compares_contract_majors_and_result_reading_rules() {
    let h = Harness::new();
    let v = |c: f64| R::Values(ValueSet::new().with("result", 3i32).with("contractVersion", c));
    assert_eq!(outcome_of(&h, v(3.0)).await.0, AppOutcome::Success);
    assert_eq!(outcome_of(&h, v(3.5)).await.0, AppOutcome::Success);
    assert_eq!(outcome_of(&h, v(2.99)).await.0, AppOutcome::PhoneContractIncompatible);
    assert_eq!(outcome_of(&h, v(4.1)).await.0, AppOutcome::PcContractIncompatible);

    // contractVersion as a header wins over the payload.
    let vs = valueset::encode(&ValueSet::new().with("contractVersion", 3.5f64));
    let raw = R::Raw(vec![("_route", "/internal/response"), ("result", "3"), ("contractVersion", "4.0")], vs);
    assert_eq!(outcome_of(&h, raw).await.0, AppOutcome::PcContractIncompatible);

    // A non-numeric contractVersion header reads as missing (0).
    let raw = R::Raw(vec![("result", "3"), ("contractVersion", "abc")], vec![]);
    assert_eq!(outcome_of(&h, raw).await.0, AppOutcome::PhoneContractIncompatible);

    // Missing result → -1 → unknown; a non-integer header → -1 even with a payload result.
    assert_eq!(outcome_of(&h, R::Values(ValueSet::new())).await, (AppOutcome::Unknown, Some(-1)));
    let vs = valueset::encode(&ValueSet::new().with("result", 0i32));
    assert_eq!(outcome_of(&h, R::Raw(vec![("result", "zero")], vs)).await, (AppOutcome::Unknown, Some(-1)));

    // A payload `result` of another type than Int32 is not read.
    assert_eq!(outcome_of(&h, R::Values(ValueSet::new().with("result", Value::Int64(0)))).await.1, Some(-1));

    // Result only in the payload is lifted.
    let vs = valueset::encode(&ValueSet::new().with("result", 5i32));
    assert_eq!(outcome_of(&h, R::Raw(vec![], vs.clone())).await, (AppOutcome::Denied, Some(5)));

    // Gzip, flagged case-insensitively.
    assert_eq!(
        outcome_of(&h, R::Raw(vec![("_isGzippedCompressed", "True")], gzip(&vs))).await,
        (AppOutcome::Denied, Some(5))
    );

    // An unparseable payload fails the request, and no result is read — not even the header's.
    assert_eq!(outcome_of(&h, R::Raw(vec![("result", "4")], vec![0xFF])).await, (AppOutcome::Failure, None));
    assert_eq!(outcome_of(&h, R::Raw(vec![], vec![0xFF])).await, (AppOutcome::Failure, None));
}

/// A `ValueSet` with one entry whose `value_type` is 40 (unknown).
fn unknown_value_type() -> Vec<u8> {
    vec![0x0A, 0x07, 0x0A, 0x01, b'k', 0x12, 0x02, 0x08, 40]
}

#[tokio::test(start_paused = true)]
async fn unparseable_response_fails_the_request() {
    let h = Harness::new();
    let header =
        |result: &'static str| vec![("_originalRequestId", "x"), ("_route", "/internal/response"), ("result", result)];
    let not_a_value_set = [
        (header("0"), vec![0xFF]),
        (header("0"), unknown_value_type()),
        // Flagged gzip, but the bytes (a plain `{ result: 0 }`) do not inflate.
        (
            [header("0"), vec![("_isGzippedCompressed", "true")]].concat(),
            valueset::encode(&ValueSet::new().with("result", 0i32)),
        ),
    ];

    for (headers, payload) in not_a_value_set {
        h.messenger.answer_all(R::Raw(headers.clone(), payload).into_platform());
        let resp = send(&h, AppRequest::kvp("/a", ValueSet::new())).await;

        // The `result: 0` header is not read: no success, a `Failure`.
        assert_eq!((resp.outcome.clone(), resp.result), (AppOutcome::Failure, None), "{headers:?}");
        assert!(!resp.outcome.is_success());
        assert!(resp.message.is_none());
        assert!(resp.values().is_none());

        // Logged to the tap as undecodable, not as an App response.
        let events = h.tap.take();
        let undecodable: Vec<_> = events.iter().filter(|e| e.layer == TapLayer::Undecodable).collect();
        assert_eq!(undecodable.len(), 1);
        assert_eq!(undecodable[0].direction, Direction::In);
        assert_eq!(undecodable[0].summary["kind"], "response");
        assert!(events.iter().filter(|e| e.layer == TapLayer::App).all(|e| e.direction == Direction::Out));
    }

    // Its `result: 13` header does not set the all-features-disabled flag; the attempt itself
    // still sets it — to `false`, since the request failed neither with 13 nor with LTW disabled.
    h.messenger.answer_all(result_response(13));
    send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    h.messenger.answer_all(R::Raw(header("13"), vec![0xFF]).into_platform());
    let resp = send(&h, AppRequest::kvp("/a", ValueSet::new())).await;
    assert_eq!((resp.outcome, resp.result), (AppOutcome::Failure, None));
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
}

#[tokio::test(start_paused = true)]
async fn platform_failures_and_rejections() {
    let h = Harness::new();
    use PlatformRequestStatus as S;
    for s in [
        S::NoResponse,
        S::RemoteSyncNotAllowed,
        S::RemoteVersionTooLow,
        S::RemoteVersionTooHigh,
        S::RemoteAllSyncDisabled,
        S::RemoteMeteredSyncDisabled,
        S::RemoteNotSubscribedToRoute,
    ] {
        assert_eq!(outcome_of(&h, R::Status(s.clone())).await, (AppOutcome::Unknown, None), "{s:?}");
    }

    assert_eq!(outcome_of(&h, R::Status(S::ResponseTimedOut)).await.0, AppOutcome::TimedOut);
    assert_eq!(outcome_of(&h, R::Status(S::Canceled)).await.0, AppOutcome::Canceled);
    assert_eq!(
        outcome_of(&h, R::Status(S::RequestSendFailed(DcgSendStatus::PartnerNotConnected))).await.0,
        AppOutcome::SendFailed(DcgSendStatus::PartnerNotConnected)
    );
    assert_eq!(
        outcome_of(
            &h,
            R::Status(S::RequestSendFailed(DcgSendStatus::HubSendFailed {
                result: HubSendResult::InternetError,
                cause: None
            }))
        )
        .await
        .0,
        AppOutcome::InternetError
    );
    assert_eq!(outcome_of(&h, R::Status(S::RequestSendFailed(DcgSendStatus::TimedOut))).await.0, AppOutcome::TimedOut);

    // Success without a message (cannot happen for a request/response send) → unknown.
    assert_eq!(outcome_of(&h, R::Status(S::Success)).await.0, AppOutcome::Unknown);
}

/// Waiting for the connection before a request is sent: the request goes on only when the connect
/// leaves the phone connected; otherwise it fails at once — *timed out* (or *internet error*
/// offline) only when the whole request timeout passed.
#[tokio::test(start_paused = true)]
async fn connection_outcomes_follow_the_connect() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    // A connect that brings the phone up within the timeout; the send gets the rest of the 120 s.
    h.sessions.set_behavior(ConnectBehavior::Delayed(Duration::from_secs(100), Box::new(ConnectBehavior::Connect)));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(resp.outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests()[0].timeout, Duration::from_secs(20));

    // The connect ends (feedback window) without a connection and without a failure: *unknown*
    // at once — the request does not wait for the rest of its timeout.
    h.sessions.set_connected(false);
    h.sessions.set_behavior(ConnectBehavior::Pending);
    let start = tokio::time::Instant::now();
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::Unknown);
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(h.messenger.requests().len(), 1);

    // The phone came up just as the window ended: the request is sent.
    h.sessions.set_behavior(ConnectBehavior::PendingButConnected);
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests().len(), 2);

    // A connect that does not end within the request timeout: timed out; offline → internet error.
    h.sessions.set_connected(false);
    h.sessions.set_behavior(ConnectBehavior::Hang);
    let start = tokio::time::Instant::now();
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::TimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    h.reachability.set(false);
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::InternetError);
    h.reachability.set(true);

    // A connect that ends without a connection only when the whole timeout passed: timed out.
    let unreachable = ConnectionFailure::Wake(WakeStatus::DeviceUnreachable);
    h.sessions.set_behavior(ConnectBehavior::Delayed(
        Duration::from_secs(120),
        Box::new(ConnectBehavior::PendingAfter(unreachable.clone())),
    ));
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::TimedOut);

    // The connection's failure.
    h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::LocalStop));
    assert_eq!(
        send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome,
        AppOutcome::ConnectionFailed(ConnectionFailure::LocalStop)
    );
    h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::Hub(OpenStatus::InternetError)));
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::InternetError);
    assert_eq!(h.messenger.requests().len(), 2);

    // Cancellation while connecting; a caller's cancel leaves the all-features-disabled flag as it
    // was.
    h.app.update_phone_record(|r| r.all_features_disabled = true);
    h.sessions.set_behavior(ConnectBehavior::Hang);
    let cancel = tokio_util::sync::CancellationToken::new();
    let app = h.app.clone();
    let c = cancel.clone();
    let task = tokio::spawn(async move { app.request_cancellable(AppRequest::legacy("/a", ValueSet::new()), c).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    cancel.cancel();
    assert_eq!(task.await.unwrap().unwrap().outcome, AppOutcome::Canceled);
    assert!(h.app.phone_record().unwrap().all_features_disabled);
}

fn all_features_disabled_events(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> Vec<bool> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        if let AppEvent::AllFeaturesDisabled { disabled, .. } = e {
            v.push(disabled);
        }
    }
    v
}

/// A connect that ends without a connection after a failed attempt ends the request with that
/// failure — the connection's state — at once.
#[tokio::test(start_paused = true)]
async fn a_connect_ending_without_a_connection_fails_with_its_last_failure() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    let mut events = h.app.subscribe_events();
    h.sessions.set_behavior(ConnectBehavior::Delayed(
        Duration::from_secs(70),
        Box::new(ConnectBehavior::PendingAfter(ConnectionFailure::Wake(WakeStatus::LtwDisabled))),
    ));

    let start = tokio::time::Instant::now();
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(start.elapsed(), Duration::from_secs(70), "the feedback window, not the 120 s timeout");
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    assert_eq!((resp.result, resp.local_gate), (None, None));
    assert!(h.messenger.requests().is_empty());

    // LTW disabled ⇒ "all features disabled".
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    assert_eq!(all_features_disabled_events(&mut events), [true]);

    // Another failure ends the request the same way and clears the flag; no network → internet error.
    let disconnect = ConnectionFailure::PartnerDisconnect(PartnerDisconnectReason::ExceededMissedHeartbeats);
    h.sessions.set_behavior(ConnectBehavior::PendingAfter(disconnect.clone()));
    assert_eq!(
        send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome,
        AppOutcome::ConnectionFailed(disconnect)
    );
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
    h.sessions.set_behavior(ConnectBehavior::PendingAfter(ConnectionFailure::Hub(OpenStatus::InternetError)));
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::InternetError);
    assert!(h.messenger.requests().is_empty());

    // A user-initiated (NewSession) request ends the same way.
    h.sessions.set_behavior(ConnectBehavior::PendingAfter(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(RequestOptions::user_initiated())).await;
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    assert_eq!(h.sessions.connects().last().unwrap().1.policy, StartPolicy::NewSession);
}

/// A connection the phone ended with `/PlatformDisconnectRequest` reason `ALL_SYNC_DISABLED` fails
/// a waiting request with *LTW disabled* — it sets the "all features disabled" flag — and reason
/// `METERED_SYNC_DISABLED` with *metered sync disabled*.
#[tokio::test(start_paused = true)]
async fn a_session_the_phone_ended_for_sync_reasons_fails_requests_as_ltw_or_metered_disabled() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    let all_sync_disabled = ConnectionFailure::SessionEnded { reason: 2 };
    h.sessions.set_behavior(ConnectBehavior::Fail(all_sync_disabled.clone()));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(all_sync_disabled));
    assert!(resp.outcome.is_ltw_disabled());
    assert!(h.app.phone_record().unwrap().all_features_disabled);

    let metered = ConnectionFailure::SessionEnded { reason: 3 };
    h.sessions.set_behavior(ConnectBehavior::Fail(metered.clone()));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(metered));
    assert!(resp.outcome.is_metered_sync_disabled() && !resp.outcome.is_ltw_disabled());
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
    assert!(h.messenger.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_connect_carries_the_requests_trace_context() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    let trace = TraceContext::new_root(vec![("other".into(), "x".into())]);
    let opts = RequestOptions::default().named("Trigger", "Scenario");
    send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(opts).with_trace(trace.clone())).await;

    let connects = h.sessions.connects();
    assert_eq!(connects.len(), 1);
    let sent = &h.messenger.requests()[0];

    // The connect gets the request's own context — the one the router sends the request with.
    assert_eq!(connects[0].1.trace, sent.trace);
    assert_eq!(connects[0].1.trace.trace_id, trace.trace_id);
    assert_eq!(connects[0].1.trace.correlation_id, trace.correlation_id);
    assert_eq!(connects[0].1.trace.trace_state[0], ("ypTriggerId".to_string(), "Trigger".to_string()));

    // Without a trace of its own: the fresh root context the request is sent with.
    h.sessions.set_connected(false);
    send(&h, AppRequest::legacy("/b", ValueSet::new())).await;
    let connects = h.sessions.connects();
    assert_eq!(connects.len(), 2);
    assert_eq!(connects[1].1.trace, h.messenger.requests()[1].trace);
    assert_ne!(connects[1].1.trace.trace_id, trace.trace_id);
}

#[tokio::test(start_paused = true)]
async fn response_timeout_is_bounded_by_the_request_timeout() {
    let h = Harness::new();
    h.messenger.set_responder(|_| Reply::Hang);
    let start = tokio::time::Instant::now();
    let opts = RequestOptions::default().with_timeout(Duration::from_secs(30));
    let resp = send(&h, AppRequest::legacy("/a", ValueSet::new()).with_options(opts)).await;

    assert_eq!(resp.outcome, AppOutcome::TimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    assert_eq!(resp.result, None);
}

#[tokio::test(start_paused = true)]
async fn every_attempt_sets_the_all_features_disabled_flag() {
    let h = Harness::new();
    h.messenger.answer_all(result_response(13));
    assert_eq!(send(&h, AppRequest::legacy("/a", ValueSet::new())).await.outcome, AppOutcome::AllFeaturesDisabled);
    assert!(h.app.phone_record().unwrap().all_features_disabled);

    h.messenger.answer_all(status_response(PlatformRequestStatus::RequestSendFailed(DcgSendStatus::WakeFailed {
        status: WakeStatus::LtwDisabled,
        cause: None,
    })));
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert!(h.app.phone_record().unwrap().all_features_disabled);

    h.messenger.answer_all(result_response(1));
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
}

struct Observer(Mutex<Vec<(String, AppOutcome, Option<LocalGate>)>>);

impl RequestObserver for Observer {
    fn on_request_completed(&self, phone: &DcgClient, route: &str, response: &AppResponse) {
        assert_eq!(*phone, app_world::phone());
        self.0.lock().unwrap().push((route.to_string(), response.outcome.clone(), response.local_gate));
    }
}

#[tokio::test(start_paused = true)]
async fn observers_see_every_request() {
    let h = Harness::new();
    let obs = Arc::new(Observer(Mutex::new(Vec::new())));
    h.app.register_request_observer(obs.clone());
    send(&h, AppRequest::legacy("/a", ValueSet::new())).await;
    h.lock.set(LockState::Locked);
    send(&h, AppRequest::legacy("/b", ValueSet::new())).await;
    assert_eq!(
        obs.0.lock().unwrap().clone(),
        [
            ("/a".to_string(), AppOutcome::Success, None),
            ("/b".to_string(), AppOutcome::Canceled, Some(LocalGate::Locked))
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn tap_sees_request_and_response() {
    let h = Harness::new();
    send(&h, AppRequest::kvp("/a", ValueSet::new().with("contentType", "x"))).await;

    let events = h.tap.take();
    let app: Vec<_> = events.iter().filter(|e| e.layer == TapLayer::App).collect();
    assert_eq!(app.len(), 2);
    assert_eq!(app[0].direction, Direction::Out);
    assert_eq!(app[0].summary["valueSet"]["contentType"]["String"], "x");
    assert_eq!(app[1].direction, Direction::In);
    assert_eq!(app[1].partner.as_deref(), Some(phone().to_string().as_str()));
}

#[tokio::test(start_paused = true)]
async fn permission_request_keys() {
    let h = Harness::new();
    let req = h.app.permission_request(0x40, false, RequestOptions::user_initiated());
    assert_eq!(req.route, "/permission");
    assert_eq!(req.kind, RequestKind::Kvp);

    let cv = req.trace.as_ref().unwrap().correlation_id.clone();
    send(&h, req).await;
    let sent = &h.messenger.requests()[0];
    assert_eq!(sent.message.route(), Some("/permission"));
    assert_eq!(sent.priority, 0);

    let vs = payload(sent);
    assert_eq!(vs.str("correlationVector"), Some(cv.as_str()));
    assert_eq!(vs.str("contentType"), Some("permission"));
    assert_eq!(vs.get("permissionType"), Some(&Value::Int32(64)));
    assert_eq!(vs.get("silent"), Some(&Value::Bool(false)));

    let session = vs.str("permissionSessionId").unwrap();
    assert_eq!(session, h.app.app_session_id());
    assert!(session.starts_with('{') && session.ends_with('}') && session.len() == 38);
    assert_eq!(session, session.to_uppercase());
    assert_eq!(vs.str("displayName"), Some("test-mac"));
    assert!(!vs.contains_key("transportType"));

    // NotificationsSensitive is sent as Int32 -2147483648; default options are not user-initiated.
    let req = h.app.permission_request(0x8000_0000, true, RequestOptions::default());
    assert_eq!(req.payload.get("permissionType"), Some(&Value::Int32(i32::MIN)));
    send(&h, req).await;
    assert_eq!(h.messenger.requests()[1].priority, 100);
}
