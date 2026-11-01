use super::*;
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::config::AppConfig;
use phonelink_core::types::{DeviceInfoItem, Environment};

fn phone() -> DcgClient {
    DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod)
}

/// A device-list entry with `os_name` and `os_version`. The record's osVersion is written only
/// through its writers (the device list, the connect reply), never set directly.
fn listed(os_name: &str, os_version: &str) -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone().id,
        os_name: Some(os_name.into()),
        os_version: Some(os_version.into()),
        ..Default::default()
    }
}

/// A connect reply carrying `attributes.osVersion`.
fn connect_reply_os_version(os_version: &str) -> ValueSet {
    ValueSet::new().with("attributes", ValueSet::new().with("osVersion", os_version))
}

fn android(caps: u32) -> PhoneRecord {
    let mut r = PhoneRecord::new(&phone());
    r.apply_device_list(&listed("Android", "14"));
    r.status_capabilities = caps;
    r
}

fn view(record: &PhoneRecord, cdeh: Option<&CdehStatus>, connected: bool) -> StatusView {
    build_view(&ViewInputs {
        phone: &phone(),
        record: Some(record),
        cdeh,
        connected,
        dnd: Overlay::None,
        ringer: Overlay::None,
    })
}

#[test]
fn read_as_with_and_without_info() {
    let absent = CdehStatus::read(None);
    assert!(!absent.has_info);
    assert!(CdehStatus::read(Some(&DeviceStatus::default())).has_info);

    assert_eq!(
        (absent.battery_percent, absent.do_not_disturb_status, absent.cellular_signal_level, absent.ringer_mode),
        (-1, 0, -1, -1)
    );
    assert_eq!(
        (absent.bluetooth, absent.cellular, absent.wifi),
        (RadioState::Unknown, RadioState::Unknown, RadioState::Off)
    );
    assert!(absent.do_not_disturb_rule_enabled);

    let present = CdehStatus::read(Some(&DeviceStatus::default()));
    assert_eq!((present.battery_percent, present.wifi_signal_level, present.cellular), (0, 0, RadioState::Off));
    let rule_off =
        CdehStatus::read(Some(&DeviceStatus { do_not_disturb_rule_enabled: Some(false), ..Default::default() }));
    assert!(!rule_off.do_not_disturb_rule_enabled);

    for (v, state) in [
        (10, RadioState::Off),
        (13, RadioState::Off),
        (11, RadioState::On),
        (12, RadioState::On),
        (0, RadioState::Unknown),
        (-2, RadioState::Unknown),
    ] {
        assert_eq!(RadioState::from_bluetooth_state(v), state, "{v}");
    }

    for (v, state) in [
        (-1, RadioState::Unknown),
        (0, RadioState::Off),
        (1, RadioState::On),
        (2, RadioState::On),
        (3, RadioState::Off),
        (4, RadioState::Off),
        (5, RadioState::Unknown),
    ] {
        assert_eq!(RadioState::from_mobile_data_status(v), state, "{v}");
    }

    assert!(CdehStatus { do_not_disturb_status: 2, ..absent.clone() }.do_not_disturb_on());
    assert!(!CdehStatus { do_not_disturb_status: 1, ..absent }.do_not_disturb_on());
}

#[test]
fn phone_link_values_win_and_cdeh_fills_the_gaps() {
    let mut r = android(0x17FF);
    r.status_keys.battery_percent = Some(80);
    let cdeh = CdehStatus {
        battery_percent: 20,
        battery_charging: true,
        is_connection_metered: true,
        ..CdehStatus::read(Some(&DeviceStatus { wifi_signal_level: 3, ..Default::default() }))
    };

    let v = view(&r, Some(&cdeh), true);
    assert_eq!(v.battery, BatteryItem::Level { percent: 80, charging: true, power_save: false }, "per quantity");
    assert_eq!(v.wifi, WifiItem::Bars(3));
    assert!(v.metered);

    // Nothing from either model: unknown.
    let v = view(&android(0x17FF), None, true);
    assert_eq!(
        (v.battery, v.wifi, v.bluetooth, v.mobile_data),
        (BatteryItem::Unknown, WifiItem::Unknown, RadioState::Unknown, None)
    );
    assert_eq!(v.cellular, CellularItem::Unknown);

    // Disconnected or all features disabled: everything unknown.
    let v = view(&r, Some(&cdeh), false);
    assert!(!v.live);
    assert_eq!((v.battery, v.wifi, v.metered), (BatteryItem::Unknown, WifiItem::Unknown, false));
    let mut disabled = r.clone();
    disabled.all_features_disabled = true;
    assert!(!view(&disabled, Some(&cdeh), true).live);
}

#[test]
fn cellular_and_bluetooth_rules() {
    let mut r = android(0x17FF);
    r.status_keys.airplane_mode = Some(true);
    r.status_keys.network_signal_strength = Some(3);
    assert_eq!(view(&r, None, true).cellular, CellularItem::AirplaneMode);

    r.status_capabilities &= !sc::AIRPLANE_MODE;
    assert_eq!(
        view(&r, None, true).cellular,
        CellularItem::Bars { level: 3, max: 5, operator: None, four_bar_glyphs: false }
    );

    r.status_keys.network_signal_strength_max = Some(4);
    r.status_keys.network_indicator_api_support = Some(true);
    r.status_keys.network_operator = Some("Carrier".into());
    assert_eq!(
        view(&r, None, true).cellular,
        CellularItem::Bars { level: 3, max: 4, operator: Some("Carrier".into()), four_bar_glyphs: true }
    );

    r.status_keys.network_signal_strength = Some(-1);
    assert_eq!(view(&r, None, true).cellular, CellularItem::NoSim);
    r.status_keys.network_signal_strength = Some(-2);
    assert_eq!(view(&r, None, true).cellular, CellularItem::Unknown);

    // Phone Link reads Bluetooth 12/10 only; CDEH also turning states.
    r.status_keys.bluetooth_state = Some(11);
    assert_eq!(view(&r, None, true).bluetooth, RadioState::Unknown);
    let cdeh = CdehStatus::read(Some(&DeviceStatus { bluetooth_state: 11, ..Default::default() }));
    assert_eq!(view(&android(0x17FF), Some(&cdeh), true).bluetooth, RadioState::On);
    r.status_keys.bluetooth_state = Some(10);
    assert_eq!(view(&r, Some(&cdeh), true).bluetooth, RadioState::Off);
}

/// CDEH's "info absent" reading supplies no value to the Phone Link status.
#[test]
fn an_ok_status_without_info_supplies_no_value() {
    let absent = CdehStatus::read(None);
    let r = android(0x17FF);

    let v = view(&r, Some(&absent), true);
    assert_eq!(v.cellular, CellularItem::Unknown, "its -1 level is not \"no SIM\"");
    assert_eq!(
        v.do_not_disturb,
        Some(DndControl { shown: DndShown::Unknown, enabled: false }),
        "its 0 DND is not known"
    );
    assert_eq!((v.battery, v.wifi, v.bluetooth), (BatteryItem::Unknown, WifiItem::Unknown, RadioState::Unknown));
    assert_eq!(select(&r.status_keys, Some(&absent)), select(&r.status_keys, None));

    // The same values read from an `info` are the phone's.
    let with_info = CdehStatus { has_info: true, ..absent.clone() };
    let v = view(&r, Some(&with_info), true);
    assert_eq!(v.cellular, CellularItem::NoSim);
    assert_eq!(v.do_not_disturb, Some(DndControl { shown: DndShown::Status { status: 0, on: false }, enabled: true }));

    // Phone Link's own values still come first.
    let mut pl = android(0x17FF);
    pl.status_keys.network_signal_strength = Some(2);
    assert!(matches!(view(&pl, Some(&absent), true).cellular, CellularItem::Bars { level: 2, max: 5, .. }));
}

/// An indicator shows only with the status strip's bit and its own bit set.
#[test]
fn indicators_need_the_status_strip_and_their_own_bits() {
    let mut r = android(0x17FF);
    r.status_keys.battery_percent = Some(50);
    r.status_keys.network_signal_strength = Some(3);
    r.status_keys.wifi_signal_level = Some(2);
    r.status_keys.bluetooth_state = Some(12);

    let shown = |r: &PhoneRecord| {
        let v = view(r, None, true);
        [
            v.battery != BatteryItem::Unknown,
            v.cellular != CellularItem::Unknown,
            v.wifi != WifiItem::Unknown,
            v.bluetooth != RadioState::Unknown,
        ]
    };

    assert_eq!(shown(&r), [true; 4]);

    // No status strip: none of the four, whatever their bits.
    r.status_capabilities = 0x17FF & !sc::DEVICE_STATUS_INDICATOR;
    assert_eq!(shown(&r), [false; 4]);

    // The strip with one indicator's bit shows that indicator only.
    let bits = [sc::BATTERY_INDICATOR, sc::NETWORK_SIGNAL_INDICATOR, sc::WIFI_INDICATOR, sc::BLUETOOTH_INDICATOR];
    for (i, bit) in bits.into_iter().enumerate() {
        r.status_capabilities = sc::DEVICE_STATUS_INDICATOR | bit;
        assert_eq!(shown(&r), std::array::from_fn(|j| j == i), "{bit:#x}");
    }

    // Airplane mode is the cellular indicator's: no network bit, no airplane mode either.
    r.status_keys.airplane_mode = Some(true);
    r.status_capabilities = sc::DEVICE_STATUS_INDICATOR | sc::AIRPLANE_MODE;
    assert_eq!(view(&r, None, true).cellular, CellularItem::Unknown);

    // All of 0x1FFF stands for every bit, the strip's too.
    r.status_capabilities = sc::ALL;
    assert_eq!(view(&r, None, true).cellular, CellularItem::AirplaneMode);
    assert_eq!(shown(&r), [true; 4]);

    // The controls keep their own gates.
    r.status_keys.do_not_disturb_status = Some(1);
    r.status_capabilities = sc::DEVICE_ACTION_INDICATOR | sc::DO_NOT_DISTURB_INDICATOR;
    assert_eq!(shown(&r), [false; 4]);
    assert!(view(&r, None, true).do_not_disturb.is_some());

    // iOS: the strip also with the battery bit alone; another platform: no strip.
    r.apply_device_list(&listed("iOS", "17"));
    r.status_capabilities = sc::BATTERY_INDICATOR;
    assert_eq!(shown(&r), [true, false, false, false]);
    r.platform = platform::UNKNOWN;
    r.status_capabilities = sc::ALL;
    assert_eq!(shown(&r), [false; 4]);
}

#[test]
fn controls_exist_and_enable_by_the_rules() {
    let mut r = android(0x17FF);
    r.status_keys.do_not_disturb_status = Some(1);
    r.status_keys.ringer_mode = Some(2);

    let v = view(&r, None, true);
    assert_eq!(v.do_not_disturb, Some(DndControl { shown: DndShown::Status { status: 1, on: false }, enabled: true }));
    assert_eq!(v.ringer, Some(RingerControl { shown: RingerShown::Mode(2), enabled: true }));

    // No action strip: no controls; no DND bit: no DND.
    assert_eq!(view(&android(0x17FB), None, true).do_not_disturb, None);
    let no_dnd = android(0x17FF & !sc::DO_NOT_DISTURB_INDICATOR);
    assert_eq!(view(&no_dnd, None, true).do_not_disturb, None);
    assert!(view(&no_dnd, None, true).ringer.is_some());

    // Unknown values disable; disconnected shows unknown, disabled.
    let v = view(&android(0x17FF), None, true);
    assert_eq!(v.do_not_disturb, Some(DndControl { shown: DndShown::Unknown, enabled: false }));
    let v = view(&r, None, false);
    assert_eq!(v.ringer, Some(RingerControl { shown: RingerShown::Unknown, enabled: false }));

    // Ringer not supported: hidden while live …
    r.status_keys.ringer_mode = Some(-5);
    assert_eq!(view(&r, None, true).ringer, None);

    // … but disconnected or with all features disabled it shows unknown (the mode reads as unknown
    // while the indicator is disabled, so `-5` hides it only while live).
    let unknown = Some(RingerControl { shown: RingerShown::Unknown, enabled: false });
    assert_eq!(view(&r, None, false).ringer, unknown);
    let mut disabled = r.clone();
    disabled.all_features_disabled = true;
    assert_eq!(view(&disabled, None, true).ringer, unknown);

    // Android 13+: the post-notification permission gates both (unreported = granted).
    r.status_keys.ringer_mode = Some(0);
    r.set_permission(permission::POST_NOTIFICATIONS, false);
    let v = view(&r, None, true);
    assert!(!v.do_not_disturb.unwrap().enabled && !v.ringer.unwrap().enabled);
    r.apply_device_list(&listed("Android", "12.1"));
    assert!(view(&r, None, true).do_not_disturb.unwrap().enabled, "Android 12: no post-notification gate");
    r.apply_connect_response(&connect_reply_os_version("13"), &AppConfig::default());
    assert!(!view(&r, None, true).do_not_disturb.unwrap().enabled, "Android 13, written by the connect reply");

    // Pending and reverted overlays.
    let pending = build_view(&ViewInputs {
        phone: &phone(),
        record: Some(&android(0x17FF)),
        cdeh: None,
        connected: true,
        dnd: Overlay::Pending(DndPending::TurningOff),
        ringer: Overlay::Reverted(1),
    });
    assert_eq!(pending.do_not_disturb, Some(DndControl { shown: DndShown::TurningOff, enabled: false }));
    assert_eq!(pending.ringer, Some(RingerControl { shown: RingerShown::Mode(1), enabled: true }));
}

#[test]
fn post_notification_version_parsing() {
    let mut r = android(0);
    for (os, supported) in [("13", true), ("14.0.1", true), (" 15 ", true), ("12", false), ("x", false), ("", false)] {
        r.apply_device_list(&listed("Android", os));
        assert_eq!(r.post_notification_ask_supported(), supported, "{os:?}");
    }

    r.apply_device_list(&listed("iOS", "17"));
    assert_eq!(r.platform, platform::IOS);
    assert!(!r.post_notification_ask_supported());
}
