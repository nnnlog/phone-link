//! The phone's Bluetooth device on its own, without a feature on top: the names a paired device may
//! carry, finding the paired phone, a pairing's early stops and its edge failures, and the Bluetooth
//! `/permission` request recording its decision.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use phonelink_engine::app::BluetoothRecord;
use phonelink_engine::app::consts::{permission, platform, result_code};
use phonelink_engine::config::features::BluetoothConfig;
use phonelink_engine::features::bluetooth::edge::testing::ScriptedBluetooth;
use phonelink_engine::features::bluetooth::{Bluetooth, BluetoothFailure, PairingError};

use crate::support::app_world::{Harness, Reply, phone, result_response};
use crate::support::common::payload;
use crate::support::feature_world::{self, FeatureWorld};

const PHONE_ADDRESS: &str = "aa-bb-cc-dd-ee-ff";

struct World {
    h: Harness,
    _w: FeatureWorld,
    edge: Arc<ScriptedBluetooth>,
    bluetooth: Arc<Bluetooth>,
}

/// An Android 14 phone shown as "Galaxy" (its device-list name is "Pixel") whose device status reports
/// the Bluetooth name "Galaxy BT".
fn world() -> World {
    let h = Harness::new();
    let w = feature_world::feature_world(&h);
    let edge = ScriptedBluetooth::new();
    let bluetooth = Bluetooth::install(&w.env, BluetoothConfig::default(), edge.clone()).unwrap();

    h.app.update_phone_record(|r| {
        r.platform = platform::ANDROID;
        r.android_sdk_version = 34;
        r.display_name = Some("Galaxy".into());
        r.status_keys.bluetooth_name = Some("Galaxy BT".into());
    });

    World { h, _w: w, edge, bluetooth }
}

/// The phone answers each `/permission` request with the code `answer` gives for its type.
fn phone_answers(h: &Harness, answer: impl Fn(u32) -> i32 + Send + 'static) {
    h.messenger.set_responder(move |req| {
        let kind = payload(req).i32("permissionType").map_or(0, |t| t as u32);
        Reply::Now(result_response(answer(kind)))
    });
}

/// The `permissionType` of every request sent so far.
fn permissions_asked(h: &Harness) -> Vec<u32> {
    h.messenger.requests().iter().filter_map(|req| payload(req).i32("permissionType")).map(|t| t as u32).collect()
}

#[tokio::test(start_paused = true)]
async fn an_android_phone_is_looked_for_by_its_bluetooth_name_too_and_an_ios_phone_is_not() {
    let w = world();
    let record = w.h.app.phone_record().unwrap();
    assert_eq!(Bluetooth::phone_names(&record), ["Galaxy", "Pixel", "Galaxy BT"]);

    w.h.app.update_phone_record(|r| r.platform = platform::IOS);
    let record = w.h.app.phone_record().unwrap();
    assert_eq!(Bluetooth::phone_names(&record), ["Galaxy", "Pixel"]);
}

#[tokio::test(start_paused = true)]
async fn the_paired_phone_is_found_by_its_names() {
    let w = world();
    assert_eq!(w.bluetooth.paired_phone(&phone()).await, Ok(None));

    w.edge.set_paired(Some(PHONE_ADDRESS));
    assert_eq!(w.bluetooth.paired_phone(&phone()).await, Ok(Some(PHONE_ADDRESS.to_string())));
    let asked = w.edge.asked_names.lock().unwrap().clone();
    assert_eq!(asked.last().unwrap(), &["Galaxy", "Pixel", "Galaxy BT"]);
}

#[tokio::test(start_paused = true)]
async fn with_the_radio_off_there_is_no_paired_phone_and_no_pairing() {
    let w = world();
    w.edge.radio.store(false, Ordering::SeqCst);
    assert_eq!(w.bluetooth.paired_phone(&phone()).await, Err(BluetoothFailure::BluetoothOff));
    assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Err(BluetoothFailure::BluetoothOff));
    assert!(w.h.messenger.requests().is_empty(), "nothing is asked of the phone");
}

#[tokio::test(start_paused = true)]
async fn a_phone_already_paired_is_used_without_asking_the_phone() {
    let w = world();
    w.edge.set_paired(Some(PHONE_ADDRESS));
    assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Ok(PHONE_ADDRESS.to_string()));
    assert!(w.h.messenger.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_does_not_become_discoverable_is_not_paired() {
    let w = world();
    phone_answers(&w.h, |_| result_code::USER_DENIED);
    assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Err(BluetoothFailure::NotDiscoverable));
    assert_eq!(permissions_asked(&w.h), [permission::BLUETOOTH_ADVERTISE]);
    assert!(w.edge.paired.lock().unwrap().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_asks_for_the_bluetooth_permission_first_is_asked_for_it() {
    let w = world();
    w.h.app.update_phone_record(|r| {
        r.bluetooth = BluetoothRecord {
            permission: false,
            capabilities: BluetoothRecord::REQUEST_BLUETOOTH_ADVERTISE_CONNECT_SCAN,
        };
    });
    phone_answers(&w.h, |kind| if kind == permission::BLUETOOTH { result_code::USER_DENIED } else { 0 });

    assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Err(BluetoothFailure::PermissionNotGranted));
    assert_eq!(permissions_asked(&w.h), [permission::BLUETOOTH]);
    assert!(!w.h.app.phone_record().unwrap().bluetooth.permission, "the denial is recorded");
}

#[tokio::test(start_paused = true)]
async fn the_edges_pairing_failures_reach_the_feature() {
    let cases = [
        (PairingError::NotFound, BluetoothFailure::PhoneNotFound),
        (
            PairingError::Failed { reason: "the phone refused".into() },
            BluetoothFailure::PairingFailed("the phone refused".into()),
        ),
    ];
    for (edge_error, expected) in cases {
        let w = world();
        phone_answers(&w.h, |_| 0);
        *w.edge.discovery.lock().unwrap() = Err(edge_error.clone());
        assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Err(expected), "for {edge_error:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_pairing_discovers_the_phone_by_its_names_and_returns_its_address() {
    let w = world();
    phone_answers(&w.h, |_| 0);
    *w.edge.discovery.lock().unwrap() = Ok(PHONE_ADDRESS.into());

    assert_eq!(w.bluetooth.pair(&phone(), "Test").await, Ok(PHONE_ADDRESS.to_string()));
    assert_eq!(w.edge.paired.lock().unwrap().as_deref(), Some(PHONE_ADDRESS));
    let asked = w.edge.asked_names.lock().unwrap().clone();
    assert_eq!(asked.last().unwrap(), &["Galaxy", "Pixel", "Galaxy BT"]);
}

#[tokio::test(start_paused = true)]
async fn the_permission_request_records_a_deciding_answer_and_ignores_the_others() {
    let w = world();
    phone_answers(&w.h, |_| 0);
    assert_eq!(w.bluetooth.request_permission(&phone(), "Test").await, Ok(Some(true)));
    assert!(w.h.app.phone_record().unwrap().bluetooth.permission);

    phone_answers(&w.h, |_| result_code::FAILURE);
    assert_eq!(w.bluetooth.request_permission(&phone(), "Test").await, Ok(None));
    assert!(w.h.app.phone_record().unwrap().bluetooth.permission, "a failure leaves the recorded answer");
}
