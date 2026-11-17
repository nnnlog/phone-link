use phonelink_engine::api::calling::{
    ActiveCall, ActiveCallRole, ActiveCallStatus, CallControlState, CallControlStatus, CallingState, CallsAccess,
};
use phonelink_engine::api::contacts::{
    ContactCard, ContactCardField, ContactFieldLabel, ContactListItem, ContactsAccess, ContactsState,
};
use serde_json::{Value, json};

use super::calls_json::{active_call_json, calls_state_json, connection_notice};
use super::contacts_json::{card_json, contact_item_json, contacts_state_json, label};
use super::control::bluetooth_settings;
use super::presentation::{Presentation, PresentationChange};
use super::ringer::{RING_PERIOD_MS, RING_RATE, ring_period};
use super::routing::{pairing_answer, reply_of};
use super::state::PhoneRequest;
use super::sync_wait::SyncWait;
use crate::system::Desktop;

fn call(id: u64, status: ActiveCallStatus, role: ActiveCallRole) -> ActiveCall {
    ActiveCall {
        id,
        number: format!("+8210000000{id}"),
        name: None,
        contact_id: None,
        photo_contact_id: None,
        incoming: status == ActiveCallStatus::Incoming,
        status,
        waiting: false,
        talking_since_ms: None,
        role,
    }
}

#[test]
fn a_ringing_call_shows_rings_and_gets_a_lifetime() {
    let mut presentation = Presentation::default();
    let calls = [call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)];
    let change = presentation.update(&calls);
    assert_eq!(change, PresentationChange { lifetimes: vec![(7, 0)], ring: Some(true), window: false });
    assert_eq!(presentation.popup(&calls).map(|c| c.id).collect::<Vec<_>>(), [7]);

    // The same call again starts nothing new.
    assert_eq!(presentation.update(&calls), PresentationChange::default());
}

#[test]
fn a_closed_or_expired_popup_stops_the_ring_and_stays_closed() {
    let mut presentation = Presentation::default();
    let calls = [call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)];
    presentation.update(&calls);

    assert!(!presentation.expire(7, 5), "another token does not expire it");
    assert!(presentation.expire(7, 0));
    assert_eq!(presentation.update(&calls).ring, Some(false));
    assert_eq!(presentation.popup(&calls).count(), 0);
    assert!(!presentation.expire(7, 0), "a hidden call expires once");
}

#[test]
fn an_ended_call_is_forgotten() {
    let mut presentation = Presentation::default();
    presentation.update(&[call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)]);
    presentation.hidden.insert(7);
    presentation.update(&[]);
    assert!(presentation.hidden.is_empty());
    assert!(!presentation.expire(7, 0), "the ended call's timer does nothing");
}

#[test]
fn a_new_main_call_opens_the_call_window_again() {
    let mut presentation = Presentation::default();
    let first = [call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary)];
    assert!(presentation.update(&first).window);
    assert!(presentation.window_open);

    // The user closes it: the same call does not open it again.
    presentation.window_open = false;
    assert_eq!(presentation.update(&first), PresentationChange::default());
    assert!(!presentation.window_open);
    assert!(!presentation.window_shows_failures());

    presentation.update(&[call(2, ActiveCallStatus::Dialing, ActiveCallRole::Primary)]);
    assert!(presentation.window_open);
    assert!(presentation.window_shows_failures());

    presentation.update(&[]);
    assert!(!presentation.window_open);
    assert_eq!(presentation.window_call, None);
}

#[test]
fn a_wait_runs_once_until_data_falls_behind_again() {
    let mut wait = SyncWait::default();
    assert_eq!(wait.observe(false, true), Some(0));
    assert_eq!(wait.observe(false, true), None, "a running wait is not started again");
    assert!(wait.expire(0));
    assert!(wait.over);
    assert_eq!(wait.observe(false, true), None, "an expired wait stays over");

    assert_eq!(wait.observe(true, true), None);
    assert_eq!(wait.observe(false, true), Some(1), "calls that fall behind again wait again");
    assert!(!wait.over);
    assert!(!wait.expire(0), "the earlier wait's timer does nothing");
    assert!(wait.expire(1));
}

#[test]
fn a_wait_without_restart_runs_once() {
    let mut wait = SyncWait::default();
    assert_eq!(wait.observe(false, false), Some(0));
    assert!(wait.expire(0));
    assert_eq!(wait.observe(true, false), None);
    assert_eq!(wait.observe(false, false), None);
    assert!(wait.over);
}

#[test]
fn calls_show_as_updating_until_the_wait_is_over() {
    let state = CallingState {
        access: CallsAccess::Granted,
        pc_consent: true,
        up_to_date: false,
        last_updated_ms: None,
        refreshing: false,
        unseen_missed: 2,
    };
    let mut wait = SyncWait::default();
    let request = PhoneRequest::default();
    assert_eq!(calls_state_json(&state, &wait, &request, false)["updating"], true);
    wait.over = true;
    let json = calls_state_json(&state, &wait, &request, false);
    assert_eq!(json["updating"], false);
    assert_eq!(json["unseenMissed"], 2);
    assert_eq!(json["access"], "granted");

    let refreshing = CallingState { refreshing: true, ..state };
    assert_eq!(calls_state_json(&refreshing, &wait, &request, false)["updating"], true);
    let off = CallingState { access: CallsAccess::PcConsentRequired, refreshing: true, ..state };
    assert_eq!(calls_state_json(&off, &wait, &request, false)["updating"], false);
}

#[test]
fn the_connection_notice_needs_both_switches_on() {
    let state =
        CallControlState { offered: true, pc_consent: true, enabled: true, status: CallControlStatus::NotPaired };
    assert_eq!(connection_notice(&state), Some("notPaired"));
    assert_eq!(connection_notice(&CallControlState { enabled: false, ..state }), None);
    assert_eq!(connection_notice(&CallControlState { pc_consent: false, ..state }), None);
    assert_eq!(connection_notice(&CallControlState { status: CallControlStatus::Ready, ..state }), None);
    assert_eq!(
        connection_notice(&CallControlState { status: CallControlStatus::BluetoothOff, ..state }),
        Some("bluetoothOff")
    );
}

#[test]
fn a_pairing_answer_is_true_or_false() {
    assert_eq!(pairing_answer("true"), Some(true));
    assert_eq!(pairing_answer("false"), Some(false));
    assert_eq!(pairing_answer("yes"), None);
    assert_eq!(pairing_answer(""), None);
}

#[test]
fn a_reply_reads_its_call_number_and_text() {
    assert_eq!(
        reply_of(r#"{"id": 9, "number": "+821023456789", "text": "Later"}"#),
        Some((9, "+821023456789".to_owned(), "Later".to_owned()))
    );
    assert_eq!(reply_of(r#"{"id": "9", "number": "+82", "text": "x"}"#), None);
    assert_eq!(reply_of("9"), None);
}

#[test]
fn contact_labels_are_lowercase_with_a_generic_label_per_kind() {
    assert_eq!(label(&ContactFieldLabel::WorkMobile, "phone"), "work mobile");
    assert_eq!(label(&ContactFieldLabel::Generic, "homepage"), "homepage");
    assert_eq!(label(&ContactFieldLabel::Custom { label: "Lab".to_owned() }, "phone"), "Lab");
}

#[test]
fn a_card_keeps_each_numbers_address_and_shown_form() {
    let card = ContactCard {
        contact_id: 11,
        name: "Minji Kim".to_owned(),
        nickname: String::new(),
        company: String::new(),
        job_title: String::new(),
        notes: String::new(),
        has_thumbnail: false,
        phone_numbers: vec![phonelink_engine::api::contacts::ContactCardNumber {
            number: "+821023456789".to_owned(),
            shown: "010-2345-6789".to_owned(),
            label: ContactFieldLabel::Mobile,
        }],
        email_addresses: vec![ContactCardField {
            value: "minji@example.com".to_owned(),
            label: ContactFieldLabel::Generic,
        }],
        postal_addresses: Vec::new(),
        urls: Vec::new(),
        dates: Vec::new(),
    };
    let json = card_json(&card, None);
    assert_eq!(json["phones"][0], json!({ "label": "mobile", "value": "010-2345-6789", "number": "+821023456789" }));
    assert_eq!(json["emails"][0]["label"], "email");
    assert_eq!(json["photo"], Value::Null);
}

#[test]
fn a_listed_contact_keeps_its_search_terms_apart() {
    let contact = ContactListItem {
        contact_id: 3,
        name: "Alex".to_owned(),
        has_thumbnail: false,
        search_terms: vec!["010-1234".to_owned(), "alex@example.com".to_owned()],
    };
    assert_eq!(contact_item_json(&contact, None)["terms"], json!(["010-1234", "alex@example.com"]));
}

#[test]
fn contacts_sync_only_while_allowed_and_behind() {
    let state = ContactsState {
        capable: true,
        pc_consent: true,
        access: ContactsAccess::Granted,
        up_to_date: false,
        last_updated_ms: None,
    };
    let wait = SyncWait::default();
    let request = PhoneRequest { pending: true, timed_out: false };
    let json = contacts_state_json(&state, &wait, &request, false);
    assert_eq!(json["syncing"], true);
    assert_eq!(json["requesting"], true);
    let denied = ContactsState { access: ContactsAccess::DeviceConsentDenied, ..state };
    let json = contacts_state_json(&denied, &wait, &request, false);
    assert_eq!(json["syncing"], false);
    assert_eq!(json["access"], "permissionDenied");
}

#[test]
fn an_active_call_says_its_state_and_role() {
    let mut talking = call(4, ActiveCallStatus::Talking, ActiveCallRole::Secondary);
    talking.talking_since_ms = Some(1_000);
    let json = active_call_json(&talking, None);
    assert_eq!(json["status"], "talking");
    assert_eq!(json["role"], "secondary");
    assert_eq!(json["talkingSinceMs"], 1_000);
}

#[test]
fn the_ring_is_two_soft_tones_in_two_seconds() {
    let period = ring_period();
    assert_eq!(period.len(), (RING_RATE * RING_PERIOD_MS / 1000) as usize);
    let loudest = period.iter().map(|sample| sample.unsigned_abs()).max().unwrap_or(0);
    assert!(loudest > 4_000 && loudest < u16::MAX / 4, "loudest sample {loudest}");
    assert_eq!(period[0], 0);
    assert!(period[RING_RATE as usize..].iter().all(|sample| *sample == 0), "the second half is silent");
}

#[test]
fn bluetooth_settings_open_in_the_desktops_own_tool() {
    assert_eq!(bluetooth_settings(Desktop::Kde), ("systemsettings", &["kcm_bluetooth"][..]));
    assert_eq!(bluetooth_settings(Desktop::Gnome), ("gnome-control-center", &["bluetooth"][..]));
    assert_eq!(bluetooth_settings(Desktop::Other).0, "blueman-manager");
}
