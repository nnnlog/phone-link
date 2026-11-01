//! Inbound App requests: dispatch, unknown or unsupported inbound traffic, the engine's rejection of
//! malformed content, and the shared App messages.

use crate::support::app_world;
use crate::support::common::eventually;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;

use app_world::*;
use phonelink_engine::app::shared::RemotingUri;
use phonelink_engine::app::{
    AppError, AppEvent, ContentProcessor, ContentProcessorFactory, DropReason, InboundContext, InboundDisposition,
    Processed, Rejection, rejection_message,
};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::edge::{LockState, SecureStore};
use phonelink_engine::seams::AppInbound;
use phonelink_engine::tap::{Direction, TapLayer};
use phonelink_engine::types::{DcgClient, Environment, MessageType, TransportMessage};

async fn handle(h: &Harness, msg: TransportMessage) -> InboundDisposition {
    h.app.handle_inbound(phone(), trace(), msg).await
}

fn answered(d: InboundDisposition) -> ValueSet {
    match d {
        InboundDisposition::Answered(v) => v,
        other => panic!("not answered: {other:?}"),
    }
}

fn simple(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

/// `/legacy/phonecontent` with `_requestId: req-1` and exactly `vs` as the payload.
fn pc(vs: ValueSet) -> TransportMessage {
    message(&[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs))
}

fn pc_with_headers(headers: &[(&str, &str)], vs: ValueSet) -> TransportMessage {
    message(headers, valueset::encode(&vs))
}

fn base(content_type: &str) -> ValueSet {
    ValueSet::new().with("contentType", content_type).with("contractVersion", 3.99f64).with("correlationVector", "cv-1")
}

fn with(mut vs: ValueSet, extra: ValueSet) -> ValueSet {
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    vs
}

fn events(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> Vec<AppEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

// ------------------------------------------------------------------------------------------------
// heartbeat, response construction
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn heartbeat_is_answered_0_with_the_response_envelope() {
    let h = Harness::new();
    let t = trace();
    let d = h.app.handle_inbound(phone(), t.clone(), phone_content("heartbeat", ValueSet::new())).await;
    assert_eq!(answered(d), simple(0));

    let r = h.messenger.responses().pop().unwrap();
    assert_eq!(r.values(), simple(0));
    let headers: Vec<_> = r.response.headers.iter().collect();
    assert_eq!(headers, [("_originalRequestId", "req-1"), ("_route", "/internal/response"), ("result", "0")]);
    assert_eq!(r.response.message_type, MessageType::App);
    assert_eq!(r.priority, 50);
    assert_eq!(r.target, phone());
    assert_eq!(r.request.headers.get("_requestId"), Some("req-1"));
    assert_eq!(r.trace, t);

    // The tap saw the request and the answer.
    let app: Vec<_> = h.tap.take().into_iter().filter(|e| e.layer == TapLayer::App).collect();
    assert_eq!(app.iter().map(|e| e.direction).collect::<Vec<_>>(), [Direction::In, Direction::Out]);
}

#[tokio::test(start_paused = true)]
async fn heartbeat_with_metadata_is_rejected() {
    let h = Harness::new();
    let d = handle(&h, phone_content("heartbeat", ValueSet::new().with("metadata", Value::Empty))).await;
    assert_eq!(answered(d), simple(1));
    assert_eq!(h.messenger.responses()[0].header("result").as_deref(), Some("1"));
}

// ------------------------------------------------------------------------------------------------
// permissions
// ------------------------------------------------------------------------------------------------

fn perms(types: &[i32], states: &[i32]) -> ValueSet {
    ValueSet::new()
        .with("permissionTypes", Value::Int32Array(types.to_vec()))
        .with("permissionStates", Value::Int32Array(states.to_vec()))
}

#[tokio::test(start_paused = true)]
async fn permissions_content_sets_bits_and_completes_on_the_pair_count() {
    let h = Harness::new();
    h.app.update_phone_record(|r| r.permission_bits = 0x2);
    let mut rx = h.app.subscribe_events();

    // Without itemCount (0) but with 2 pairs: applied, answered 0, the processor stays stored.
    let d = handle(&h, phone_content("permissions", perms(&[0x1, 0x2], &[0, 1]))).await;
    assert_eq!(answered(d), simple(0));
    assert_eq!(h.app.phone_record().unwrap().permission_bits, 0x1);
    assert!(events(&mut rx).contains(&AppEvent::PermissionsChanged { phone: phone(), bits: 0x1 }));

    // A message under the same key goes to the stored processor whatever its contentType: no arrays → 14.
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(14));

    // Zero pairs: count 0 == itemCount 0 → completed.
    assert_eq!(answered(handle(&h, phone_content("permissions", perms(&[], &[]))).await), simple(0));

    // Gone: a heartbeat under the key is a heartbeat again.
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));
}

#[tokio::test(start_paused = true)]
async fn permissions_metadata_answer_and_transaction() {
    let h = Harness::new();
    let d =
        handle(&h, phone_content("permissions", ValueSet::new().with("metadata", true).with("itemCount", 2i32))).await;
    let expected = ValueSet::new()
        .with("correlationVector", "cv-1")
        .with("contentType", "Permissions")
        .with("contractVersion", "1.0")
        .with("itemCount", 0i32)
        .with("result", 1i32);
    assert_eq!(answered(d), expected);

    // With the `result` header: an `Int32` `result` without the header is no answer Phone Link on
    // Windows sends.
    assert_eq!(h.messenger.responses()[0].header("result").as_deref(), Some("1"));

    // Unequal arrays: 14, the count is kept, the transaction stays open.
    assert_eq!(answered(handle(&h, phone_content("permissions", perms(&[1, 2], &[0]))).await), simple(14));

    // Mistyped arrays: 14.
    let mistyped = ValueSet::new()
        .with("permissionTypes", Value::Int64Array(vec![1, 2]))
        .with("permissionStates", Value::Int32Array(vec![0, 0]));
    assert_eq!(answered(handle(&h, phone_content("permissions", mistyped)).await), simple(14));

    // Two pairs: count 2 == itemCount 2 → completed.
    assert_eq!(answered(handle(&h, phone_content("permissions", perms(&[0x800, 0x40], &[0, 0]))).await), simple(0));
    assert_eq!(h.app.phone_record().unwrap().permission_bits, 0x840);
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));

    // itemCount 0: result 0 and completed at once.
    let d = handle(&h, phone_content("permissions", ValueSet::new().with("metadata", true))).await;
    assert_eq!(answered(d).i32("result"), Some(0));
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));
}

#[tokio::test(start_paused = true)]
async fn permissions_edge_cases() {
    let h = Harness::new();
    // A mistyped itemCount is rejected (1) and stores nothing.
    let d =
        handle(&h, phone_content("permissions", ValueSet::new().with("metadata", true).with("itemCount", "2"))).await;
    assert_eq!(answered(d), simple(1));
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));

    // Without a correlationVector the metadata answer omits it.
    let vs = ValueSet::new().with("contentType", "permissions").with("contractVersion", 3.99f64).with("metadata", true);
    let a = answered(handle(&h, pc(vs)).await);
    assert!(!a.contains_key("correlationVector"));
    assert_eq!(a.str("contentType"), Some("Permissions"));

    // NotificationsSensitive arrives as Int32 -2147483648.
    assert_eq!(answered(handle(&h, phone_content("permissions", perms(&[i32::MIN], &[0]))).await), simple(0));
    assert!(h.app.phone_record().unwrap().has_permission(0x8000_0000));
}

// ------------------------------------------------------------------------------------------------
// phoneCommand
// ------------------------------------------------------------------------------------------------

fn cmd(t: i32, extra: ValueSet) -> TransportMessage {
    phone_content("phoneCommand", with(ValueSet::new().with("phoneCommandType", t), extra))
}

#[tokio::test(start_paused = true)]
async fn phone_command_types() {
    let h = Harness::new();
    let mut rx = h.app.subscribe_events();
    let status = |h: &Harness| h.app.phone_record().unwrap().status_keys.sync_over_metered_connection;

    // 0: the sync-over-mobile-data setting; absent → enabled; mistyped → rejected, nothing applied.
    assert_eq!(answered(handle(&h, cmd(0, ValueSet::new())).await), simple(0));
    assert_eq!(status(&h), Some(true));
    assert_eq!(answered(handle(&h, cmd(0, ValueSet::new().with("syncOverMeteredConnection", false))).await), simple(0));
    assert_eq!(status(&h), Some(false));
    assert_eq!(answered(handle(&h, cmd(0, ValueSet::new().with("syncOverMeteredConnection", 1i32))).await), simple(1));
    assert_eq!(status(&h), Some(false));

    // 1: LaunchUri.
    assert_eq!(answered(handle(&h, cmd(1, ValueSet::new().with("launchURI", "MS-Phone:open?x=1"))).await), simple(0));
    assert!(events(&mut rx).contains(&AppEvent::BringWindowForward { uri: "MS-Phone:open?x=1".into() }));

    for bad in [
        ValueSet::new().with("launchURI", "https://example.com"),
        ValueSet::new().with("launchURI", "relative"),
        ValueSet::new(),
    ] {
        assert_eq!(answered(handle(&h, cmd(1, bad)).await), simple(1));
    }
    assert_eq!(answered(handle(&h, cmd(1, ValueSet::new().with("launchURI", 5i32))).await), simple(1));
    assert!(!events(&mut rx).iter().any(|e| matches!(e, AppEvent::BringWindowForward { .. })));

    // 2: SyncDisabled.
    assert_eq!(answered(handle(&h, cmd(2, ValueSet::new())).await), simple(0));
    assert!(h.app.phone_record().unwrap().all_features_disabled);
    assert!(events(&mut rx).contains(&AppEvent::AllFeaturesDisabled { phone: phone(), disabled: true }));

    // 3: the hotspot facts each key carries are recorded; a mistyped one is rejected, nothing applied.
    let hotspot = ValueSet::new().with("hotspotUserConsent", true).with("oemBluetoothPermission", false);
    assert_eq!(answered(handle(&h, cmd(3, hotspot)).await), simple(0));
    let record = h.app.phone_record().unwrap().hotspot;
    assert!(record.user_consent && !record.oem_bluetooth_permission && !record.oem_rfcomm_enabled);
    assert_eq!(answered(handle(&h, cmd(3, ValueSet::new().with("oemBluetoothRfcommEnabled", true))).await), simple(0));
    assert!(h.app.phone_record().unwrap().hotspot.oem_rfcomm_enabled);
    let mistyped = ValueSet::new().with("hotspotUserConsent", false).with("oemBluetoothRfcommEnabled", "on");
    assert_eq!(answered(handle(&h, cmd(3, mistyped)).await), simple(1));
    assert!(h.app.phone_record().unwrap().hotspot.user_consent);

    // 4: PostNotifications.
    let post = |s: &str| cmd(4, ValueSet::new().with("postNotificationSettingScenario", s));
    assert_eq!(answered(handle(&h, post("post_notification_permission_granted")).await), simple(0));
    assert!(h.app.phone_record().unwrap().has_permission(0x800_0000));
    assert_eq!(answered(handle(&h, post("something_else")).await), simple(0));
    assert!(h.app.phone_record().unwrap().has_permission(0x800_0000));
    assert_eq!(answered(handle(&h, post("post_notification_permission_denied")).await), simple(0));
    assert!(!h.app.phone_record().unwrap().has_permission(0x800_0000));
    assert_eq!(
        answered(handle(&h, cmd(4, ValueSet::new().with("postNotificationSettingScenario", true))).await),
        simple(1)
    );

    // 5, 6: logged only; 7: NotificationsSensitive; other and missing types: nothing.
    assert_eq!(answered(handle(&h, cmd(5, ValueSet::new())).await), simple(0));
    assert_eq!(answered(handle(&h, cmd(6, ValueSet::new())).await), simple(0));

    let sens = |s: &str| cmd(7, ValueSet::new().with("notificationsSensitivePermissionScenario", s));
    assert_eq!(answered(handle(&h, sens("notifications_sensitive_permission_granted")).await), simple(0));
    assert!(h.app.phone_record().unwrap().has_permission(0x8000_0000));
    assert_eq!(answered(handle(&h, sens("notifications_sensitive_permission_denied")).await), simple(0));
    assert!(!h.app.phone_record().unwrap().has_permission(0x8000_0000));
    assert_eq!(answered(handle(&h, cmd(99, ValueSet::new())).await), simple(0));
    assert_eq!(answered(handle(&h, phone_content("phoneCommand", ValueSet::new())).await), simple(0));
}

#[tokio::test(start_paused = true)]
async fn a_remoting_launch_uri_goes_to_screen_mirroring_unless_it_names_another_phone() {
    let h = Harness::new();
    let mut rx = h.app.subscribe_events();
    let launch = |uri: &str| cmd(1, ValueSet::new().with("launchURI", uri));
    let mirror = "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen&cid=6f1c0d5e";

    assert_eq!(answered(handle(&h, launch(mirror)).await), simple(0));
    let expected = RemotingUri {
        device_id: None,
        package_name: Some("yourphone.phonescreen".into()),
        launch_source: Some("protocol".into()),
    };
    assert_eq!(events(&mut rx), vec![AppEvent::RemotingLaunch { phone: phone(), uri: expected }]);

    // The selected phone named by id (in any case) is the same launch.
    let own = format!("ms-phone:remoting?deviceid={}&packagename=com.example.app", phone().id.to_uppercase());
    assert_eq!(answered(handle(&h, launch(&own)).await), simple(0));
    assert!(matches!(events(&mut rx).as_slice(), [AppEvent::RemotingLaunch { .. }]));

    // Another phone: answered, nothing launched.
    assert_eq!(answered(handle(&h, launch("ms-phone:remoting?deviceid=other&packagename=x")).await), simple(0));
    assert_eq!(events(&mut rx), vec![]);

    // An authority makes it another ms-phone URI.
    assert_eq!(answered(handle(&h, launch("ms-phone://remoting?packagename=x")).await), simple(0));
    assert_eq!(events(&mut rx), vec![AppEvent::BringWindowForward { uri: "ms-phone://remoting?packagename=x".into() }]);
}

#[tokio::test(start_paused = true)]
async fn phone_command_requires_its_correlation_vector_and_typed_keys() {
    let h = Harness::new();
    let no_cv = ValueSet::new()
        .with("contentType", "phoneCommand")
        .with("contractVersion", 3.99f64)
        .with("phoneCommandType", 2i32);
    assert_eq!(answered(handle(&h, pc(no_cv.clone())).await), simple(1));
    assert!(!h.app.phone_record().unwrap().all_features_disabled);

    // The header's correlationVector is copied into the payload.
    let d = handle(
        &h,
        pc_with_headers(
            &[("_requestId", "req-1"), ("_route", "/legacy/phonecontent"), ("correlationVector", "hdr-cv")],
            no_cv.clone(),
        ),
    )
    .await;
    assert_eq!(answered(d), simple(0));
    assert!(h.app.phone_record().unwrap().all_features_disabled);

    // An empty header is not copied: still no correlationVector, so rejected.
    h.app.update_phone_record(|r| r.all_features_disabled = false);
    let d = handle(
        &h,
        pc_with_headers(
            &[("_requestId", "req-1"), ("_route", "/legacy/phonecontent"), ("correlationVector", "")],
            no_cv.clone(),
        ),
    )
    .await;
    assert_eq!(answered(d), simple(1));
    assert!(!h.app.phone_record().unwrap().all_features_disabled);

    // Mistyped correlationVector or phoneCommandType.
    assert_eq!(
        answered(handle(&h, pc(with(no_cv.clone(), ValueSet::new().with("correlationVector", 1i32)))).await),
        simple(1)
    );
    assert_eq!(
        answered(handle(&h, phone_content("phoneCommand", ValueSet::new().with("phoneCommandType", "2"))).await),
        simple(1)
    );

    // Metadata phase: the PC's answer (no result header); type 0 applied at creation (absent →
    // enabled); completed.
    assert_eq!(h.app.phone_record().unwrap().status_keys.sync_over_metered_connection, None);
    let d = handle(&h, cmd(0, ValueSet::new().with("metadata", 1i32))).await;
    let expected = ValueSet::new()
        .with("correlationVector", "cv-1")
        .with("contentType", "phoneCommand")
        .with("contractVersion", 3.99f64);
    assert_eq!(answered(d), expected);
    assert_eq!(h.messenger.responses().last().unwrap().header("result"), None);
    assert_eq!(h.app.phone_record().unwrap().status_keys.sync_over_metered_connection, Some(true));
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));

    // Metadata without correlationVector: rejected.
    assert_eq!(answered(handle(&h, pc(with(no_cv, ValueSet::new().with("metadata", true)))).await), simple(1));
}

// ------------------------------------------------------------------------------------------------
// deviceQuery, unknown types
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn device_query_is_answered_empty_without_a_result_header() {
    let h = Harness::new();
    for extra in [ValueSet::new(), ValueSet::new().with("metadata", true)] {
        assert_eq!(answered(handle(&h, phone_content("deviceQuery", extra)).await), ValueSet::new());
        let r = h.messenger.responses().pop().unwrap();
        assert_eq!(r.header("result"), None);
        assert_eq!(r.response.headers.len(), 2);
        assert!(r.response.payload.is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn types_without_a_processor_get_2_for_metadata_and_1_for_content() {
    let h = Harness::new();
    for t in ["wallpaper", "phoneApps", "copypaste_metadata", "notifications", "foo", "Heartbeat", "connect", "test"] {
        assert_eq!(
            answered(handle(&h, phone_content(t, ValueSet::new().with("metadata", true))).await),
            simple(2),
            "{t}"
        );
        assert_eq!(answered(handle(&h, phone_content(t, ValueSet::new())).await), simple(1), "{t}");
    }

    // No contentType at all.
    let vs = ValueSet::new().with("contractVersion", 3.99f64);
    assert_eq!(answered(handle(&h, pc(vs.clone())).await), simple(1));
    assert_eq!(answered(handle(&h, pc(vs.with("metadata", true))).await), simple(2));
}

// ------------------------------------------------------------------------------------------------
// Contract version, lock
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn contract_version_rules() {
    let h = Harness::new();
    let hb = |headers: &[(&str, &str)], cv: Option<f64>| {
        let mut vs = ValueSet::new().with("contentType", "heartbeat");
        if let Some(cv) = cv {
            vs.insert("contractVersion", cv);
        }
        let mut all = vec![("_requestId", "req-1"), ("_route", "/legacy/phonecontent")];
        all.extend_from_slice(headers);
        pc_with_headers(&all, vs)
    };

    assert_eq!(answered(handle(&h, hb(&[], Some(3.0))).await), simple(0));
    assert_eq!(answered(handle(&h, hb(&[], Some(3.5))).await), simple(0));
    assert_eq!(answered(handle(&h, hb(&[], Some(4.0))).await), simple(3));
    assert_eq!(answered(handle(&h, hb(&[], Some(2.99))).await), simple(3));
    assert_eq!(answered(handle(&h, hb(&[], None)).await), simple(3));
    assert_eq!(answered(handle(&h, hb(&[], Some(-1.0))).await), simple(0));

    // The header wins over the payload; a non-numeric header reads as missing.
    assert_eq!(answered(handle(&h, hb(&[("contractVersion", "3.0")], Some(4.0))).await), simple(0));
    assert_eq!(answered(handle(&h, hb(&[("contractVersion", "4.2")], Some(3.99))).await), simple(3));
    assert_eq!(answered(handle(&h, hb(&[("contractVersion", "abc")], Some(3.99))).await), simple(3));
    assert_eq!(answered(handle(&h, hb(&[("contractVersion", "-1")], None)).await), simple(0));

    // A payload contractVersion of another type is not lifted: missing → 3.
    let vs = ValueSet::new().with("contentType", "heartbeat").with("contractVersion", "3.99");
    assert_eq!(answered(handle(&h, pc(vs)).await), simple(3));
    assert_eq!(h.messenger.responses().last().unwrap().header("result").as_deref(), Some("3"));
}

#[tokio::test(start_paused = true)]
async fn locked_session_answers_19_from_contract_3_8_or_minus_1() {
    let h = Harness::new();
    for state in [LockState::Locked, LockState::Unknown] {
        h.lock.set(state);
        let d = handle(&h, phone_content("heartbeat", ValueSet::new().with("batteryPercent", 40i32))).await;
        assert_eq!(answered(d), simple(19), "{state:?}");
    }

    let hb = |cv: f64| pc(ValueSet::new().with("contentType", "heartbeat").with("contractVersion", cv));
    assert_eq!(answered(handle(&h, hb(-1.0)).await), simple(19));
    assert_eq!(answered(handle(&h, hb(3.8)).await), simple(19));

    // Below 3.8 the message is processed normally.
    assert_eq!(answered(handle(&h, hb(3.5)).await), simple(0));

    // Answered 3 / 19 before the device-status keys are read: nothing recorded.
    assert_eq!(h.app.phone_record().unwrap().status_keys.battery_percent, None);
    h.lock.set(LockState::Unlocked);
    assert_eq!(answered(handle(&h, hb(3.99)).await), simple(0));
}

// ------------------------------------------------------------------------------------------------
// Unparseable payloads, routes, request ids
// ------------------------------------------------------------------------------------------------

/// A `ValueSet` with one entry whose `value_type` is 40 (unknown).
fn unknown_value_type() -> Vec<u8> {
    vec![0x0A, 0x07, 0x0A, 0x01, b'k', 0x12, 0x02, 0x08, 40]
}

#[tokio::test(start_paused = true)]
async fn unparseable_payloads() {
    let h = Harness::new();
    let ok_headers = [("_requestId", "req-1"), ("_route", "/legacy/phonecontent")];
    for bytes in [vec![0xFF], unknown_value_type(), b"not gzip".to_vec()] {
        let d = handle(&h, message(&ok_headers, bytes.clone())).await;
        assert_eq!(answered(d), simple(1));
    }

    // Gzip-flagged but not gzip.
    let d = handle(&h, message(&[ok_headers[0], ok_headers[1], ("_isGzippedCompressed", "true")], vec![1, 2, 3])).await;
    assert_eq!(answered(d), simple(1));
    assert_eq!(h.messenger.responses().len(), 4);
    assert!(h.messenger.responses().iter().all(|r| r.header("result").as_deref() == Some("1")));

    // Without `_requestId` or on another route: silence.
    let d = handle(&h, message(&[("_route", "/legacy/phonecontent")], vec![0xFF])).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::Unparseable));
    let d = handle(&h, message(&[("_requestId", "r"), ("_route", "/foo")], vec![0xFF])).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::Unparseable));
    assert_eq!(h.messenger.responses().len(), 4);
    assert!(h.tap.take().iter().any(|e| e.layer == TapLayer::Undecodable));
}

#[tokio::test(start_paused = true)]
async fn routes_other_than_phone_content_get_no_answer() {
    let h = Harness::new();
    let hb = base("heartbeat");
    for r in ["/foo", "", "/connect/v1"] {
        let d = handle(&h, pc_with_headers(&[("_requestId", "req-1"), ("_route", r)], hb.clone())).await;
        assert_eq!(d, InboundDisposition::Dropped(DropReason::OtherRoute), "{r:?}");
    }

    // A response recognizable only from its payload is dropped too.
    let d =
        handle(&h, pc_with_headers(&[("_requestId", "req-1")], hb.clone().with("_route", "/internal/response"))).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::OtherRoute));

    // No route anywhere.
    let d = handle(&h, pc_with_headers(&[("_requestId", "req-1")], hb.clone())).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::NoRoute));
    assert!(h.messenger.responses().is_empty());

    // The route only in the payload is lifted.
    let d = handle(&h, pc_with_headers(&[("_requestId", "req-1")], hb.with("_route", "/legacy/phonecontent"))).await;
    assert_eq!(answered(d), simple(0));
}

#[tokio::test(start_paused = true)]
async fn missing_request_id_applies_nothing_and_payload_request_id_is_lifted() {
    let h = Harness::new();
    let vs = with(base("permissions"), perms(&[1], &[0])).with("batteryPercent", 50i32);
    let d = handle(&h, pc_with_headers(&[("_route", "/legacy/phonecontent")], vs.clone())).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::NoRequestId));

    let r = h.app.phone_record().unwrap();
    assert_eq!(r.permission_bits, 0);
    assert_eq!(r.status_keys.battery_percent, None);
    assert!(h.messenger.responses().is_empty());

    // `_requestId` only in the payload: answered, and the router sees it as a header.
    let d =
        handle(&h, pc_with_headers(&[("_route", "/legacy/phonecontent")], vs.with("_requestId", "payload-id"))).await;
    assert_eq!(answered(d), simple(0));
    let resp = h.messenger.responses().pop().unwrap();
    assert_eq!(resp.request.headers.get("_requestId"), Some("payload-id"));
    assert_eq!(resp.header("_originalRequestId").as_deref(), Some("payload-id"));
    assert_eq!(h.app.phone_record().unwrap().permission_bits, 1);
}

#[tokio::test(start_paused = true)]
async fn gzip_payload_and_header_lift() {
    let h = Harness::new();
    let bytes = gzip(&valueset::encode(&base("heartbeat")));
    let d = handle(
        &h,
        message(&[("_requestId", "r"), ("_route", "/legacy/phonecontent"), ("_isGzippedCompressed", "TRUE")], bytes),
    )
    .await;
    assert_eq!(answered(d), simple(0));

    // The contentType header wins over the payload's: a heartbeat, not permissions (which would say 14).
    let d = handle(
        &h,
        pc_with_headers(
            &[("_requestId", "r"), ("_route", "/legacy/phonecontent"), ("contentType", "heartbeat")],
            base("permissions"),
        ),
    )
    .await;
    assert_eq!(answered(d), simple(0));
}

// ------------------------------------------------------------------------------------------------
// Device-status keys
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn device_status_keys_are_recorded_from_every_request_with_type_checks() {
    let h = Harness::new();
    let keys = ValueSet::new()
        .with("batteryPercent", 55i32)
        .with("batteryCharging", true)
        .with("batteryPowerSave", false)
        .with("bluetoothState", 12i32)
        .with("doNotDisturbStatus", 1i32)
        .with("doNotDisturbRuleEnabled", false)
        .with("networkOperator", "Carrier")
        .with("networkSignalStrength", 3i32)
        .with("networkSignalStrengthMax", 5i32)
        .with("networkIndicatorApiSupport", true)
        .with("mobileDataStatus", 1i32)
        .with("syncOverMeteredConnection", false)
        .with("ringerMode", 2i32)
        .with("wifiSignalLevel", 4i32)
        .with("wifiState", false)
        .with("airplaneMode", false);

    // An unknown contentType still records (and is answered 1).
    assert_eq!(answered(handle(&h, phone_content("foo", keys)).await), simple(1));
    let s = h.app.phone_record().unwrap().status_keys;
    assert_eq!(
        (s.battery_percent, s.battery_charging, s.battery_power_save, s.bluetooth_state, s.do_not_disturb_status),
        (Some(55), Some(true), Some(false), Some(12), Some(1))
    );
    assert_eq!((s.do_not_disturb_rule_enabled, s.network_operator.as_deref()), (Some(false), Some("Carrier")));
    assert_eq!(
        (
            s.network_signal_strength,
            s.network_signal_strength_max,
            s.network_indicator_api_support,
            s.mobile_data_status
        ),
        (Some(3), Some(5), Some(true), Some(1))
    );
    assert_eq!(
        (s.sync_over_metered_connection, s.ringer_mode, s.wifi_signal_level, s.wifi_state, s.airplane_mode),
        (Some(false), Some(2), Some(4), Some(false), Some(false))
    );

    // Mistyped keys are ignored, the rest recorded; `batteryPowersave` is not a step-6 key.
    let more = ValueSet::new()
        .with("batteryPercent", "80")
        .with("ringerMode", Value::Int64(0))
        .with("batteryPowersave", true)
        .with("wifiSignalLevel", 2i32);
    assert_eq!(answered(handle(&h, phone_content("heartbeat", more)).await), simple(0));
    let s = h.app.phone_record().unwrap().status_keys;
    assert_eq!(
        (s.battery_percent, s.ringer_mode, s.battery_power_save, s.wifi_signal_level),
        (Some(55), Some(2), Some(false), Some(2))
    );

    // Persisted with the record.
    let stored = h.store.load(format!("app/phone/{PHONE_ID}.Prod")).unwrap().unwrap();
    assert!(String::from_utf8(stored).unwrap().contains("\"wifi_signal_level\":2"));
}

#[tokio::test(start_paused = true)]
async fn a_rejected_message_still_records_its_device_status_keys() {
    let h = Harness::new();
    // `heartbeat` has no metadata phase: a heartbeat with `metadata` is rejected (answered 1) …
    // (`networkOperator` is mistyped: ignored)
    let hb = ValueSet::new().with("metadata", Value::Empty).with("batteryPercent", 77i32).with("networkOperator", 5i32);
    assert_eq!(answered(handle(&h, phone_content("heartbeat", hb)).await), simple(1));

    // … but the device-status keys are read before any processor runs: its device-status key is recorded all the same.
    let s = h.app.phone_record().unwrap().status_keys;
    assert_eq!((s.battery_percent, s.network_operator), (Some(77), None));

    // A message refused while its processor is created (`permissions` with a String `itemCount`).
    let perm = ValueSet::new().with("itemCount", "1").with("ringerMode", 2i32);
    assert_eq!(answered(handle(&h, phone_content("permissions", perm)).await), simple(1));
    let r = h.app.phone_record().unwrap();
    assert_eq!(r.status_keys.ringer_mode, Some(2));
    assert_eq!(r.permission_bits, 0);
}

// ------------------------------------------------------------------------------------------------
// Transactions and the rejection rule (a test processor)
// ------------------------------------------------------------------------------------------------

struct TestFactory {
    created: Arc<AtomicUsize>,
    none: bool,
}

impl ContentProcessorFactory for TestFactory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        assert_eq!(*ctx.phone, phone());
        if payload.contains_key("rejectAtCreate") {
            return Err(Rejection::new("rejected at create"));
        }

        if self.none {
            return Ok(None);
        }

        let id = self.created.fetch_add(1, Ordering::SeqCst) as i32 + 1;
        Ok(Some(Box::new(TestProcessor { id, items: 0 })))
    }
}

struct TestProcessor {
    id: i32,
    items: i32,
}

#[async_trait]
impl ContentProcessor for TestProcessor {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        assert_eq!(ctx.content_type, "deviceStatus");
        Processed::open(ValueSet::new().with("result", 0i32).with("processor", self.id))
    }

    async fn content(&mut self, _ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        if payload.contains_key("bad") {
            let reason = "bad item";
            return Processed::Rejected(Rejection::with_answer(
                reason,
                ValueSet::new().with("result", 1i32).with("message", rejection_message(reason)),
            ));
        }

        self.items += 1;
        let answer = ValueSet::new().with("result", 0i32).with("processor", self.id).with("items", self.items);
        Processed::Answer { answer, completed: payload.bool("last") == Some(true) }
    }
}

fn synced(content_type: &str, sync_id: &str, extra: ValueSet) -> TransportMessage {
    phone_content(content_type, with(ValueSet::new().with("syncID", sync_id), extra))
}

#[tokio::test(start_paused = true)]
async fn transactions_keep_their_processor_whatever_the_content_type() {
    let h = Harness::new();
    let created = Arc::new(AtomicUsize::new(0));
    h.app
        .register_processor_factory(&["deviceStatus"], Arc::new(TestFactory { created: created.clone(), none: false }))
        .unwrap();
    let meta = ValueSet::new().with("metadata", true);
    let p = |id: i32, items: i32| ValueSet::new().with("result", 0i32).with("processor", id).with("items", items);

    assert_eq!(
        answered(handle(&h, synced("deviceStatus", "s1", meta.clone())).await),
        ValueSet::new().with("result", 0i32).with("processor", 1i32)
    );

    // Content under s1 typed `heartbeat` reaches the stored processor.
    assert_eq!(answered(handle(&h, synced("heartbeat", "s1", ValueSet::new())).await), p(1, 1));

    // A rejected item discards the transaction; the answer carries the engine's message text.
    let rejected = answered(handle(&h, synced("deviceStatus", "s1", ValueSet::new().with("bad", true))).await);
    assert_eq!(rejected, ValueSet::new().with("result", 1i32).with("message", "bad item 80004005"));

    // A new processor is created for the next content message.
    assert_eq!(answered(handle(&h, synced("deviceStatus", "s1", ValueSet::new())).await), p(2, 1));
    assert_eq!(answered(handle(&h, synced("deviceStatus", "s1", ValueSet::new().with("last", true))).await), p(2, 2));

    // Completed: the key is free again.
    assert_eq!(answered(handle(&h, synced("heartbeat", "s1", ValueSet::new())).await), simple(0));

    // Without syncID the key is the correlationVector header.
    let meta_cv =
        |cv: &str| phone_content("deviceStatus", ValueSet::new().with("metadata", true).with("correlationVector", cv));
    handle(&h, meta_cv("cv-9")).await;
    let content_cv = phone_content("heartbeat", ValueSet::new().with("correlationVector", "cv-9"));
    assert_eq!(answered(handle(&h, content_cv).await).i32("processor"), Some(3));

    // A rejected metadata message stores none and removes the entry under its key.
    handle(&h, synced("deviceStatus", "s2", meta.clone())).await;
    assert_eq!(
        answered(handle(&h, synced("deviceStatus", "s2", meta.clone().with("rejectAtCreate", true))).await),
        simple(1)
    );
    assert_eq!(answered(handle(&h, synced("heartbeat", "s2", ValueSet::new())).await), simple(0));

    // A metadata message replaces the entry under its key.
    handle(&h, synced("deviceStatus", "s3", meta.clone())).await;
    handle(&h, synced("deviceStatus", "s3", meta.clone())).await;
    assert_eq!(answered(handle(&h, synced("heartbeat", "s3", ValueSet::new())).await).i32("processor"), Some(6));
    assert_eq!(created.load(Ordering::SeqCst), 6);
}

#[tokio::test(start_paused = true)]
async fn factory_that_creates_none_and_registration_rules() {
    let h = Harness::new();
    let none = Arc::new(TestFactory { created: Arc::new(AtomicUsize::new(0)), none: true });
    h.app.register_processor_factory(&["mediaInfo"], none.clone()).unwrap();
    assert_eq!(
        answered(handle(&h, phone_content("mediaInfo", ValueSet::new().with("metadata", true))).await),
        simple(1)
    );
    assert_eq!(answered(handle(&h, phone_content("mediaInfo", ValueSet::new())).await), simple(1));

    for types in [&["foo"][..], &["test"], &["heartbeat"], &["mediaInfo"], &["contacts", "contacts"]] {
        assert!(
            matches!(h.app.register_processor_factory(types, none.clone()), Err(AppError::Programming(_))),
            "{types:?}"
        );
    }

    // A failed registration registers nothing.
    assert!(h.app.register_processor_factory(&["contactsV2", "heartbeat"], none.clone()).is_err());
    assert_eq!(answered(handle(&h, phone_content("contactsV2", ValueSet::new())).await), simple(1));
    assert_eq!(
        answered(handle(&h, phone_content("contactsV2", ValueSet::new().with("metadata", true))).await),
        simple(2)
    );
}

// ------------------------------------------------------------------------------------------------
// Source, ordering
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn only_the_selected_phone_is_served() {
    let h = Harness::new();
    let other = DcgClient::new("11111111-2222-3333-4444-555555555555", Environment::Prod);
    let d = h.app.handle_inbound(other, trace(), phone_content("heartbeat", ValueSet::new())).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::NotSelectedPhone));

    let other_env = DcgClient::new(PHONE_ID, Environment::Beta);
    let d = h.app.handle_inbound(other_env, trace(), phone_content("heartbeat", ValueSet::new())).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::NotSelectedPhone));
    assert!(h.messenger.responses().is_empty());

    // The id compares case-insensitively.
    let upper = DcgClient::new(PHONE_ID.to_uppercase(), Environment::Prod);
    assert_eq!(
        answered(h.app.handle_inbound(upper, trace(), phone_content("heartbeat", ValueSet::new())).await),
        simple(0)
    );

    let h = Harness::without_phone(Default::default(), Default::default());
    let d = h.app.handle_inbound(phone(), trace(), phone_content("heartbeat", ValueSet::new())).await;
    assert_eq!(d, InboundDisposition::Dropped(DropReason::NotSelectedPhone));
}

#[tokio::test(start_paused = true)]
async fn queued_messages_are_processed_in_arrival_order() {
    let h = Harness::new();
    // Queued before start: processed once the worker runs.
    let msg = |id: &str, content_type: &str| {
        pc_with_headers(&[("_requestId", id), ("_route", "/legacy/phonecontent")], base(content_type))
    };

    h.app.on_app_message(phone(), None, trace(), msg("r1", "heartbeat"));
    h.app.start().unwrap();
    h.app.on_app_message(phone(), None, trace(), msg("r2", "deviceQuery"));
    h.app.on_app_message(phone(), None, trace(), msg("r3", "foo"));
    h.app.on_app_message(phone(), Some("css".into()), trace(), msg("r4", "heartbeat"));

    h.messenger.wait_responses(4).await;
    let ids: Vec<_> = h.messenger.responses().iter().map(|r| r.header("_originalRequestId").unwrap()).collect();
    assert_eq!(ids, ["r1", "r2", "r3", "r4"]);
    let results: Vec<_> = h.messenger.responses().iter().map(|r| r.header("result")).collect();
    assert_eq!(results, [Some("0".to_string()), None, Some("1".to_string()), Some("0".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn arrival_order_holds_on_a_multi_threaded_runtime() {
    let h = Harness::new();
    h.app.start().unwrap();
    let ids: Vec<String> = (0..50).map(|i| format!("m{i}")).collect();

    for (i, id) in ids.iter().enumerate() {
        let t = if i % 3 == 0 { "permissions" } else { "heartbeat" };
        let vs = with(base(t), ValueSet::new().with("syncID", format!("s{i}")).with("batteryPercent", i as i32));
        h.app.on_app_message(
            phone(),
            None,
            trace(),
            pc_with_headers(&[("_requestId", id), ("_route", "/legacy/phonecontent")], vs),
        );
    }

    h.messenger.wait_responses(50).await;
    let got: Vec<_> = h.messenger.responses().iter().map(|r| r.header("_originalRequestId").unwrap()).collect();
    assert_eq!(got, ids);
    assert_eq!(h.app.phone_record().unwrap().status_keys.battery_percent, Some(49));
}

// ------------------------------------------------------------------------------------------------
// A panicking processor
// ------------------------------------------------------------------------------------------------

/// A processor that panics (a feature bug).
struct PanickingFactory;

impl ContentProcessorFactory for PanickingFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        panic!("a feature bug");
    }
}

/// Error handling on the persistent multiplex connection: a panic while one message is processed is
/// isolated to it — the message is refused `{ result: 1 }` and the worker answers the next one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_processor_refuses_its_message_and_the_worker_goes_on() {
    let h = Harness::new();
    h.app.register_processor_factory(&["callLogs"], Arc::new(PanickingFactory)).unwrap();
    h.app.start().unwrap();

    let bad =
        message(&[("_requestId", "req-bad"), ("_route", "/legacy/phonecontent")], valueset::encode(&base("callLogs")));
    AppInbound::on_app_message(&*h.app, phone(), None, trace(), bad);

    let hb =
        message(&[("_requestId", "req-hb"), ("_route", "/legacy/phonecontent")], valueset::encode(&base("heartbeat")));
    AppInbound::on_app_message(&*h.app, phone(), None, trace(), hb);

    let answered =
        |id: &str| h.messenger.responses().into_iter().find(|r| r.request.headers.get("_requestId") == Some(id));
    assert!(
        eventually(|| answered("req-hb").is_some()).await,
        "the heartbeat after the panicking message was answered"
    );
    assert_eq!(answered("req-bad").expect("the panicking message was answered").values(), simple(1));
    assert_eq!(answered("req-hb").unwrap().values(), simple(0));
}

/// A processor whose feature bug panics inside a phone-record update — feature code that runs under
/// the App layer's record lock (`update_phone_record` / `update_record_for` closures).
struct RecordPanicFactory;

impl ContentProcessorFactory for RecordPanicFactory {
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        _payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        Ok(Some(Box::new(RecordPanic)))
    }
}

struct RecordPanic;

#[async_trait]
impl ContentProcessor for RecordPanic {
    async fn content(&mut self, ctx: &InboundContext<'_>, _payload: &ValueSet) -> Processed {
        ctx.app.update_phone_record(|r| {
            r.all_features_disabled = true;
            panic!("a feature bug inside a phone-record update")
        });
        Processed::completed(simple(0))
    }
}

/// A local choice (engine README): a panic while one inbound message is processed is isolated to it — also
/// one inside a phone-record update, which poisons the App layer's record lock: the message is
/// refused `{ result: 1 }`, the record is left as it was (the update is atomic), and the worker answers
/// the next message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panic_inside_a_phone_record_update_is_isolated_and_changes_nothing() {
    let h = Harness::new();
    h.app.register_processor_factory(&["callLogs"], Arc::new(RecordPanicFactory)).unwrap();
    h.app.start().unwrap();

    let bad =
        message(&[("_requestId", "req-bad"), ("_route", "/legacy/phonecontent")], valueset::encode(&base("callLogs")));
    AppInbound::on_app_message(&*h.app, phone(), None, trace(), bad);

    let hb = message(
        &[("_requestId", "req-hb"), ("_route", "/legacy/phonecontent")],
        valueset::encode(&with(base("heartbeat"), ValueSet::new().with("batteryPercent", 33i32))),
    );
    AppInbound::on_app_message(&*h.app, phone(), None, trace(), hb);

    let answered =
        |id: &str| h.messenger.responses().into_iter().find(|r| r.request.headers.get("_requestId") == Some(id));
    assert!(
        eventually(|| answered("req-hb").is_some()).await,
        "the heartbeat after the panicking message was answered"
    );
    assert_eq!(answered("req-bad").expect("the panicking message was answered").values(), simple(1));
    assert_eq!(answered("req-hb").unwrap().values(), simple(0));

    let record = h.app.phone_record().expect("the record is readable after the panic");
    assert!(!record.all_features_disabled, "the panicking update changed nothing");
    assert_eq!(record.status_keys.battery_percent, Some(33), "later updates apply");
}

/// Unfinished processors (an engine rule): when the selected phone changes, the
/// entries of every other phone leave — switching back does not resume a transaction.
#[tokio::test(start_paused = true)]
async fn a_phone_switch_drops_the_other_phones_open_transactions() {
    let h = Harness::new();
    // A `permissions` transaction left open: two pairs announced, none sent.
    let open = with(base("permissions"), ValueSet::new().with("metadata", true).with("itemCount", 2i32));
    assert_eq!(answered(handle(&h, pc(open)).await).i32("result"), Some(1));
    // A content message under the same key reaches the stored processor (no arrays → 14).
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(14));
    let open = with(base("permissions"), ValueSet::new().with("metadata", true).with("itemCount", 2i32));
    assert_eq!(answered(handle(&h, pc(open)).await).i32("result"), Some(1));

    // Another phone is selected, then this one again.
    let other = DcgClient::new("11111111-2222-3333-4444-555555555555", Environment::Prod);
    let mut item = device_item(Some(ASYNC_TRUST_CUSTOM_DATA));
    item.id = other.id.clone();
    h.app.set_phone(other, &item).unwrap();
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();

    // The transaction is gone: the heartbeat is a heartbeat again.
    assert_eq!(answered(handle(&h, phone_content("heartbeat", ValueSet::new())).await), simple(0));
}

/// Inbound dispatch: the transaction key is the payload's `String` `syncID`, else the
/// `correlationVector` header — empty when there is none, so a message without the header and one with
/// an empty header continue the same transaction.
#[tokio::test(start_paused = true)]
async fn a_missing_and_an_empty_correlation_vector_header_key_the_same_transaction() {
    let h = Harness::new();
    // A metadata message with no correlationVector anywhere opens a `permissions` transaction.
    let open = ValueSet::new()
        .with("contentType", "permissions")
        .with("contractVersion", 3.99f64)
        .with("metadata", true)
        .with("itemCount", 1i32);
    assert_eq!(answered(handle(&h, pc(open)).await).i32("result"), Some(1));

    // An item with an empty correlationVector header continues it, whatever its own contentType.
    let item = ValueSet::new()
        .with("contentType", "heartbeat")
        .with("contractVersion", 3.99f64)
        .with("permissionTypes", Value::Int32Array(vec![0x800]))
        .with("permissionStates", Value::Int32Array(vec![0]));
    let d = handle(
        &h,
        pc_with_headers(
            &[("_requestId", "req-2"), ("_route", "/legacy/phonecontent"), ("correlationVector", "")],
            item,
        ),
    )
    .await;
    assert_eq!(answered(d), simple(0), "answered by the stored permissions processor");
    assert!(h.app.phone_record().unwrap().has_permission(0x800));

    // Completed (one pair of one): a heartbeat without the header is a heartbeat again.
    let hb = ValueSet::new().with("contentType", "heartbeat").with("contractVersion", 3.99f64);
    assert_eq!(answered(handle(&h, pc(hb)).await), simple(0));
}

/// Inbound dispatch: a contract version whose integer part is `-1` (in (−2, −1]) passes the
/// contract-version check, and the lock check counts it as `-1`: answered `19` while the session is locked.
#[tokio::test(start_paused = true)]
async fn a_contract_version_with_integer_part_minus_1_is_accepted_and_counts_as_minus_1_when_locked() {
    let h = Harness::new();
    let hb = |cv: f64| pc(ValueSet::new().with("contentType", "heartbeat").with("contractVersion", cv));
    for cv in [-1.5, -1.0, -1.99] {
        assert_eq!(answered(handle(&h, hb(cv)).await), simple(0), "{cv}: accepted");
    }
    assert_eq!(answered(handle(&h, hb(-2.0)).await), simple(3), "-2: integer part -2");

    h.lock.set(LockState::Locked);
    for cv in [-1.5, -1.0, -1.99] {
        assert_eq!(answered(handle(&h, hb(cv)).await), simple(19), "{cv}: locked");
    }
    assert_eq!(answered(handle(&h, hb(3.7)).await), simple(0), "below 3.8 while locked: processed");
}

/// `phoneCommand` type 7: a `notificationsSensitivePermissionScenario` of another type than `String`
/// is a mistyped key — the message is refused `{ result: 1 }` and the permission bit is unchanged.
#[tokio::test(start_paused = true)]
async fn phone_command_7_with_a_mistyped_scenario_is_refused() {
    let h = Harness::new();
    let cmd7 = |v: Value| {
        phone_content(
            "phoneCommand",
            ValueSet::new().with("phoneCommandType", 7i32).with("notificationsSensitivePermissionScenario", v),
        )
    };

    assert_eq!(
        answered(handle(&h, cmd7(Value::String("notifications_sensitive_permission_granted".into()))).await),
        simple(0)
    );
    assert!(h.app.phone_record().unwrap().has_permission(0x8000_0000));

    for mistyped in [Value::Bool(false), Value::Int32(0)] {
        assert_eq!(answered(handle(&h, cmd7(mistyped)).await), simple(1));
    }
    assert!(h.app.phone_record().unwrap().has_permission(0x8000_0000), "unchanged");
}
