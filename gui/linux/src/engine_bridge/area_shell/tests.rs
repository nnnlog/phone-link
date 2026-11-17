//! Unit tests of what the account pane shows of the phones, the PCs and the registration, and of the
//! hotspot's join by hand.

use std::collections::BTreeSet;

use phonelink_engine::api::engine::DeviceRegistration;
use phonelink_engine::api::hotspot::HotspotError;
use phonelink_engine::api::phone::{AccountPc, PhoneChoice};
use serde_json::{Value, json};

use super::account::{pc_app, pc_json, phone_json, registration_json};
use super::status::manual_join;

fn choice(id: &str, name: &str, model: Option<&str>, last_seen: i64, selected: bool) -> PhoneChoice {
    PhoneChoice { id: id.to_owned(), name: name.to_owned(), model: model.map(str::to_owned), last_seen, selected }
}

#[test]
fn a_phone_names_its_model_only_when_it_differs() {
    let phone = phone_json(&choice("A1", "Galaxy S25", Some("SM-S931N"), 1_700_000_000, false), Some("a1"));
    assert_eq!(phone["model"], "SM-S931N");
    assert_eq!(phone["lastSeenMs"], 1_700_000_000_000_i64);
    assert_eq!(phone["inUse"], true);

    let phone = phone_json(&choice("B2", "Pixel 9", Some("Pixel 9"), 0, true), Some("a1"));
    assert_eq!(phone["model"], Value::Null);
    assert_eq!(phone["lastSeenMs"], Value::Null);
    assert_eq!(phone["inUse"], false);
}

#[test]
fn without_a_selected_phone_the_marked_one_is_in_use() {
    assert_eq!(phone_json(&choice("B2", "Pixel 9", None, 0, true), None)["inUse"], true);
}

#[test]
fn a_pc_names_the_app_it_was_enrolled_for() {
    assert_eq!(pc_app("PL"), "Phone Link");
    assert_eq!(pc_app("WEA"), "Cross Device");
    assert_eq!(pc_app(""), "Unknown app");
    assert_eq!(pc_app("XYZ"), "XYZ");
}

#[test]
fn a_pc_being_unlinked_is_marked() {
    let pc = AccountPc {
        id: "pc1".to_owned(),
        name: String::new(),
        client_type: "PL".to_owned(),
        registered: 1_700_000_000,
        last_seen: 0,
        this_mac: false,
    };
    let unlinking = BTreeSet::from(["pc1".to_owned()]);
    let value = pc_json(&pc, &unlinking);
    assert_eq!(value["unlinking"], true);
    assert_eq!(value["registeredMs"], 1_700_000_000_000_i64);
    assert_eq!(value["lastSeenMs"], Value::Null);
}

#[test]
fn the_registration_reads_as_not_loaded_registered_or_not() {
    assert_eq!(registration_json(None, true), json!({ "loaded": false, "reading": true }));
    assert_eq!(registration_json(Some(&None), false)["registered"], false);
    let registration = Some(DeviceRegistration { puid: "0018".to_owned(), distinguished_device_ids: vec![] });
    let value = registration_json(Some(&registration), false);
    assert_eq!(value["deviceId"], "0018");
    assert_eq!(value["linkedIds"], json!([]));
}

#[test]
fn only_a_network_with_credentials_offers_its_password() {
    let not_visible = HotspotError::NotVisible { ssid: "Phone".to_owned(), passphrase: "secret".to_owned() };
    assert_eq!(manual_join(&not_visible), Some(("Your Phone's Hotspot Isn't Visible Yet", "secret")));
    assert_eq!(manual_join(&HotspotError::Busy), None);
}
