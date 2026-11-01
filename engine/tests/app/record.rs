//! The phone record and the per-install identifiers through the facade: the App layer's
//! preconditions and state, the phone record from the device list (Android), the phone capability
//! bits, and where the app differs from Phone Link on Windows there.

use crate::support::app_world;

use std::sync::Arc;

use app_world::*;
use phonelink_engine::app::{AppError, AppEvent, ConnectKind, ConnectSendResult, PhoneRecord};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::config::{AppConfig, EngineConfig};
use phonelink_engine::edge::SecureStore;
use phonelink_engine::edge::testing::MemoryStore;
use phonelink_engine::types::{DcgClient, Environment};

async fn connect_with(h: &Harness, values: ValueSet) {
    h.messenger.answer_all(response_with(values));
    assert!(matches!(h.app.send_connect(ConnectKind::Immediate, "T").await, ConnectSendResult::Sent(_)));
}

fn caps(bits: i32) -> ValueSet {
    ValueSet::new().with("registeredPermissionTypes", bits)
}

#[test]
fn device_list_defaults_through_the_facade() {
    let h = Harness::with_custom_data(None);
    let r = h.app.phone_record().unwrap();
    assert_eq!(r.capability_bits, 0x4F5F);
    assert_eq!(r.ypp_version, 8);
    assert_eq!(r.android_sdk_version, 24);
    assert_eq!(r.status_capabilities, 0x17FF);
    assert_eq!(r.consent_version, 0);
    assert_eq!(r.contract_version(&AppConfig::default()), 3.99);
    assert!(r.is_from_device_list());
    assert!(!r.is_async_trust_capable());
    assert_eq!(r.name.as_deref(), Some("Pixel"));
    assert_eq!(r.os_version.as_deref(), Some("14"));

    let h = Harness::with_custom_data(Some(r#"{"assetCategory":"Tablet","yppCapabilities":"AsyncTrust"}"#));
    let r = h.app.phone_record().unwrap();
    assert_eq!(r.capability_bits, 0x4E4D);
    assert!(r.is_async_trust_capable());
}

#[tokio::test(start_paused = true)]
async fn defaults_apply_only_while_the_stored_bits_are_the_new_record_value() {
    let store = Arc::new(MemoryStore::new());
    let h = Harness::without_phone(store.clone(), EngineConfig::default());
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();

    connect_with(
        &h,
        ValueSet::new().with("result", 0i32).with("contractVersion", 3.99f64).with("capabilities", caps(0x801)),
    )
    .await;
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 0x801);

    // A device-list refresh without registeredPermissionTypes keeps the reported bits…
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 0x801);

    // …across a restart too: the record is loaded from the store.
    drop(h);
    let h = Harness::without_phone(store.clone(), EngineConfig::default());
    assert_eq!(h.app.phone_record(), None);
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
    let r = h.app.phone_record().unwrap();
    assert_eq!(r.capability_bits, 0x801);
    assert_eq!(r.reported_contract_version, Some(3.99));

    // A device list carrying the property overwrites them.
    h.app.set_phone(phone(), &device_item(Some(r#"{"registeredPermissionTypes":3,"yppCapabilities":32}"#))).unwrap();
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 3);
}

#[tokio::test(start_paused = true)]
async fn contract_version_is_3_99_until_reported_then_the_last_reported_value() {
    let h = Harness::new();
    assert_eq!(h.app.phone_record().unwrap().contract_version(&AppConfig::default()), 3.99);

    connect_with(&h, ValueSet::new().with("result", 0i32).with("contractVersion", 3.5f64)).await;
    assert_eq!(h.app.phone_record().unwrap().contract_version(&AppConfig::default()), 3.5);

    // A device-list refresh keeps the reported value (Phone Link on Windows resets it to 3.99).
    h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
    assert_eq!(h.app.phone_record().unwrap().contract_version(&AppConfig::default()), 3.5);

    // A connect response without contractVersion leaves it.
    connect_with(&h, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(h.app.phone_record().unwrap().reported_contract_version, Some(3.5));
    connect_with(&h, ValueSet::new().with("contractVersion", 3.99f64)).await;
    assert_eq!(h.app.phone_record().unwrap().reported_contract_version, Some(3.99));
}

#[tokio::test(start_paused = true)]
async fn calls_are_added_for_an_android_phone_at_contract_3_93_or_below() {
    let h = Harness::new();
    connect_with(&h, ValueSet::new().with("contractVersion", 3.93f64).with("capabilities", caps(0x1))).await;
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 0x11);

    connect_with(&h, ValueSet::new().with("contractVersion", 3.94f64).with("capabilities", caps(0x1))).await;
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 0x1);

    // The reply's own platform decides: iOS gets nothing added.
    let attrs = ValueSet::new().with("platform", 2i32);
    connect_with(
        &h,
        ValueSet::new().with("contractVersion", 3.0f64).with("attributes", attrs).with("capabilities", caps(0x1)),
    )
    .await;
    let r = h.app.phone_record().unwrap();
    assert_eq!((r.platform, r.capability_bits), (2, 0x1));
}

#[test]
fn ypp_version_rule_without_a_device_list() {
    let config = AppConfig::default();
    let mut r = PhoneRecord::new(&phone());
    r.apply_connect_response(&ValueSet::new(), &config);
    assert_eq!(r.ypp_version, -1);
    r.apply_connect_response(&ValueSet::new().with("yppVersion", 9i32), &config);
    assert_eq!(r.ypp_version, 9);
    r.apply_connect_response(&ValueSet::new(), &config);
    assert_eq!(r.ypp_version, 9);
    r.apply_connect_response(&ValueSet::new().with("yppVersion", Value::Int64(10)), &config);
    assert_eq!(r.ypp_version, 9);
}

/// The `installationId` is per install (persisted), the app session id
/// per app run — every App layer of the process, a rebuilt engine core's included, sends the same.
#[tokio::test(start_paused = true)]
async fn installation_id_persists_and_the_session_id_is_per_run() {
    let store = Arc::new(MemoryStore::new());
    let a = Harness::without_phone(store.clone(), EngineConfig::default());
    let b = Harness::without_phone(store.clone(), EngineConfig::default());
    assert_eq!(a.app.installation_id(), b.app.installation_id());

    let id = a.app.installation_id();
    assert_eq!(id.len(), 36);
    assert!(!id.contains('{'));
    assert_eq!(id, id.to_uppercase());
    assert_eq!(store.load("app/installation-id".into()).unwrap().unwrap(), id.as_bytes());

    let other_install = Harness::without_phone(Arc::new(MemoryStore::new()), EngineConfig::default());
    assert_ne!(other_install.app.installation_id(), id);

    assert_eq!(a.app.app_session_id(), b.app.app_session_id(), "one per run, not per layer");
    assert_eq!(other_install.app.app_session_id(), a.app.app_session_id());
    let s = a.app.app_session_id();
    assert!(s.starts_with('{') && s.ends_with('}'));
    assert_eq!(&s[1..37], s[1..37].to_uppercase());

    // The standard keys carry it.
    assert_eq!(a.app.standard_keys().installation_id, id);
    assert_eq!(a.app.standard_keys().display_name, "test-mac");
    assert_eq!(a.app.standard_keys().contract_version, 3.99);
}

#[test]
fn configured_display_name_and_store_errors() {
    let mut config = EngineConfig::default();
    config.identity.display_name = Some("Studio".into());
    let h = Harness::without_phone(Arc::new(MemoryStore::new()), config);
    assert_eq!(h.app.standard_keys().display_name, "Studio");

    // A corrupt record or installation id fails loudly.
    let store = Arc::new(MemoryStore::new());
    store.store(format!("app/phone/{PHONE_ID}.Prod"), b"{not json".to_vec()).unwrap();
    let h = Harness::without_phone(store.clone(), EngineConfig::default());
    assert!(matches!(h.app.set_phone(phone(), &device_item(None)), Err(AppError::CorruptStore { .. })));

    store.store("app/installation-id".into(), b"lower-case-or-junk".to_vec()).unwrap();
    assert!(matches!(try_app(store, EngineConfig::default()), Err(AppError::CorruptStore { .. })));

    // An empty display name is a configuration error.
    let mut config = EngineConfig::default();
    config.identity.display_name = Some(" ".into());
    assert!(matches!(try_app(Arc::new(MemoryStore::new()), config), Err(AppError::Config(_))));
}

#[test]
fn set_phone_checks_its_arguments_and_switches_records() {
    let h = Harness::new();
    let mut item = device_item(None);
    item.id = "aaaaaaaa-0000-0000-0000-000000000000".into();
    assert!(matches!(h.app.set_phone(phone(), &item), Err(AppError::Programming(_))));

    let odd = DcgClient::new("../escape", Environment::Prod);
    let mut item = device_item(None);
    item.id = odd.id.clone();
    assert!(matches!(h.app.set_phone(odd, &item), Err(AppError::Programming(_))));

    // Another phone gets its own record; the first stays stored.
    let mut rx = h.app.subscribe_events();
    let other = DcgClient::new("11111111-2222-3333-4444-555555555555", Environment::Prod);
    let mut item = device_item(Some(r#"{"registeredPermissionTypes":1}"#));
    item.id = other.id.clone();
    h.app.set_phone(other.clone(), &item).unwrap();
    assert_eq!(h.app.selected_phone(), Some(other.clone()));
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 1);
    assert!(matches!(rx.try_recv(), Ok(AppEvent::PhoneRecordChanged { phone }) if phone == other));

    let mut keys = h.store.keys();
    keys.sort();
    assert_eq!(
        keys,
        [
            "app/installation-id".to_string(),
            format!("app/phone/{PHONE_ID}.Prod"),
            "app/phone/11111111-2222-3333-4444-555555555555.Prod".to_string()
        ]
    );
}

/// The connect response's `attributes`: a `String` `countryCode` writes the record's country code
/// and `defaultLocale` the locale only; the device list's `defaultLocale` writes both — the last
/// writer wins.
#[tokio::test(start_paused = true)]
async fn the_country_code_is_the_last_writers() {
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"defaultLocale":"en-GB"}"#));
    let r = h.app.phone_record().unwrap();
    assert_eq!((r.country_code.as_deref(), r.locale.as_deref()), (Some("en-GB"), Some("en-GB")));

    let attributes = ValueSet::new().with("countryCode", "us").with("defaultLocale", "en-US");
    connect_with(&h, ValueSet::new().with("result", 0i32).with("attributes", attributes)).await;
    let r = h.app.phone_record().unwrap();
    assert_eq!((r.country_code.as_deref(), r.locale.as_deref()), (Some("us"), Some("en-US")));

    // A reply without countryCode leaves it.
    connect_with(&h, ValueSet::new().with("result", 0i32).with("attributes", ValueSet::new())).await;
    assert_eq!(h.app.phone_record().unwrap().country_code.as_deref(), Some("us"));

    // The next device-list application writes both again.
    h.app
        .reapply_device_list(&phone(), &device_item(Some(r#"{"yppCapabilities":32,"defaultLocale":"en-GB"}"#)))
        .unwrap();
    let r = h.app.phone_record().unwrap();
    assert_eq!((r.country_code.as_deref(), r.locale.as_deref()), (Some("en-GB"), Some("en-GB")));
}

/// The phone's OS version is one value with two writers, the last one winning: every device-list
/// application writes the entry's `osVersion` — an entry without one clears it — and a connect
/// reply writes a `String` `attributes.osVersion`.
#[tokio::test(start_paused = true)]
async fn the_os_version_is_the_last_writers() {
    let h = Harness::new();
    let major = |h: &Harness| h.app.phone_record().unwrap().android_major_version();
    assert_eq!(major(&h), Some(14), "the device list at selection");

    let os = |v: &str| ValueSet::new().with("result", 0i32).with("attributes", ValueSet::new().with("osVersion", v));
    connect_with(&h, os("15.0.1")).await;
    assert_eq!(major(&h), Some(15), "the connect reply came last");

    // A reply without a String osVersion writes nothing.
    connect_with(&h, ValueSet::new().with("attributes", ValueSet::new().with("osVersion", 16i32))).await;
    assert_eq!(major(&h), Some(15));

    // The next device-list application comes last.
    let mut item = device_item(Some(ASYNC_TRUST_CUSTOM_DATA));
    item.os_version = Some("13".into());
    assert!(h.app.reapply_device_list(&phone(), &item).unwrap());
    assert_eq!(major(&h), Some(13), "the device list came last");

    connect_with(&h, os("16")).await;
    assert_eq!(major(&h), Some(16));

    // A later device-list entry without osVersion clears it.
    item.os_version = None;
    h.app.reapply_device_list(&phone(), &item).unwrap();
    assert_eq!(major(&h), None, "cleared");

    // Empty or unparsable: no version.
    connect_with(&h, os("")).await;
    assert_eq!(major(&h), None);
}

/// `AppLayer::reapply_device_list` rewrites only the selected phone's record — it never switches the
/// selection — and refuses another phone's entry.
#[test]
fn reapply_device_list_rewrites_only_the_selected_phone() {
    let h = Harness::new();
    let other = DcgClient::new("11111111-2222-3333-4444-555555555555", Environment::Prod);
    let mut item = device_item(Some(r#"{"registeredPermissionTypes":1}"#));
    item.id = other.id.clone();
    assert!(!h.app.reapply_device_list(&other, &item).unwrap(), "not the selected phone");
    assert_eq!(h.app.selected_phone(), Some(phone()));
    assert!(matches!(h.app.reapply_device_list(&phone(), &item), Err(AppError::Programming(_))), "another entry");

    let mut rx = h.app.subscribe_events();
    assert!(h.app.reapply_device_list(&phone(), &device_item(Some(r#"{"registeredPermissionTypes":3}"#))).unwrap());
    assert_eq!(h.app.phone_record().unwrap().capability_bits, 3);
    assert!(matches!(rx.try_recv(), Ok(AppEvent::PhoneRecordChanged { phone: p }) if p == phone()));

    // Persisted.
    let stored = h.store.load(format!("app/phone/{PHONE_ID}.Prod")).unwrap().unwrap();
    assert!(String::from_utf8(stored).unwrap().contains("\"capability_bits\":3"));
}
