//! Device status — Phone Link's contract through the App layer: the sync request, the status
//! content, `phoneCommand` type 0, the commands, the status capabilities; the contribution to the
//! connect message; the status shown when data is missing, the Phone Link model and its `wifiState`
//! reset; the wrongly typed keys (differs from Phone Link on Windows).

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::device_status_world::fixture;

use std::sync::Arc;
use std::time::Duration;

use app_world::*;
use fixture::*;
use phonelink_engine::app::{AppOutcome, AppRequest, ConnectKind, ConnectSendResult, LocalGate, RequestOptions};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::config::features::DeviceStatusConfig;
use phonelink_engine::edge::LockState;
use phonelink_engine::edge::testing::MemoryStore;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::seams::{ConnectionFailure, PlatformRequest, PlatformRequestStatus};
use phonelink_engine::types::{DeviceInfoItem, HeaderMap, MessageType, TransportMessage};
use tokio_util::sync::CancellationToken;

fn sent(r: ConnectSendResult) -> phonelink_engine::app::AppResponse {
    match r {
        ConnectSendResult::Sent(resp) => resp,
        other => panic!("connect not sent: {other:?}"),
    }
}

fn simple(code: i32) -> ValueSet {
    ValueSet::new().with("result", code)
}

fn route(req: &PlatformRequest) -> &str {
    req.message.route().unwrap_or_default()
}

/// Answers the connect with `connect`, every other request `{result: 0}`.
fn answer_connect_with(f: &Fixture, connect: ValueSet) {
    f.h.messenger.set_responder(move |req| {
        if route(req) == "/connect/v1" {
            Reply::Now(response_with(connect.clone()))
        } else {
            Reply::Now(result_response(0))
        }
    });
}

// ------------------------------------------------------------------------------------------------
// The connect contribution
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_connect_request_carries_the_six_flags() {
    let f = Fixture::new();
    sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
    let vs = f.sent_on("/connect/v1").remove(0);
    let configuration = vs.object("configuration").unwrap();

    for k in [
        "BatteryIndicator",
        "DeviceIndicators",
        "DeviceIndicatorsBluetooth",
        "DeviceIndicatorsDND",
        "DeviceIndicatorsVolume",
    ] {
        assert_eq!(configuration.get(k), Some(&Value::Bool(true)), "{k}");
    }

    assert_eq!(configuration.get("AirplaneMode"), Some(&Value::Bool(true)), "the device-list default 0x17FF has 0x400");

    // A phone whose status capabilities lack AirplaneMode.
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":1023}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());
    assert_eq!(f.record().status_capabilities, 0x3FF);

    sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
    let vs = f.sent_on("/connect/v1").remove(0);
    assert_eq!(vs.object("configuration").unwrap().get("AirplaneMode"), Some(&Value::Bool(false)));
}

#[tokio::test(start_paused = true)]
async fn the_connect_reply_applies_capabilities_status_keys_and_asks_for_a_sync() {
    let f = Fixture::new();
    let capabilities = ValueSet::new().with("statusCapabilities", 0x6i32).with("ringMyPhoneCapabilities", 1i32);
    answer_connect_with(
        &f,
        ValueSet::new()
            .with("result", 0i32)
            .with("capabilities", capabilities)
            .with("ringerMode", 2i32)
            .with("doNotDisturbStatus", 1i32)
            .with("batteryPowersave", true)
            .with("bluetoothName", "Pixel BT")
            .with("displayName", "not read from the connect reply"),
    );

    sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
    let r = f.record();
    // Step 1 replaces (0x6), step 2 adds each status key's bit.
    assert_eq!(r.status_capabilities, 0x6 | 0x80 | 0x20 | 0x8);
    assert_eq!(r.status_keys.ringer_mode, Some(2));
    assert_eq!(r.status_keys.do_not_disturb_status, Some(1));
    assert_eq!(r.status_keys.battery_power_save, Some(true));
    assert_eq!(r.status_keys.bluetooth_name.as_deref(), Some("Pixel BT"));
    assert_ne!(r.display_name.as_deref(), Some("not read from the connect reply"));

    // Step 3: result 0 and neither batteryPercent nor bluetoothState → the sync request.
    f.h.messenger.wait_requests(2).await;
    let requests = f.h.messenger.requests_on("/devicestatus");
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.priority, 100, "not user-initiated");

    let vs = payload(request);
    let cv = vs.str("correlationVector").unwrap().to_string();
    assert_eq!(cv.len(), 16);
    assert!(cv.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    assert_eq!(cv, request.trace.correlation_id, "the request's trace correlation id");

    assert_eq!(vs.str("contentType"), Some("deviceStatus"));
    assert_eq!(vs.get("relatedCorrelationIds"), Some(&Value::StringArray(vec![cv.clone()])));
    assert_eq!(vs.get("subcontentTypes"), Some(&Value::StringArray(vec!["deviceStatus".into()])));
    assert_eq!(vs.get("isuserinitiated"), Some(&Value::Bool(false)));
    assert_eq!(vs.get("ignoreResponse"), Some(&Value::Bool(false)));
    assert_eq!(vs.get("transportType"), Some(&Value::Int32(0)));
    assert_eq!(vs.get("contractVersion"), Some(&Value::Double(3.99)));
    let keys: Vec<&str> = vs.keys().collect();
    assert_eq!(&keys[..4], ["correlationVector", "contentType", "relatedCorrelationIds", "subcontentTypes"]);
}

/// Trace context names: the sync request carries the `connect` message's trigger and the scenario
/// `DeviceStatusSync` (a child scenario of the connect trigger's context).
#[tokio::test(start_paused = true)]
async fn the_sync_request_carries_the_connect_triggers_name() {
    let f = Fixture::new();
    answer_connect_with(&f, ValueSet::new().with("result", 0i32));
    sent(f.h.app.send_connect(ConnectKind::Immediate, "AppActivated").await);
    f.h.messenger.wait_requests(2).await;

    let request = f.h.messenger.requests_on("/devicestatus").remove(0);
    let names: Vec<(&str, &str)> =
        request.trace.trace_state.iter().take(2).map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(names, [("ypTriggerId", "AppActivated"), ("ypScenarioId", "DeviceStatusSync")]);
}

#[tokio::test(start_paused = true)]
async fn no_sync_request_when_the_reply_has_status_or_no_success() {
    for reply in [
        ValueSet::new().with("result", 0i32).with("batteryPercent", 50i32),
        ValueSet::new().with("result", 0i32).with("bluetoothState", "mistyped but present"),
        ValueSet::new().with("result", 1i32),
        ValueSet::new().with("result", Value::Int64(0)),
        ValueSet::new(),
    ] {
        let f = Fixture::new();
        answer_connect_with(&f, reply.clone());
        sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
        settle().await;
        assert!(f.h.messenger.requests_on("/devicestatus").is_empty(), "{reply:?}");
    }
}

/// Reply step 3 reads a reply: a connect that got none (timed out) or whose payload is not a
/// `ValueSet` sends no sync request.
#[tokio::test(start_paused = true)]
async fn no_sync_request_without_a_connect_response() {
    for response in [
        status_response(PlatformRequestStatus::ResponseTimedOut),
        phonelink_engine::seams::PlatformResponse {
            status: PlatformRequestStatus::Success,
            message: Some(TransportMessage::new(MessageType::App, HeaderMap::new(), vec![0xFF, 0xFF])),
        },
    ] {
        let f = Fixture::new();
        f.h.messenger.set_responder(move |req| {
            if route(req) == "/connect/v1" { Reply::Now(response.clone()) } else { Reply::Now(result_response(0)) }
        });

        let outcome = sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await).outcome;
        settle().await;
        assert!(f.h.messenger.requests_on("/devicestatus").is_empty(), "{outcome:?}");
    }
}

/// A phone whose device-list entry has no `osName` (platform Unknown — an `LTW` client) with status
/// capabilities `caps`.
fn unknown_platform_fixture(caps: u32) -> Fixture {
    let item = DeviceInfoItem {
        os_name: None,
        client_type: Some("LTW".into()),
        custom_data: Some(format!(r#"{{"yppCapabilities":32,"statusCapabilities":{caps}}}"#)),
        ..device_item(None)
    };

    let h = Harness::without_phone(Arc::new(MemoryStore::new()), EngineConfig::default());
    h.app.set_phone(phone(), &item).unwrap();

    let f = Fixture::build_with_item(h, StoreLocation::Memory, DeviceStatusConfig::default(), item);
    assert_eq!((f.record().platform, f.record().status_capabilities), (0, caps));
    f
}

/// Reply step 3 needs a phone that supports device-status requests: `DeviceStatusRequestSupport`
/// (`0x200`), or an Android phone — which
/// `the_connect_reply_applies_capabilities_status_keys_and_asks_for_a_sync` covers without the bit.
#[tokio::test(start_paused = true)]
async fn the_sync_request_needs_request_support_or_an_android_phone() {
    for (caps, syncs) in [(0xFF, 0), (0xFF | 0x200, 1)] {
        let f = unknown_platform_fixture(caps);
        answer_connect_with(&f, ValueSet::new().with("result", 0i32));
        sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
        settle().await;
        assert_eq!(f.h.messenger.requests_on("/devicestatus").len(), syncs, "{caps:#x}");
    }
}

#[tokio::test(start_paused = true)]
async fn status_capabilities_outside_the_flags_or_mistyped_are_ignored() {
    for capabilities in [
        ValueSet::new().with("statusCapabilities", 0x2000i32),
        ValueSet::new().with("statusCapabilities", -1i32),
        ValueSet::new().with("statusCapabilities", Value::Int64(0x6)),
    ] {
        let f = Fixture::new();
        answer_connect_with(&f, ValueSet::new().with("result", 1i32).with("capabilities", capabilities.clone()));
        sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
        assert_eq!(f.record().status_capabilities, 0x17FF, "{capabilities:?}");
    }

    // A device-list refresh with customData statusCapabilities overwrites what a connect reply set.
    let f = Fixture::new();
    answer_connect_with(
        &f,
        ValueSet::new()
            .with("result", 1i32)
            .with("capabilities", ValueSet::new().with("statusCapabilities", 0x1FFFi32)),
    );
    sent(f.h.app.send_connect(ConnectKind::Immediate, "T").await);
    assert_eq!(f.record().status_capabilities, 0x1FFF);

    f.h.app.set_phone(phone(), &device_item(Some(r#"{"yppCapabilities":32,"statusCapabilities":5}"#))).unwrap();
    assert_eq!(f.record().status_capabilities, 5);
    f.h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
    assert_eq!(f.record().status_capabilities, 5, "absent: kept while not 0");
}

// ------------------------------------------------------------------------------------------------
// Status content
// ------------------------------------------------------------------------------------------------

fn every_item_key() -> ValueSet {
    ValueSet::new()
        .with("batteryPercent", 57i32)
        .with("batteryCharging", true)
        .with("batteryPowersave", true)
        .with("bluetoothState", 12i32)
        .with("bluetoothName", "Pixel BT")
        .with("doNotDisturbStatus", 1i32)
        .with("doNotDisturbRuleEnabled", false)
        .with("networkOperator", "Carrier")
        .with("networkSignalStrength", 4i32)
        .with("networkSignalStrengthMax", 5i32)
        .with("networkIndicatorApiSupport", true)
        .with("mobileDataStatus", 2i32)
        .with("syncOverMeteredConnection", false)
        .with("ringerMode", 2i32)
        .with("wifiSignalLevel", 3i32)
        .with("wifiState", false)
        .with("airplaneMode", false)
        .with("ltwTermsUpdateAvailable", true)
        .with("displayName", "My Pixel")
        .with("isTwoPhoneModeEnabled", true)
        .with("ContentTransferCopyPasteSupported", true)
        .with("ContentTransferDragDropSupported", false)
}

#[tokio::test(start_paused = true)]
async fn a_lone_item_is_applied_with_its_capability_bits_and_answered_0() {
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":0}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());
    assert_eq!(f.record().status_capabilities, 0);

    let answer = f.status_item(every_item_key()).await;
    assert_eq!(answer, simple(0));
    assert_eq!(f.h.messenger.responses().last().unwrap().header("result").as_deref(), Some("0"));

    let r = f.record();
    let s = &r.status_keys;
    assert_eq!((s.battery_percent, s.battery_charging, s.battery_power_save), (Some(57), Some(true), Some(true)));
    assert_eq!((s.bluetooth_state, s.bluetooth_name.as_deref()), (Some(12), Some("Pixel BT")));
    assert_eq!((s.do_not_disturb_status, s.do_not_disturb_rule_enabled), (Some(1), Some(false)));
    assert_eq!(
        (s.network_operator.as_deref(), s.network_signal_strength, s.network_signal_strength_max),
        (Some("Carrier"), Some(4), Some(5))
    );
    assert_eq!(
        (s.network_indicator_api_support, s.mobile_data_status, s.sync_over_metered_connection),
        (Some(true), Some(2), Some(false))
    );
    assert_eq!(
        (s.ringer_mode, s.wifi_signal_level, s.wifi_state, s.airplane_mode),
        (Some(2), Some(3), Some(false), Some(false))
    );

    // Battery 0x8, Bluetooth 0x10, DND 0x20, signal 0x40, ringer 0x80, Wi-Fi 0x100, airplane 0x400.
    assert_eq!(r.status_capabilities, 0x5F8);
    assert_eq!(r.display_name.as_deref(), Some("My Pixel"), "the phone's friendly name");
    assert_eq!(r.two_phone_mode_enabled, Some(true));
    assert_eq!(r.content_transfer, Some(0x1));

    // Bits are only added; ContentTransfer bits are set and cleared; blank names are ignored.
    let more = ValueSet::new()
        .with("ContentTransferCopyPasteSupported", false)
        .with("ContentTransferDragDropSupported", true)
        .with("displayName", "  ")
        .with("bluetoothName", "");
    assert_eq!(f.status_item(more).await, simple(0));

    let r = f.record();
    assert_eq!(r.status_capabilities, 0x5F8);
    assert_eq!(r.content_transfer, Some(0x2));
    assert_eq!(r.display_name.as_deref(), Some("My Pixel"));
    assert_eq!(r.status_keys.bluetooth_name.as_deref(), Some("Pixel BT"));
}

#[tokio::test(start_paused = true)]
async fn the_metadata_step_is_answered_and_keeps_the_transaction_for_its_item() {
    let f = Fixture::new();
    let meta = phone_content("deviceStatus", ValueSet::new().with("metadata", Value::Empty).with("syncID", "s-1"));
    let answer = f.inbound(meta).await;
    assert_eq!(answer, ValueSet::new().with("contractVersion", 3.99f64).with("itemCount", 0i32).with("result", 0i32));
    let keys: Vec<&str> = answer.keys().collect();
    assert_eq!(keys, ["contractVersion", "itemCount", "result"]);

    // The item of that transaction goes to the stored processor.
    let item = phone_content("deviceStatus", ValueSet::new().with("syncID", "s-1").with("bluetoothName", "BT-1"));
    assert_eq!(f.inbound(item).await, simple(0));
    assert_eq!(f.record().status_keys.bluetooth_name.as_deref(), Some("BT-1"));
}

/// Pins a wording fix: `correlationVector` is read only by the message that creates the processor —
/// a metadata message without one is rejected, while an item continuing that message's stored
/// transaction needs none and is applied, answered `0`.
#[tokio::test(start_paused = true)]
async fn only_the_message_that_creates_the_processor_needs_a_correlation_vector() {
    let f = Fixture::new();
    let without_cv = |vs: ValueSet, request_id: &str| {
        let mut payload = ValueSet::new().with("contentType", "deviceStatus").with("contractVersion", 3.99f64);
        for (k, v) in vs.iter() {
            payload.insert(k, v.clone());
        }
        TransportMessage::new(
            MessageType::App,
            HeaderMap::from_pairs([("_requestId", request_id), ("_route", "/legacy/phonecontent")]).unwrap(),
            phonelink_engine::codec::valueset::encode(&payload),
        )
    };

    // A metadata message without one creates no processor: rejected, nothing stored.
    let meta = without_cv(ValueSet::new().with("metadata", Value::Empty).with("syncID", "s-0"), "req-1");
    assert_eq!(f.inbound(meta).await, simple(1));
    let item = without_cv(ValueSet::new().with("syncID", "s-0").with("bluetoothName", "never"), "req-2");
    assert_eq!(f.inbound(item).await, simple(1), "no stored transaction: this item creates the processor");
    assert_eq!(f.record().status_keys.bluetooth_name, None);

    // The metadata message carries one; the item of its transaction does not.
    let meta = phone_content("deviceStatus", ValueSet::new().with("metadata", Value::Empty).with("syncID", "s-1"));
    assert_eq!(f.inbound(meta).await.i32("result"), Some(0));
    let item = without_cv(ValueSet::new().with("syncID", "s-1").with("bluetoothName", "BT-1"), "req-3");
    assert_eq!(f.inbound(item).await, simple(0));
    assert_eq!(f.record().status_keys.bluetooth_name.as_deref(), Some("BT-1"));
}

#[tokio::test(start_paused = true)]
async fn an_item_without_correlation_vector_is_rejected() {
    let f = Fixture::new();
    let vs = ValueSet::new()
        .with("contentType", "deviceStatus")
        .with("contractVersion", 3.99f64)
        .with("bluetoothName", "never applied")
        .with("batteryPowersave", true);
    let no_cv = TransportMessage::new(
        MessageType::App,
        HeaderMap::from_pairs([("_requestId", "req-9"), ("_route", "/legacy/phonecontent")]).unwrap(),
        phonelink_engine::codec::valueset::encode(&vs),
    );

    assert_eq!(f.inbound(no_cv).await, simple(1));
    let r = f.record();
    assert_eq!((r.status_keys.bluetooth_name.clone(), r.status_keys.battery_power_save), (None, None));

    // A correlationVector of another type is accepted.
    let odd_cv =
        phone_content("deviceStatus", ValueSet::new().with("correlationVector", 7i32).with("bluetoothName", "BT"));
    assert_eq!(f.inbound(odd_cv).await, simple(0));
    assert_eq!(f.record().status_keys.bluetooth_name.as_deref(), Some("BT"));

    // Only in the header: copied into the payload by the dispatcher, accepted.
    let header_cv = TransportMessage::new(
        MessageType::App,
        HeaderMap::from_pairs([
            ("_requestId", "req-10"),
            ("_route", "/legacy/phonecontent"),
            ("correlationVector", "hdr-cv"),
        ])
        .unwrap(),
        phonelink_engine::codec::valueset::encode(&vs.with("bluetoothName", "BT-2")),
    );
    assert_eq!(f.inbound(header_cv).await, simple(0));
    assert_eq!(f.record().status_keys.bluetooth_name.as_deref(), Some("BT-2"));
}

#[tokio::test(start_paused = true)]
async fn wrongly_typed_item_keys_are_skipped_and_the_rest_applied() {
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":0}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());

    let keys = ValueSet::new()
        // Skipped (differs from Phone Link on Windows):
        .with("bluetoothState", "12")
        .with("doNotDisturbStatus", Value::Int64(2))
        .with("ringerMode", true)
        .with("wifiSignalLevel", Value::Empty)
        .with("doNotDisturbRuleEnabled", 0i32)
        .with("isTwoPhoneModeEnabled", 1i32)
        .with("ContentTransferCopyPasteSupported", "yes")
        // Ignored, as on Windows:
        .with("batteryPercent", "50")
        .with("airplaneMode", 1i32)
        // Well typed, applied:
        .with("networkSignalStrength", 2i32)
        .with("networkOperator", Value::Empty);
    assert_eq!(f.status_item(keys).await, simple(0), "the normal reply");

    let r = f.record();
    let s = &r.status_keys;
    assert_eq!(
        (s.bluetooth_state, s.do_not_disturb_status, s.ringer_mode, s.wifi_signal_level),
        (None, None, None, None)
    );
    assert_eq!((s.do_not_disturb_rule_enabled, s.battery_percent, s.airplane_mode), (None, None, None));
    assert_eq!((r.two_phone_mode_enabled, r.content_transfer), (None, None));
    assert_eq!(s.network_signal_strength, Some(2));
    assert_eq!(s.network_operator.as_deref(), Some(""), "a null networkOperator is stored as \"\"");
    assert_eq!(r.status_capabilities, 0x40, "only the applied keys add bits");
}

#[tokio::test(start_paused = true)]
async fn the_two_power_save_spellings() {
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":0}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());

    // Every payload: `batteryPowerSave`, which adds the battery bit.
    let hb = phone_content("heartbeat", ValueSet::new().with("batteryPowerSave", true).with("batteryPowersave", false));
    assert_eq!(f.inbound(hb).await, simple(0));
    assert_eq!(f.record().status_keys.battery_power_save, Some(true));
    assert_eq!(f.record().status_capabilities, 0x8);

    // A status item is read for `batteryPowersave`, and the App layer reads `batteryPowerSave` from it
    // as from every payload: the item's `batteryPowersave` wins.
    assert_eq!(f.status_item(ValueSet::new().with("batteryPowersave", false)).await, simple(0));
    assert_eq!(f.record().status_keys.battery_power_save, Some(false));
    assert_eq!(f.record().status_capabilities, 0x8);

    // An item carrying only `batteryPowerSave`: the App layer records it with the battery bit; the
    // item's own reading finds nothing.
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"statusCapabilities":0}"#));
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());
    assert_eq!(f.status_item(ValueSet::new().with("batteryPowerSave", true)).await, simple(0));
    assert_eq!((f.record().status_keys.battery_power_save, f.record().status_capabilities), (Some(true), 0x8));
}

#[tokio::test(start_paused = true)]
async fn phone_command_0_writes_the_model() {
    let f = Fixture::new();
    let cmd = |v: Option<bool>| {
        let mut vs = ValueSet::new().with("phoneCommandType", 0i32);
        if let Some(v) = v {
            vs.insert("syncOverMeteredConnection", v);
        }
        phone_content("phoneCommand", vs)
    };

    assert_eq!(f.inbound(cmd(Some(false))).await, simple(0));
    assert_eq!(f.record().status_keys.sync_over_metered_connection, Some(false));
    assert_eq!(f.inbound(cmd(None)).await, simple(0));
    assert_eq!(f.record().status_keys.sync_over_metered_connection, Some(true), "absent → enabled");

    // A deviceStatus item writes the same value.
    assert_eq!(f.status_item(ValueSet::new().with("syncOverMeteredConnection", false)).await, simple(0));
    assert_eq!(f.record().status_keys.sync_over_metered_connection, Some(false));
}

#[tokio::test(start_paused = true)]
async fn the_model_survives_a_restart() {
    let store = Arc::new(MemoryStore::new());
    let h = Harness::without_phone(store.clone(), EngineConfig::default());
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
    let f = Fixture::build(h, StoreLocation::Memory, DeviceStatusConfig::default());

    assert_eq!(f.status_item(every_item_key()).await, simple(0));
    let before = f.record();
    drop(f);

    let h = Harness::without_phone(store, EngineConfig::default());
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();

    let after = h.app.phone_record().unwrap();
    let mut expected = before.status_keys.clone();
    expected.changes = Default::default();
    assert_eq!(after.status_keys, expected, "values and the delivered set persist");
    assert_eq!(after.status_capabilities, before.status_capabilities);
}

// ------------------------------------------------------------------------------------------------
// The Wi-Fi state reset
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_request_whose_response_is_missing_resets_the_wifi_state() {
    let f = Fixture::new();
    let wifi = |f: &Fixture| f.record().status_keys.wifi_state;
    let request = || {
        AppRequest::kvp("/x", ValueSet::new())
            .with_options(RequestOptions::default().with_timeout(Duration::from_secs(5)))
    };

    // The status says Wi-Fi on (a background connect follows; it is answered 0).
    assert_eq!(f.status_item(ValueSet::new().with("wifiState", true)).await, simple(0));
    assert_eq!(wifi(&f), Some(true));

    // A response arrived, whatever its result: kept.
    f.h.messenger.answer_all(result_response(1));
    assert_eq!(f.h.app.request(request()).await.unwrap().outcome, AppOutcome::Failure);
    assert_eq!(wifi(&f), Some(true));

    // A local gate stops the request: kept.
    f.h.lock.set(LockState::Locked);
    assert_eq!(f.h.app.request(request()).await.unwrap().local_gate, Some(LocalGate::Locked));
    f.h.lock.set(LockState::Unlocked);

    f.h.account.set(false);
    assert_eq!(f.h.app.request(request()).await.unwrap().local_gate, Some(LocalGate::AccountNotReady));
    f.h.account.set(true);
    assert_eq!(wifi(&f), Some(true));

    // Canceled by its caller: kept.
    f.h.messenger.set_responder(|_| Reply::Hang);
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(f.h.app.request_cancellable(request(), cancel).await.unwrap().outcome, AppOutcome::Canceled);
    assert_eq!(wifi(&f), Some(true));

    // Timed out: reset to unknown.
    assert_eq!(f.h.app.request(request()).await.unwrap().outcome, AppOutcome::TimedOut);
    assert_eq!(wifi(&f), None);

    // Canceled by the transport, not the caller: reset to unknown.
    f.h.messenger.answer_all(result_response(0));
    assert_eq!(f.status_item(ValueSet::new().with("wifiState", true)).await, simple(0));
    assert_eq!(wifi(&f), Some(true));

    f.h.messenger.answer_all(status_response(PlatformRequestStatus::Canceled));
    let response = f.h.app.request(request()).await.unwrap();
    assert_eq!((response.outcome, response.canceled_by_caller), (AppOutcome::Canceled, false));
    assert_eq!(wifi(&f), None);

    // A rejection and an unreadable response reset it too.
    for response in [
        status_response(PlatformRequestStatus::RemoteNotSubscribedToRoute),
        phonelink_engine::seams::PlatformResponse {
            status: PlatformRequestStatus::Success,
            message: Some(TransportMessage::new(MessageType::App, HeaderMap::new(), vec![0xFF, 0xFF])),
        },
    ] {
        f.h.messenger.answer_all(result_response(0));
        assert_eq!(f.status_item(ValueSet::new().with("wifiState", false)).await, simple(0));
        assert_eq!(wifi(&f), Some(false));

        f.h.messenger.answer_all(response);
        let outcome = f.h.app.request(request()).await.unwrap().outcome;
        assert!(matches!(outcome, AppOutcome::Unknown | AppOutcome::Failure), "{outcome:?}");
        assert_eq!(wifi(&f), None, "{outcome:?}");
    }

    // So does a request whose wait for the connection failed (a connection failure).
    f.h.messenger.answer_all(result_response(0));
    assert_eq!(f.status_item(ValueSet::new().with("wifiState", false)).await, simple(0));
    assert_eq!(wifi(&f), Some(false));

    f.h.sessions.set_connected(false);
    f.h.sessions.set_behavior(ConnectBehavior::Fail(ConnectionFailure::ConnectionDropped));

    let outcome = f.h.app.request(request()).await.unwrap().outcome;
    assert!(matches!(outcome, AppOutcome::ConnectionFailed(_)), "{outcome:?}");
    assert_eq!(wifi(&f), None);
}
