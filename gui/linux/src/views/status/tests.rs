use super::*;

fn status(live: bool) -> PhoneStatus {
    PhoneStatus {
        live,
        battery: BatteryState::Level { percent: 76, charging: true, power_save: false },
        cellular: CellularState::Bars { level: 3, max: 4, operator: Some("Carrier".to_owned()), four_bar_glyphs: true },
        wifi: WifiState::Bars { level: 4 },
        bluetooth: RadioStatus::On,
        mobile_data: Some(false),
        metered: true,
        do_not_disturb: Some(DndControlState { shown: DndDisplay::Status { status: 1, on: true }, enabled: true }),
        ringer: Some(RingerControlState { shown: RingerDisplay::Mode { mode: 1 }, enabled: true }),
        ring_my_phone: Some(RingControlState { shown: RingDisplay::Off, enabled: true, can_stop: false }),
    }
}

#[test]
fn counts_fill_their_specifier_and_percent_signs() {
    assert_eq!(with_count("%d%% · Charging", 76), "76% · Charging");
    assert_eq!(with_count("Ringing · %u s", 12), "Ringing · 12 s");
    assert_eq!(with_count("Expires in %lld min", 3), "Expires in 3 min");
}

#[test]
fn a_live_phone_lists_every_known_item_in_the_menus_order() {
    let value = status_value(&status(true));
    let ids: Vec<&str> = value["lines"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["battery", "cellular", "wifi", "bluetooth", "mobileData", "metered"]);
    assert_eq!(value["lines"][0]["text"], "76% · Charging");
    assert_eq!(value["lines"][0]["icon"], "battery-080-charging");
    assert_eq!(value["lines"][1]["text"], "Carrier");
    assert_eq!(value["lines"][1]["icon"], "network-mobile-80");
    assert_eq!(value["lines"][4]["text"], "Mobile Data Off");
}

#[test]
fn a_phone_that_is_not_live_has_no_lines_and_name_only_tiles() {
    let value = status_value(&status(false));
    assert!(value["lines"].as_array().unwrap().is_empty());
    assert_eq!(value["dnd"]["tile"], json!({ "state": null, "on": false, "busy": false, "enabled": false }));
    assert_eq!(value["dnd"]["menuEnabled"], false);
    assert_eq!(value["ringer"]["menuHint"], Value::Null);
}

#[test]
fn the_ringer_names_its_mode_and_the_next_one() {
    let value = status_value(&status(true));
    assert_eq!(value["ringer"]["mode"], "vibrate");
    assert_eq!(value["ringer"]["menuTitle"], "Ringer: Vibrate");
    assert_eq!(value["ringer"]["menuHint"], "Click to switch to Silent");
    assert_eq!(value["ringer"]["tile"]["state"], "Vibrate");
}

#[test]
fn a_pending_ringer_shows_where_it_goes() {
    let mut phone = status(true);
    phone.ringer = Some(RingerControlState { shown: RingerDisplay::TurningOff, enabled: true });
    let value = status_value(&phone);
    assert_eq!(value["ringer"]["menuTitle"], "Ringer: Switching to Silent...");
    assert_eq!(value["ringer"]["menuEnabled"], false);
    assert_eq!(value["ringer"]["tile"]["busy"], true);
}

#[test]
fn a_ringing_phone_counts_down_and_offers_stop() {
    let mut phone = status(true);
    phone.ring_my_phone = Some(RingControlState {
        shown: RingDisplay::Ringing { remaining_seconds: 12, confirm_stop: false },
        enabled: true,
        can_stop: true,
    });
    let value = status_value(&phone);
    assert_eq!(value["ring"]["menuTitle"], "Ringing... 12 s");
    assert_eq!(value["ring"]["stopTitle"], "Stop Ringing");
    assert_eq!(value["ring"]["tile"], json!({ "state": "Ringing · 12 s", "on": true, "busy": false, "enabled": true }));
}

#[test]
fn do_not_disturb_turning_on_is_a_line_not_a_toggle() {
    let mut phone = status(true);
    phone.do_not_disturb = Some(DndControlState { shown: DndDisplay::TurningOn, enabled: true });
    let value = status_value(&phone);
    assert_eq!(value["dnd"]["menuOn"], Value::Null);
    assert_eq!(value["dnd"]["menuTitle"], "Turning On Do Not Disturb...");
    assert_eq!(value["dnd"]["tile"]["busy"], true);
}
