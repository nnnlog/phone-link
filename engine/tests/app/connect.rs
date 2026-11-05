//! The connect message and its sequence.

use crate::support::app_world;
use crate::support::common::payload;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::oneshot;

use app_world::*;
use phonelink_engine::app::{
    AppError, AppEvent, AppOutcome, ConnectContext, ConnectContribution, ConnectContributor, ConnectKind,
    ConnectProvider, ConnectSendResult, ConnectTrigger, LocalGate,
};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::seams::{ConnectionFailure, SessionEvent, StartPolicy};
use phonelink_engine::types::{DcgClient, Environment, MessageType, WakeStatus};

fn is_lower_guid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

fn sent(r: ConnectSendResult) -> phonelink_engine::app::AppResponse {
    match r {
        ConnectSendResult::Sent(resp) => resp,
        other => panic!("connect not sent: {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
#[expect(clippy::too_many_lines, reason = "checks the whole connect message, field by field")]
async fn connect_message_matches_the_derived_example() {
    let h = Harness::new();
    let resp = sent(h.app.send_connect(ConnectKind::Background, "Test").await);
    assert_eq!(resp.outcome, AppOutcome::Success);

    let reqs = h.messenger.requests_on("/connect/v1");
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    assert_eq!(req.target, phone());
    assert_eq!(req.message.message_type, MessageType::App);

    // Envelope headers: `_requestId` (fresh lower-case GUID) then `_route`; the router adds
    // `_rejectionVersion`. No `contentType`/`correlationVector`/`contractVersion` headers.
    let headers: Vec<_> = req.message.headers.iter().collect();
    assert_eq!(headers.len(), 2, "{headers:?}");
    assert_eq!(headers[0].0, "_requestId");
    assert!(is_lower_guid(headers[0].1), "{}", headers[0].1);
    assert_eq!(headers[1], ("_route", "/connect/v1"));

    let vs = payload(req);
    let cv = vs.str("correlationVector").unwrap().to_string();
    assert_eq!(cv.len(), 16);
    assert!(cv.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    let installation_id = h.app.installation_id().to_string();
    assert_eq!(installation_id, installation_id.to_uppercase());
    assert_eq!(installation_id.len(), 36);

    let expected = json!({
        "correlationVector": {"String": cv},
        "contentType": {"String": "connect"},
        "sequenceNumbers": {"Object": {
            "browserHistory": {"Int64": -1},
            "continuityAppContext": {"Int64": -1}
        }},
        "configuration": {"Object": {
            "SequencedContentSyncs": {"Bool": true},
            "SyncSkipMetadataForAuto": {"Bool": false},
            "AndroidCanaryOptInNotifications": {"Bool": false},
            "yppPresenceFlowConfig": {"Bool": false},
            "UnlinkableVersion": {"Int32": 2},
            "btWakeUserEnabled": {"Bool": false},
            "Wallpaper": {"Bool": false},
            "RemoteControlPhoneToPc": {"Bool": true},
            "RecentApps": {"Bool": false},
            "AppHandoff": {"Bool": false},
            "Exp006": {"Bool": true},
            "CallLogs": {"Bool": false},
            "fileTransferToPcRetryEnabled": {"Bool": false},
            "BrowserHistory": {"Bool": false},
            "TaskContinuityVersion": {"UInt32": 1},
            "TaskContinuity": {"Bool": false}
        }},
        "attributes": {"Object": {
            "flightRing": {"String": "Public"},
            "appVersion": {"String": "1.26092.119.0"},
            "displayName": {"String": "test-mac"}
        }},
        "displayName": {"String": "test-mac"},
        "appVersion": {"String": "1.26092.119.0"},
        "installationId": {"String": installation_id},
        "contractVersion": {"Double": 3.99}
    });
    assert_eq!(vs.to_json(), expected);

    // The containers come first, the standard keys last; no TelemetryInfo or legacy keys.
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        [
            "correlationVector",
            "contentType",
            "sequenceNumbers",
            "configuration",
            "attributes",
            "displayName",
            "appVersion",
            "installationId",
            "contractVersion"
        ]
    );

    // The configuration in the key order Phone Link on Windows sends.
    assert_eq!(
        vs.object("configuration").unwrap().keys().collect::<Vec<_>>(),
        [
            "Wallpaper",
            "RemoteControlPhoneToPc",
            "SequencedContentSyncs",
            "SyncSkipMetadataForAuto",
            "AndroidCanaryOptInNotifications",
            "yppPresenceFlowConfig",
            "UnlinkableVersion",
            "RecentApps",
            "AppHandoff",
            "Exp006",
            "CallLogs",
            "fileTransferToPcRetryEnabled",
            "btWakeUserEnabled",
            "BrowserHistory",
            "TaskContinuityVersion",
            "TaskContinuity"
        ]
    );

    // Background request shape: not user-initiated, priority 100, the router bounds the wait.
    assert_eq!(req.priority, 100);
    assert!(!req.options.ignore_device_disabled);
    assert!(req.options.skip_wake_if_not_connected);
    assert_eq!(req.timeout, Duration::from_secs(120));

    // Sent with a child of the trigger's context: ParentId (this context's correlation id) is fresh,
    // its parent is the correlationVector.
    assert_eq!(req.trace.parent_id.as_deref(), Some(cv.as_str()));
    assert_ne!(req.trace.correlation_id, cv);
    assert_eq!(req.trace.flags, 0);
    let state: Vec<(&str, &str)> = req.trace.trace_state.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        state,
        [
            ("ypTriggerId", "Test"),
            ("ypScenarioId", "EstablishSyncSession"),
            ("isuserinitiated", "False"),
            ("isForeground", "False")
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn immediate_connect_is_user_initiated_without_a_wire_key() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    h.app.set_foreground(true);

    sent(h.app.send_connect(ConnectKind::Immediate, "Refresh").await);
    let req = &h.messenger.requests_on("/connect/v1")[0];
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    assert!(!payload(req).contains_key("isuserinitiated"));
    assert!(req.trace.trace_state.contains(&("isuserinitiated".into(), "True".into())));
    assert!(req.trace.trace_state.contains(&("isForeground".into(), "True".into())));

    let connects = h.sessions.connects();
    assert_eq!(connects.len(), 1);
    assert_eq!(connects[0].1.policy, StartPolicy::NewSession);
    assert!(connects[0].1.ignore_device_disabled);
    assert_eq!(connects[0].1.feedback_window, Duration::from_secs(70));
}

#[tokio::test(start_paused = true)]
async fn background_connect_starts_a_default_connect_and_has_600_s() {
    let h = Harness::new();
    h.sessions.set_connected(false);

    // A connect that takes 500 s still fits the background connect's 600 s budget.
    h.sessions.set_behavior(ConnectBehavior::Delayed(Duration::from_secs(500), Box::new(ConnectBehavior::Connect)));
    let resp = sent(h.app.send_connect(ConnectKind::Background, "T").await);
    assert_eq!(resp.outcome, AppOutcome::Success);
    let connects = h.sessions.connects();
    assert_eq!(connects[0].1.policy, StartPolicy::Default);
    assert!(!connects[0].1.ignore_device_disabled);

    // A connect that never ends: timed out after 600 s.
    h.sessions.set_connected(false);
    h.sessions.set_behavior(ConnectBehavior::Hang);
    let started = tokio::time::Instant::now();
    let resp = sent(h.app.send_connect(ConnectKind::Background, "T").await);
    assert_eq!(resp.outcome, AppOutcome::TimedOut);
    assert_eq!(started.elapsed(), Duration::from_secs(600));
}

// ------------------------------------------------------------------------------------------------
// Contributors
// ------------------------------------------------------------------------------------------------

/// Records the order of its calls and what the record held when its response was applied.
struct Recorder {
    provider: ConnectProvider,
    log: Arc<Mutex<Vec<String>>>,
    contribution: ConnectContribution,
}

#[async_trait]
impl ConnectContributor for Recorder {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        assert_eq!(ctx.correlation_vector.len(), 16);
        self.log.lock().unwrap().push(format!("contribute {}", self.provider.number()));
        self.contribution.clone()
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        self.log.lock().unwrap().push(format!(
            "apply {} cv={} bits={:#x} result={:?}",
            self.provider.number(),
            ctx.record.contract_version(&Default::default()),
            ctx.record.capability_bits,
            response.i32("result"),
        ));
    }
}

fn recorder(provider: ConnectProvider, log: &Arc<Mutex<Vec<String>>>, c: ConnectContribution) -> Arc<Recorder> {
    Arc::new(Recorder { provider, log: log.clone(), contribution: c })
}

#[tokio::test(start_paused = true)]
async fn contributors_run_in_provider_order_and_fill_their_slots() {
    let h = Harness::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    let order = [
        ConnectProvider::Contacts,
        ConnectProvider::SyncNotifications,
        ConnectProvider::SyncDeviceStatus,
        ConnectProvider::Messaging,
        ConnectProvider::Photos,
    ];

    for p in order {
        let mut c = ConnectContribution::default();
        c.configuration.insert(format!("{}Flag", p.name()), true);
        c.sequence_numbers.insert(format!("seq{}", p.number()), Value::Int64(i64::from(p.number())));
        if p == ConnectProvider::SyncNotifications {
            c.top_level.insert("operation", 3i32);
            c.top_level.insert("notificationKeys", Value::StringArray(vec!["k".into()]));
        }
        h.app.register_connect_contributor(p, recorder(p, &log, c)).unwrap();
    }

    // A slot takes one contributor.
    assert!(matches!(
        h.app.register_connect_contributor(
            ConnectProvider::Photos,
            recorder(ConnectProvider::Photos, &log, Default::default())
        ),
        Err(AppError::Programming(_))
    ));

    h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32)));
    sent(h.app.send_connect(ConnectKind::Background, "T").await);

    let log = log.lock().unwrap().clone();
    let contributes: Vec<_> = log.iter().filter(|l| l.starts_with("contribute")).cloned().collect();
    assert_eq!(contributes, ["contribute 3", "contribute 9", "contribute 10", "contribute 11", "contribute 14"]);
    let applies: Vec<_> = log.iter().filter(|l| l.starts_with("apply")).map(|l| l[..8].to_string()).collect();
    assert_eq!(applies, ["apply 3 ", "apply 9 ", "apply 10", "apply 11", "apply 14"]);

    let vs = payload(&h.messenger.requests_on("/connect/v1")[0]);
    let conf = vs.object("configuration").unwrap();
    let keys: Vec<_> = conf.keys().collect();
    // Device status before `Wallpaper`; photos, messaging and notifications after
    // `UnlinkableVersion`; contacts after `CallLogs`.
    let pos = |k: &str| keys.iter().position(|x| *x == k).unwrap();
    assert!(pos("SyncDeviceStatusFlag") < pos("Wallpaper"));
    assert!(pos("UnlinkableVersion") < pos("PhotosFlag"));
    assert!(pos("PhotosFlag") < pos("MessagingFlag"));
    assert!(pos("MessagingFlag") < pos("SyncNotificationsFlag"));
    assert!(pos("SyncNotificationsFlag") < pos("RecentApps"));
    assert!(pos("CallLogs") < pos("ContactsFlag"));
    assert!(pos("ContactsFlag") < pos("fileTransferToPcRetryEnabled"));

    let seq = vs.object("sequenceNumbers").unwrap();
    assert_eq!(seq.i64("seq14"), Some(14));
    assert_eq!(seq.i64("browserHistory"), Some(-1));
    assert_eq!(vs.i32("operation"), Some(3));
    assert_eq!(vs.keys().last(), Some("contractVersion"));
}

#[tokio::test(start_paused = true)]
async fn contributions_that_break_the_key_rules_fail_the_connect() {
    for (bad, what) in [
        (
            ConnectContribution { configuration: ValueSet::new().with("Wallpaper", true), ..Default::default() },
            "duplicate",
        ),
        (
            ConnectContribution { sequence_numbers: ValueSet::new().with("sms", 5i32), ..Default::default() },
            "Int32 seq",
        ),
        (ConnectContribution { top_level: ValueSet::new().with("displayName", "x"), ..Default::default() }, "reserved"),
        (
            ConnectContribution { top_level: ValueSet::new().with("isuserinitiated", true), ..Default::default() },
            "reserved",
        ),
        (
            ConnectContribution {
                sequence_numbers: ValueSet::new().with("browserHistory", 4i64),
                ..Default::default()
            },
            "dup seq",
        ),
    ] {
        let h = Harness::new();
        let log = Arc::new(Mutex::new(Vec::new()));
        h.app
            .register_connect_contributor(
                ConnectProvider::SyncDeviceStatus,
                recorder(ConnectProvider::SyncDeviceStatus, &log, bad),
            )
            .unwrap();

        let r = h.app.send_connect(ConnectKind::Immediate, "T").await;
        assert!(matches!(r, ConnectSendResult::Failed(AppError::Programming(_))), "{what}: {r:?}");
        assert!(h.messenger.requests().is_empty(), "{what}");
    }
}

#[tokio::test(start_paused = true)]
async fn response_applies_device_info_first_and_ignores_mistyped_keys() {
    let h = Harness::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    for p in [ConnectProvider::SyncDeviceStatus, ConnectProvider::Contacts] {
        h.app.register_connect_contributor(p, recorder(p, &log, Default::default())).unwrap();
    }

    let response = ValueSet::new()
        .with("result", 0i32)
        .with("contractVersion", 3.5f64)
        .with("yppVersion", "9") // mistyped: ignored
        .with("yppCapabilities", 0x60i32)
        .with("ltwA2DMigrationNeeded", true)
        .with(
            "attributes",
            ValueSet::new()
                .with("platform", 1i32)
                .with("displayName", "Phone")
                .with("assetCategory", "1")
                .with("userId", "user@example.com")
                .with("countryCode", 7i32), // mistyped: ignored
        )
        .with(
            "capabilities",
            ValueSet::new()
                .with("registeredPermissionTypes", 0x801i32)
                .with("contentTransfer", 5i32)
                .with("isTwoPhoneModeEnabled", "yes"), // mistyped: ignored
        )
        .with(
            "permissions",
            ValueSet::new()
                .with("post_notifications", true)
                .with("notification_sensitive", 1i32) // mistyped: ignored
                .with("auto_launch", true),
        )
        .with("userAttributes", ValueSet::new().with("aadcOptionalDataCollectionPolicy", 2i32))
        .with("deviceConfiguration", ValueSet::new().with("ignoringBatteryOptimization", true));

    h.messenger.answer_all(response_with(response));
    let mut events = h.app.subscribe_events();
    sent(h.app.send_connect(ConnectKind::Background, "T").await);

    // Contract version 3.5 <= 3.93 on Android: Calls (0x10) added to the reported bits; the
    // contributors already see the reply's values.
    let log = log.lock().unwrap().clone();
    assert_eq!(
        log.iter().filter(|l| l.starts_with("apply")).cloned().collect::<Vec<_>>(),
        ["apply 3 cv=3.5 bits=0x811 result=Some(0)", "apply 14 cv=3.5 bits=0x811 result=Some(0)"]
    );

    let r = h.app.phone_record().unwrap();
    assert_eq!(r.reported_contract_version, Some(3.5));
    assert_eq!(r.capability_bits, 0x811);
    assert_eq!(r.ypp_version, 8);
    assert_eq!(r.ypp_capabilities, 0x60);
    assert_eq!(r.ltw_a2d_migration_needed, Some(true));
    assert_eq!(r.display_name.as_deref(), Some("Phone"));
    assert_eq!(r.asset_category, 1);
    assert_eq!(r.user_id.as_deref(), Some("user@example.com"));
    assert_eq!(r.country_code, None);
    assert_eq!(r.content_transfer, Some(5));
    assert_eq!(r.two_phone_mode_enabled, None);
    assert_eq!(r.aadc_optional_data_collection_policy, Some(2));
    assert_eq!(r.ignoring_battery_optimization, Some(true));
    assert_eq!(r.permission_bits, 0x800_0000 | 0x1000_0000);

    let mut saw_permissions = false;
    while let Ok(e) = events.try_recv() {
        if let AppEvent::PermissionsChanged { bits, .. } = e {
            saw_permissions = bits == 0x1800_0000;
        }
    }
    assert!(saw_permissions);

    // Mistyped DeviceInfo keys are ignored; the rest applies.
    h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("contractVersion", "4.0")
            .with("capabilities", ValueSet::new().with("registeredPermissionTypes", Value::Int64(1)))
            .with("attributes", 5i32)
            .with("yppCapabilities", 0x20i32),
    ));
    sent(h.app.send_connect(ConnectKind::Background, "T").await);

    let r = h.app.phone_record().unwrap();
    assert_eq!(r.reported_contract_version, Some(3.5));
    assert_eq!(r.capability_bits, 0x811);
    assert_eq!(r.ypp_capabilities, 0x20);
}

#[tokio::test(start_paused = true)]
async fn response_is_applied_whatever_result_and_only_when_it_arrived() {
    let h = Harness::new();
    let log = Arc::new(Mutex::new(Vec::new()));
    h.app
        .register_connect_contributor(
            ConnectProvider::Messaging,
            recorder(ConnectProvider::Messaging, &log, Default::default()),
        )
        .unwrap();

    h.messenger.answer_all(response_with(ValueSet::new().with("result", 13i32).with("contractVersion", 3.9f64)));
    let resp = sent(h.app.send_connect(ConnectKind::Background, "T").await);
    assert_eq!(resp.outcome, AppOutcome::AllFeaturesDisabled);
    assert_eq!(log.lock().unwrap().iter().filter(|l| l.starts_with("apply")).count(), 1);

    let r = h.app.phone_record().unwrap();
    assert!(r.all_features_disabled);
    assert_eq!(r.reported_contract_version, Some(3.9));

    // No response: nothing applied, no retry.
    h.messenger.set_responder(|_| Reply::Hang);
    let resp = sent(h.app.send_connect(ConnectKind::Immediate, "T").await);
    assert_eq!(resp.outcome, AppOutcome::TimedOut);
    assert_eq!(log.lock().unwrap().iter().filter(|l| l.starts_with("apply")).count(), 1);
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(h.messenger.requests_on("/connect/v1").len(), 2);
}

// ------------------------------------------------------------------------------------------------
// Sequence: in-flight drop, cooldown, cancellation, triggers
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn background_connect_is_dropped_while_one_is_in_flight() {
    let h = Harness::new();
    let (tx, rx) = oneshot::channel();
    let mut first = Some(rx);
    h.messenger.set_responder(move |_| match first.take() {
        Some(rx) => Reply::Later(rx),
        None => Reply::Now(result_response(0)),
    });

    let app = h.app.clone();
    let in_flight = tokio::spawn(async move { app.send_connect(ConnectKind::Background, "first").await });
    h.messenger.wait_requests(1).await;

    assert_eq!(h.app.send_connect(ConnectKind::Background, "second").await, ConnectSendResult::Dropped);

    // An immediate connect always runs.
    assert_eq!(sent(h.app.send_connect(ConnectKind::Immediate, "immediate").await).outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests_on("/connect/v1").len(), 2);

    tx.send(result_response(0)).unwrap();
    assert_eq!(sent(in_flight.await.unwrap()).outcome, AppOutcome::Success);

    // The in-flight flag is cleared when the connect ends.
    sent(h.app.send_connect(ConnectKind::Background, "third").await);
    assert_eq!(h.messenger.requests_on("/connect/v1").len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_connect_ending_without_a_connection_ends_the_background_connect_and_frees_its_slot() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    // The platform connect ends after its feedback window without a connection, after an
    // LTW-disabled wake.
    h.sessions.set_behavior(ConnectBehavior::Delayed(
        Duration::from_secs(70),
        Box::new(ConnectBehavior::PendingAfter(ConnectionFailure::Wake(WakeStatus::LtwDisabled))),
    ));

    let mut events = h.app.subscribe_events();
    let app = h.app.clone();
    let start = tokio::time::Instant::now();
    let first = tokio::spawn(async move { app.send_connect(ConnectKind::Background, "first").await });
    tokio::time::sleep(Duration::from_secs(10)).await;

    // While it is connecting, another background connect is dropped.
    assert_eq!(h.app.send_connect(ConnectKind::Background, "second").await, ConnectSendResult::Dropped);
    assert_eq!(h.sessions.connects().len(), 1);

    // The connect ends with that failure at the end of the window, long before its 600 s, and the
    // phone is marked "all features disabled".
    let resp = sent(first.await.unwrap());
    assert_eq!(start.elapsed(), Duration::from_secs(70));
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    assert!(h.messenger.requests().is_empty());

    let mut completed = Vec::new();
    while let Ok(e) = events.try_recv() {
        if let AppEvent::ConnectCompleted { kind, outcome, .. } = e {
            completed.push((kind, outcome));
        }
    }
    assert_eq!(
        completed,
        [(ConnectKind::Background, AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)))]
    );

    // The slot is free: the next background connect is sent, not dropped.
    h.sessions.set_behavior(ConnectBehavior::Connect);
    let resp = sent(h.app.send_connect(ConnectKind::Background, "third").await);
    assert_eq!(resp.outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests_on("/connect/v1").len(), 1);
    assert_eq!(h.sessions.connects().len(), 2);
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
}

#[tokio::test(start_paused = true)]
async fn the_platform_connect_carries_the_connect_requests_trace_context() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    sent(h.app.send_connect(ConnectKind::Background, "T").await);

    let connects = h.sessions.connects();
    assert_eq!(connects.len(), 1);
    let req = &h.messenger.requests_on("/connect/v1")[0];

    // The connect message's own context (a child of the trigger's): the router sends it with it too.
    assert_eq!(connects[0].1.trace, req.trace);
    let cv = payload(req).str("correlationVector").unwrap().to_string();
    assert_eq!(connects[0].1.trace.parent_id.as_deref(), Some(cv.as_str()));
    assert_eq!(connects[0].1.trace.trace_state[0], ("ypTriggerId".to_string(), "T".to_string()));
}

#[tokio::test(start_paused = true)]
async fn became_connected_connects_at_most_once_per_60_s() {
    let h = Harness::new();
    h.app.start().unwrap();
    // Engine start with a linked phone: one background connect.
    h.messenger.wait_requests(1).await;
    assert_eq!(h.messenger.requests_on("/connect/v1")[0].trace.trace_state[0].1, "EngineStart");

    h.sessions.send_event(SessionEvent::Connected(phone()));
    h.messenger.wait_requests(2).await;
    assert_eq!(h.messenger.requests()[1].trace.trace_state[0].1, "PlatformConnected");

    // Another partner, then the phone again inside the cooldown: nothing.
    h.sessions
        .send_event(SessionEvent::Connected(DcgClient::new("11111111-2222-3333-4444-555555555555", Environment::Prod)));
    tokio::time::sleep(Duration::from_secs(30)).await;
    h.sessions.send_event(SessionEvent::Connected(phone()));
    tokio::time::sleep(Duration::from_secs(29)).await;
    assert_eq!(h.messenger.requests().len(), 2);

    // 60 s after the last connect it sent: allowed again.
    tokio::time::sleep(Duration::from_secs(1)).await;
    h.sessions.send_event(SessionEvent::Connected(phone()));
    h.messenger.wait_requests(3).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(h.messenger.requests().len(), 3);

    // A second start does nothing.
    h.app.start().unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(h.messenger.requests().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn disconnect_trigger_cancels_an_in_flight_connect() {
    let h = Harness::new();
    h.messenger.set_responder(|_| Reply::Hang);

    let app = h.app.clone();
    let task = tokio::spawn(async move { app.send_connect(ConnectKind::Background, "T").await });
    h.messenger.wait_requests(1).await;

    h.app.trigger(ConnectTrigger::LockOrSleep);
    let resp = sent(task.await.unwrap());
    assert_eq!(resp.outcome, AppOutcome::Canceled);
    assert!(h.messenger.requests()[0].cancel.is_cancelled());

    // Also while it waits for the connection.
    h.sessions.set_connected(false);
    h.sessions.set_behavior(ConnectBehavior::Hang);
    let app = h.app.clone();
    let task = tokio::spawn(async move { app.send_connect(ConnectKind::Immediate, "T").await });
    tokio::time::sleep(Duration::from_secs(5)).await;
    h.app.cancel_connects();
    assert_eq!(sent(task.await.unwrap()).outcome, AppOutcome::Canceled);

    // Later connects are not affected.
    h.sessions.set_connected(true);
    h.messenger.answer_all(result_response(0));
    assert_eq!(sent(h.app.send_connect(ConnectKind::Background, "T").await).outcome, AppOutcome::Success);
}

#[tokio::test(start_paused = true)]
async fn triggers_map_to_their_kinds() {
    assert_eq!(ConnectTrigger::EngineStart.kind(), Some(ConnectKind::Background));
    assert_eq!(ConnectTrigger::BecameConnected.kind(), Some(ConnectKind::Background));
    assert_eq!(ConnectTrigger::UnlockOrWake.kind(), Some(ConnectKind::Background));
    assert_eq!(ConnectTrigger::SyncOverMeteredEnabled.kind(), Some(ConnectKind::Background));
    assert_eq!(ConnectTrigger::AppActivated.kind(), Some(ConnectKind::Immediate));
    assert_eq!(ConnectTrigger::Refresh.kind(), Some(ConnectKind::Immediate));
    assert_eq!(ConnectTrigger::LockOrSleep.kind(), None);

    let h = started().await;
    h.app.trigger(ConnectTrigger::Refresh);
    h.messenger.wait_requests(2).await;
    assert_eq!(h.messenger.requests()[1].priority, 0);
    h.app.trigger(ConnectTrigger::UnlockOrWake);
    h.messenger.wait_requests(3).await;
    assert_eq!(h.messenger.requests()[2].priority, 100);
}

/// A harness whose App layer has started and sent its engine-start connect.
async fn started() -> Harness {
    let h = Harness::new();
    h.app.start().unwrap();
    h.messenger.wait_requests(1).await;
    assert_eq!(h.messenger.requests()[0].trace.trace_state[0].1, "EngineStart");
    h
}

fn trigger_names(h: &Harness) -> Vec<String> {
    h.messenger.requests_on("/connect/v1").iter().map(|r| r.trace.trace_state[0].1.clone()).collect()
}

/// Connect triggers, as the app runs them: before the App layer starts — while the engine start
/// runs — no trigger sends a connect (a connect built then would miss the providers not yet
/// registered); a coming to the foreground is recorded, and the start sends the engine-start
/// connect and then the immediate connect of "app came to the foreground", built with every
/// provider registered.
#[tokio::test(start_paused = true)]
async fn triggers_before_the_start_send_nothing_and_the_start_sends_the_activation_connect() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    h.app.foreground_changed(true);
    for trigger in [ConnectTrigger::EngineStart, ConnectTrigger::UnlockOrWake, ConnectTrigger::Refresh] {
        h.app.trigger(trigger);
    }

    h.app.update_phone_record(|r| r.status_keys.wifi_state = Some(true));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(h.messenger.requests().is_empty(), "nothing before the start");
    assert!(h.sessions.connects().is_empty());
    assert!(h.app.is_foreground() && !h.app.is_started());

    // A provider registered after those triggers, before the start (as the engine installs the
    // features), is in every connect.
    let recorded: Arc<Mutex<Vec<ConnectKind>>> = Arc::default();
    struct Probe(Arc<Mutex<Vec<ConnectKind>>>);
    #[async_trait]
    impl ConnectContributor for Probe {
        async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
            self.0.lock().unwrap().push(ctx.kind);
            ConnectContribution::default()
        }
        async fn apply(&self, _ctx: &ConnectContext<'_>, _response: &ValueSet) {}
    }

    h.app.register_connect_contributor(ConnectProvider::Contacts, Arc::new(Probe(recorded.clone()))).unwrap();
    h.app.start().unwrap();
    h.messenger.wait_requests(2).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    let mut names = trigger_names(&h);
    names.sort();
    assert_eq!(names, ["AppActivated", "EngineStart"], "one of each, nothing of the triggers before the start");

    let activation =
        h.messenger.requests_on("/connect/v1").into_iter().find(|r| r.trace.trace_state[0].1 == "AppActivated");
    let activation = activation.unwrap();
    assert_eq!(activation.priority, 0, "the immediate connect");
    assert!(activation.trace.trace_state.contains(&("isForeground".into(), "True".into())));

    let mut kinds = recorded.lock().unwrap().clone();
    kinds.sort_by_key(|k| *k == ConnectKind::Immediate);
    assert_eq!(kinds, [ConnectKind::Background, ConnectKind::Immediate], "both built with the provider");

    // Already in the foreground: a report of the same state sends nothing more.
    h.app.foreground_changed(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(h.messenger.requests_on("/connect/v1").len(), 2);
}

/// Started in the background, the layer sends only the engine-start connect; coming to the
/// foreground afterwards sends the activation connect at once, exactly one.
#[tokio::test(start_paused = true)]
async fn a_start_in_the_background_sends_no_activation_connect() {
    let h = started().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(trigger_names(&h), ["EngineStart"]);
    h.app.foreground_changed(true);
    h.messenger.wait_requests(2).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(trigger_names(&h), ["EngineStart", "AppActivated"]);
}

#[tokio::test(start_paused = true)]
async fn background_connect_needs_async_trust_and_no_phone_sends_nothing() {
    // A device-list phone without AsyncTrust: the background connect fails locally.
    let h = Harness::with_custom_data(None);
    let resp = sent(h.app.send_connect(ConnectKind::Background, "T").await);
    assert_eq!(resp.local_gate, Some(LocalGate::AsyncTrust));
    assert_eq!(resp.outcome, AppOutcome::Canceled);
    assert!(h.messenger.requests().is_empty());

    // The immediate connect is user-initiated and goes out; its reply's AsyncTrust opens the gate.
    h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32).with("yppCapabilities", 0x20i32)));
    sent(h.app.send_connect(ConnectKind::Immediate, "T").await);
    assert_eq!(sent(h.app.send_connect(ConnectKind::Background, "T").await).outcome, AppOutcome::Success);
    assert_eq!(h.messenger.requests().len(), 2);

    let h = Harness::without_phone(Default::default(), Default::default());
    assert_eq!(h.app.send_connect(ConnectKind::Immediate, "T").await, ConnectSendResult::NoPhone);
    h.app.start().unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(h.messenger.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn only_sync_over_mobile_data_turning_on_from_off_triggers_a_background_connect() {
    let h = started().await;
    let connects = || h.messenger.requests_on("/connect/v1").len();
    let heartbeat = |extra: ValueSet| phone_content("heartbeat", extra);
    // phoneCommand type 0 (like the device-status keys of every phone-content request) writes
    // sync-over-mobile-data.
    let cmd = |enabled: bool| {
        phone_content(
            "phoneCommand",
            ValueSet::new().with("phoneCommandType", 0i32).with("syncOverMeteredConnection", enabled),
        )
    };

    // Wi-Fi off → on, unknown → on, on → off, and the Wi-Fi reset after a failed request: none
    // is the off → on of the setting.
    h.app.handle_inbound(phone(), trace(), heartbeat(ValueSet::new().with("wifiState", false))).await;
    h.app.handle_inbound(phone(), trace(), heartbeat(ValueSet::new().with("wifiState", true))).await;
    h.app.handle_inbound(phone(), trace(), cmd(true)).await;
    h.app.handle_inbound(phone(), trace(), cmd(false)).await;
    h.app.update_phone_record(|r| r.status_keys.wifi_state = None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(connects(), 1, "only the engine-start connect");

    h.app.handle_inbound(phone(), trace(), heartbeat(ValueSet::new().with("syncOverMeteredConnection", true))).await;
    h.messenger.wait_requests(2).await;
    assert_eq!(connects(), 2);
    assert_eq!(h.messenger.requests()[1].trace.trace_state[0].1, "SyncOverMeteredConnectionEnabled");

    // Unchanged: nothing more.
    h.app.handle_inbound(phone(), trace(), cmd(true)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(connects(), 2);
}

#[tokio::test(start_paused = true)]
async fn connection_failure_with_ltw_disabled_marks_all_features_disabled() {
    let h = Harness::new();
    h.sessions.set_connected(false);
    h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    let mut events = h.app.subscribe_events();
    let resp = sent(h.app.send_connect(ConnectKind::Immediate, "T").await);
    assert_eq!(resp.outcome, AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));
    assert!(h.app.phone_record().unwrap().all_features_disabled);

    let mut saw = false;
    while let Ok(e) = events.try_recv() {
        saw |= matches!(e, AppEvent::AllFeaturesDisabled { disabled: true, .. });
    }
    assert!(saw);

    // The next successful response clears it.
    h.sessions.set_connected(true);
    sent(h.app.send_connect(ConnectKind::Immediate, "T").await);
    assert!(!h.app.phone_record().unwrap().all_features_disabled);
}

/// Connect triggers: the app coming to the foreground sends the immediate connect (user-initiated,
/// priority `0`, `NewSession`), with `isForeground` `True` in its trace state; leaving the
/// foreground sends nothing and later requests carry `isForeground` `False`; reporting the state
/// the app is already in sends nothing.
#[tokio::test(start_paused = true)]
async fn coming_to_the_foreground_sends_the_immediate_connect() {
    let h = started().await;
    h.sessions.set_connected(false);
    h.app.foreground_changed(true);
    h.messenger.wait_requests(2).await;

    let req = &h.messenger.requests_on("/connect/v1")[1];
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    let state: Vec<(&str, &str)> = req.trace.trace_state.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        state,
        [
            ("ypTriggerId", "AppActivated"),
            ("ypScenarioId", "EstablishSyncSession"),
            ("isuserinitiated", "True"),
            ("isForeground", "True")
        ]
    );

    assert_eq!(h.sessions.connects()[0].1.policy, StartPolicy::NewSession);
    assert!(h.app.is_foreground());

    // Already in the foreground: nothing.
    h.app.foreground_changed(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(h.messenger.requests().len(), 2);

    // Leaving: nothing sent; the next request says so.
    h.app.foreground_changed(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(h.messenger.requests().len(), 2);
    assert!(!h.app.is_foreground());

    h.app.trigger(ConnectTrigger::Refresh);
    h.messenger.wait_requests(3).await;
    assert!(h.messenger.requests()[2].trace.trace_state.contains(&("isForeground".into(), "False".into())));
}
