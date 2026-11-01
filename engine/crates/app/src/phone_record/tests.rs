use super::*;
use crate::AppError;
use phonelink_core::edge::SecureStore;
use phonelink_core::edge::testing::MemoryStore;

fn phone() -> DcgClient {
    DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod)
}

fn item(custom: Option<&str>) -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        custom_data: custom.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn android_defaults_from_an_empty_custom_data() {
    let mut r = PhoneRecord::new(&phone());
    assert_eq!(r.capability_bits, 0x10);
    assert_eq!(r.consent_version, -1);

    r.apply_device_list(&item(None));

    assert_eq!(r.capability_bits, 0x4F5F);
    assert_eq!(r.ypp_version, 8);
    assert_eq!(r.android_sdk_version, 24);
    assert_eq!(r.status_capabilities, 0x17FF);
    assert_eq!(r.consent_version, 0);
    assert_eq!(r.platform, platform::ANDROID);
    assert_eq!(r.contract_version(&AppConfig::default()), 3.99);
}

#[test]
fn tablet_default_and_only_while_new_record_bits() {
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&item(Some(r#"{"assetCategory":"Tablet"}"#)));
    assert_eq!(r.capability_bits, 0x4E4D);

    // Stored bits are no longer the new-record value: an absent key keeps them.
    r.capability_bits = 0x1;
    r.apply_device_list(&item(Some(r#"{"assetCategory":2}"#)));
    assert_eq!(r.capability_bits, 0x1);

    r.apply_device_list(&item(Some(r#"{"registeredPermissionTypes":"2049"}"#)));
    assert_eq!(r.capability_bits, 2049);
}

#[test]
fn ypp_capabilities_keep_a_stored_async_trust_bit_and_accept_names() {
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&item(Some(r#"{"yppCapabilities":"CryptoTrust, AsyncTrust"}"#)));
    assert_eq!(r.ypp_capabilities, 0x21);

    r.apply_device_list(&item(Some(r#"{"yppCapabilities":64}"#)));
    assert_eq!(r.ypp_capabilities, 0x60);

    r.apply_device_list(&item(Some(r#"{"yppCapabilities":"bogus"}"#)));
    assert_eq!(r.ypp_capabilities, 0x60);
    assert!(r.is_async_trust_capable());
}

#[test]
fn locale_and_strings() {
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&item(Some(
            r#"{"defaultLocale":"ko-KR","market":"KR","flightRing":"Retail","deviceResolution":"1080,2400","clientModelName":7,"clientModelRevision":"3","yppVersion":9,"androidSdkVersion":"34","statusCapabilities":5}"#,
        )));

    assert_eq!(r.locale.as_deref(), Some("ko-KR"));
    assert_eq!(r.country_code.as_deref(), Some("ko-KR"));
    assert_eq!(r.market.as_deref(), Some("KR"));
    assert_eq!(r.flight_ring.as_deref(), Some("Retail"));
    assert_eq!(r.device_resolution.as_deref(), Some("1080,2400"));
    assert_eq!(r.client_model_name, Some(7));
    assert_eq!(r.client_model_revision, Some(3));
    assert_eq!(r.ypp_version, 9);
    assert_eq!(r.android_sdk_version, 34);
    assert_eq!(r.status_capabilities, 5);
}

#[test]
fn non_finite_contract_version_is_ignored() {
    let mut r = PhoneRecord::new(&phone());
    r.apply_connect_response(&ValueSet::new().with("contractVersion", f64::NAN), &AppConfig::default());
    assert_eq!(r.reported_contract_version, None);
    assert_eq!(r, r.clone());
}

#[test]
fn android_major_version_is_the_last_writers() {
    let config = AppConfig::default();
    let os_reply = |v: &str| ValueSet::new().with("attributes", ValueSet::new().with("osVersion", v));
    let entry = |os: Option<&str>| DeviceInfoItem { os_version: os.map(str::to_string), ..item(None) };
    let mut r = PhoneRecord::new(&phone());

    assert_eq!(r.android_major_version(), None, "no platform, no version");
    r.apply_device_list(&entry(Some("14.1")));
    assert_eq!(r.android_major_version(), Some(14));
    assert_eq!(r.current_os_version(), Some("14.1"));

    // The connect reply came last.
    r.apply_connect_response(&os_reply(" 15 "), &config);
    assert_eq!(r.android_major_version(), Some(15));
    assert_eq!(r.current_os_version(), Some(" 15 "));

    // A non-String osVersion writes nothing.
    r.apply_connect_response(&ValueSet::new().with("attributes", ValueSet::new().with("osVersion", 16i32)), &config);
    assert_eq!(r.android_major_version(), Some(15));

    // The device list came last.
    r.apply_device_list(&entry(Some("13")));
    assert_eq!(r.android_major_version(), Some(13));
    assert_eq!(r.current_os_version(), Some("13"));
    r.apply_connect_response(&os_reply("x.1"), &config);
    assert_eq!(r.android_major_version(), None, "the last value does not parse");

    // A later device-list entry without osVersion clears it.
    r.apply_connect_response(&os_reply("16"), &config);
    r.apply_device_list(&entry(None));
    assert_eq!(r.android_major_version(), None);
    assert_eq!(r.current_os_version(), None);
    r.apply_device_list(&entry(Some("")));
    assert_eq!(r.android_major_version(), None, "empty: no version");

    // Another platform has no Android version; the 0x17FF status-capability default is Android's
    // only.
    r.platform = platform::IOS;
    r.apply_connect_response(&os_reply("17"), &config);
    assert_eq!(r.android_major_version(), None);

    let mut ios = PhoneRecord::new(&phone());
    ios.apply_device_list(&DeviceInfoItem { os_name: Some("iOS".into()), ..item(None) });
    assert_eq!(ios.status_capabilities, 0);
}

#[test]
fn device_status_items_and_connect_replies_write_the_phone_link_model() {
    let mut r = PhoneRecord::new(&phone());
    let item = ValueSet::new()
        .with("batteryPercent", 40i32)
        .with("batteryPowerSave", true)
        .with("doNotDisturbStatus", 2i32)
        .with("networkOperator", Value::Empty)
        .with("ringerMode", "1");

    r.apply_device_status_item(&item);
    let s = &r.status_keys;
    assert_eq!((s.battery_percent, s.battery_power_save), (Some(40), None), "batteryPowersave on this path");
    assert_eq!((s.do_not_disturb_status, s.network_operator.as_deref(), s.ringer_mode), (Some(2), Some(""), None));
    assert_eq!(r.status_capabilities, 0x28);
    assert_eq!(s.changes, StatusChanges { do_not_disturb_status: 1, ..Default::default() });

    // The same value again is not a change; the inbound phone-content path counts changes too.
    r.status_keys.record(&ValueSet::new().with("doNotDisturbStatus", 2i32).with("ringerMode", 1i32));
    assert_eq!(r.status_keys.changes, StatusChanges { do_not_disturb_status: 1, ringer_mode: 1, ..Default::default() });

    // A connect reply: capabilities first (replace), then the keys' bits.
    let reply = ValueSet::new()
        .with("capabilities", ValueSet::new().with("statusCapabilities", 0x6i32))
        .with("wifiSignalLevel", 2i32);
    r.apply_device_status_connect_reply(&reply);
    assert_eq!(r.status_capabilities, 0x106);

    r.status_keys.reset_wifi_state();
    assert_eq!(r.status_keys.wifi_state, None);
}

#[test]
fn unparsable_custom_data_is_ignored() {
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&item(Some("not json")));
    assert_eq!(r.capability_bits, 0x4F5F);
}

#[test]
fn installation_id_shape_and_persistence() {
    let store = MemoryStore::new();
    let a = load_or_create_installation_id(&store).unwrap();
    let b = load_or_create_installation_id(&store).unwrap();
    assert_eq!(a, b);

    assert!(is_installation_id(&a), "{a}");
    assert!(!is_installation_id(&a.to_lowercase()));

    store.store(INSTALLATION_ID_KEY.into(), b"{bad}".to_vec()).unwrap();
    assert!(matches!(load_or_create_installation_id(&store), Err(AppError::CorruptStore { .. })));

    let s = new_app_session_id();
    assert!(s.starts_with('{') && s.ends_with('}'));
    assert!(is_installation_id(&s[1..s.len() - 1]));

    assert_eq!(process_app_session_id(), process_app_session_id(), "one per run");
    assert!(is_installation_id(&process_app_session_id()[1..37]));
}

#[test]
fn record_round_trips_through_the_store_and_rejects_odd_ids() {
    let store = MemoryStore::new();
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&item(None));
    r.status_keys.wifi_state = Some(true);

    save_record(&store, &r).unwrap();
    assert_eq!(load_record(&store, &phone()).unwrap(), Some(r));
    assert_eq!(store.keys(), vec![format!("app/phone/{}.Prod", phone().id)]);

    assert!(record_key(&DcgClient::new("../x", Environment::Prod)).is_err());
}
