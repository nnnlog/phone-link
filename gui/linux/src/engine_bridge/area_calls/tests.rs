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
use super::phone_call_notice::{notice_id, notice_title};
use super::presentation::{Presentation, PresentationChange};
use super::ringer::{RING_PERIOD_MS, RING_RATE, ring_period};
use super::routing::{key_of, pairing_answer, reply_of};
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
        on_computer: false,
        muted: false,
        can_mute: false,
        can_hold: false,
        can_use_keypad: false,
        can_transfer: false,
    }
}

/// A call the user placed or answered on the phone, which this computer could take.
fn on_phone(id: u64, status: ActiveCallStatus) -> ActiveCall {
    ActiveCall { can_transfer: true, ..call(id, status, ActiveCallRole::Untracked) }
}

#[test]
fn a_ringing_call_shows_rings_and_gets_a_lifetime() {
    let mut presentation = Presentation::default();
    let calls = [call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)];
    let change = presentation.update(&calls, true, &mut || false);
    assert_eq!(
        change,
        PresentationChange { lifetimes: vec![(7, 0)], ring: Some(true), ..PresentationChange::default() }
    );
    assert_eq!(presentation.popup(&calls).map(|c| c.id).collect::<Vec<_>>(), [7]);

    // The same call again starts nothing new.
    assert_eq!(presentation.update(&calls, true, &mut || false), PresentationChange::default());
}

#[test]
fn a_closed_or_expired_popup_stops_the_ring_and_stays_closed() {
    let mut presentation = Presentation::default();
    let calls = [call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)];
    presentation.update(&calls, true, &mut || false);

    assert!(!presentation.expire(7, 5), "another token does not expire it");
    assert!(presentation.expire(7, 0));
    assert_eq!(presentation.update(&calls, true, &mut || false).ring, Some(false));
    assert_eq!(presentation.popup(&calls).count(), 0);
    assert!(!presentation.expire(7, 0), "a hidden call expires once");
}

#[test]
fn a_call_that_rings_while_the_desktop_is_not_to_be_disturbed_is_never_shown() {
    let mut presentation = Presentation::default();
    let ringing = [call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)];
    let mut asked = 0;
    let mut undisturbed = || {
        asked += 1;
        true
    };

    assert_eq!(presentation.update(&ringing, true, &mut undisturbed), PresentationChange::default());
    assert_eq!(presentation.popup(&ringing).count(), 0);
    // It stays hidden when the desktop may be disturbed again, and is asked about once.
    assert_eq!(presentation.update(&ringing, true, &mut || false), PresentationChange::default());
    presentation.update(&[call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary)], true, &mut undisturbed);
    assert_eq!(asked, 1, "only a call that starts ringing asks");
}

#[test]
fn an_ended_call_is_forgotten() {
    let mut presentation = Presentation::default();
    presentation.update(&[call(7, ActiveCallStatus::Incoming, ActiveCallRole::Untracked)], true, &mut || false);
    presentation.hidden.insert(7);
    presentation.update(&[], true, &mut || false);
    assert!(presentation.hidden.is_empty());
    assert!(!presentation.expire(7, 0), "the ended call's timer does nothing");
}

#[test]
fn the_call_window_shows_while_a_main_call_goes_on() {
    let mut presentation = Presentation::default();
    assert!(!presentation.window_shows_failures());

    let first = [call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary)];
    assert!(presentation.update(&first, true, &mut || false).window);
    assert_eq!(presentation.update(&first, true, &mut || false), PresentationChange::default());
    assert!(presentation.window_shows_failures());

    assert!(
        presentation.update(&[call(2, ActiveCallStatus::Dialing, ActiveCallRole::Primary)], true, &mut || false).window
    );
    assert_eq!(presentation.window_call, Some(2));

    assert!(presentation.update(&[], true, &mut || false).window);
    assert_eq!(presentation.window_call, None);
}

#[test]
fn a_call_taken_on_the_phone_is_told_once_and_its_notice_taken_back_when_it_no_longer_applies() {
    let mut presentation = Presentation::default();
    let talking = [on_phone(3, ActiveCallStatus::Talking)];

    assert_eq!(presentation.update(&talking, true, &mut || false).notices, [3]);
    assert_eq!(presentation.update(&talking, true, &mut || false), PresentationChange::default(), "told once");

    // Transferred to this computer: it is the call window's now.
    let transferred = [ActiveCall { on_computer: true, ..call(3, ActiveCallStatus::Talking, ActiveCallRole::Primary) }];
    assert_eq!(presentation.update(&transferred, true, &mut || false).withdrawn, [3]);
    assert!(!presentation.notice_over(3), "the notice is gone already");

    // A notice that ran out is taken back once, and the call's end takes nothing more back.
    assert_eq!(presentation.update(&[on_phone(4, ActiveCallStatus::Dialing)], true, &mut || false).notices, [4]);
    assert!(presentation.notice_over(4));
    assert!(!presentation.notice_over(4));
    assert!(presentation.update(&[], true, &mut || false).withdrawn.is_empty());
}

#[test]
fn a_call_on_the_phone_gets_no_notice_when_it_should_not() {
    // (the call, the other calls, notices allowed)
    let ringing = call(5, ActiveCallStatus::Incoming, ActiveCallRole::Untracked);
    let cannot_take = call(5, ActiveCallStatus::Talking, ActiveCallRole::Untracked);
    let window = call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary);
    let cases = [
        (on_phone(5, ActiveCallStatus::Talking), None, false),
        (on_phone(5, ActiveCallStatus::Talking), Some(window), true),
        (ActiveCall { can_transfer: true, ..ringing }, None, true),
        (cannot_take, None, true),
    ];

    for (subject, other, allowed) in cases {
        let mut presentation = Presentation::default();
        let calls: Vec<ActiveCall> = other.into_iter().chain([subject.clone()]).collect();
        assert!(presentation.update(&calls, allowed, &mut || false).notices.is_empty(), "no notice for {subject:?}");
    }

    // A call seen while notices were off gets none when they are turned on.
    let mut presentation = Presentation::default();
    let talking = [on_phone(5, ActiveCallStatus::Talking)];
    presentation.update(&talking, false, &mut || false);
    assert!(presentation.update(&talking, true, &mut || false).notices.is_empty());
}

#[test]
fn a_notice_names_the_contact_then_the_number() {
    let named = ActiveCall { name: Some("Ada".to_owned()), ..on_phone(6, ActiveCallStatus::Talking) };
    assert_eq!(notice_title(&named), "Ada");
    assert_eq!(notice_title(&on_phone(6, ActiveCallStatus::Talking)), "+82100000006");
    assert_eq!(notice_id(6), "calls/onPhone/6");
}

#[test]
fn a_keypad_key_reads_its_call_and_key() {
    assert_eq!(key_of(r##"{"id": 4, "key": "#"}"##), Some((4, "#".to_owned())));
    assert_eq!(key_of(r#"{"id": 4}"#), None);
    assert_eq!(key_of("4"), None);
}

#[test]
fn a_call_tells_the_window_where_it_is_heard_and_what_acts_on_it() {
    let heard_here = ActiveCall {
        on_computer: true,
        muted: true,
        can_mute: true,
        can_hold: true,
        can_use_keypad: true,
        can_transfer: true,
        ..call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary)
    };
    let json = active_call_json(&heard_here, None);
    for key in ["onComputer", "muted", "canMute", "canHold", "canUseKeypad", "canTransfer"] {
        assert_eq!(json[key], true, "{key}");
    }

    let on_phone = active_call_json(&call(1, ActiveCallStatus::Talking, ActiveCallRole::Primary), None);
    assert_eq!((&on_phone["onComputer"], &on_phone["canTransfer"]), (&json!(false), &json!(false)));
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
    let state = CallControlState {
        offered: true,
        pc_consent: true,
        enabled: true,
        status: CallControlStatus::NotPaired,
        can_take_audio: true,
    };
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
