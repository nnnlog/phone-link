//! The contacts connect provider: its contribution to the connect message and its sequence numbers.

use crate::support::app_world;
use crate::support::contacts_world;

use app_world::*;
use contacts_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{ConnectKind, ConnectSendResult};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::contacts::ContactsEvent;
use phonelink_engine::seams::PlatformRequest;

const NOT_CAPABLE: &str = r#"{"yppCapabilities":32,"registeredPermissionTypes":"1"}"#;

fn connect_payload(h: &Harness) -> ValueSet {
    let reqs: Vec<PlatformRequest> = h.messenger.requests_on("/connect/v1");
    valueset::decode(&reqs.last().expect("a connect was sent").message.payload).unwrap()
}

async fn connect(h: &Harness) {
    match h.app.send_connect(ConnectKind::Background, "Test").await {
        ConnectSendResult::Sent(_) => {}
        other => panic!("connect not sent: {other:?}"),
    }
}

async fn complete_session(world: &World, numbers: [i64; 6]) {
    let push = item(ValueSet::new().with("syncID", "seed").with("sequenceNumbers", seq(numbers)));
    assert_eq!(send(&world.h, push).await, result(0));
}

#[tokio::test(start_paused = true)]
async fn a_capable_phone_gets_the_flags_and_the_current_sequence_numbers() {
    let world = world();
    complete_session(&world, [4, 5, -1, -1, 0, -1]).await;
    connect(&world.h).await;

    let payload = connect_payload(&world.h);
    let config = payload.object("configuration").unwrap();
    assert_eq!(config.get("ContactsV2Sync"), Some(&Value::Bool(true)));
    assert_eq!(config.get("ContactThumbnails"), Some(&Value::Bool(true)));

    // The contacts keys sit between `CallLogs` and `fileTransferToPcRetryEnabled`.
    let keys: Vec<_> = config.keys().collect();
    let at = keys.iter().position(|k| *k == "ContactsV2Sync").unwrap();
    assert_eq!(
        &keys[at - 1..at + 3],
        ["CallLogs", "ContactsV2Sync", "ContactThumbnails", "fileTransferToPcRetryEnabled"]
    );

    let numbers = payload.object("sequenceNumbers").unwrap();
    for (k, v) in [
        ("contact", 4),
        ("phonenumber", 5),
        ("emailaddress", -1),
        ("postaladdress", -1),
        ("contactdate", 0),
        ("contacturl", -1),
    ] {
        assert_eq!(numbers.get(k), Some(&Value::Int64(v)), "{k}");
    }
    // The shared layer's own entries stay.
    assert_eq!(numbers.get("browserHistory"), Some(&Value::Int64(-1)));
}

#[tokio::test(start_paused = true)]
async fn a_new_store_sends_minus_one() {
    let world = world();
    connect(&world.h).await;

    let numbers = connect_payload(&world.h).object("sequenceNumbers").unwrap().clone();
    for k in ["contact", "phonenumber", "emailaddress", "postaladdress", "contactdate", "contacturl"] {
        assert_eq!(numbers.get(k), Some(&Value::Int64(-1)), "{k}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_is_not_contacts_capable_gets_the_flags_only() {
    let world = world_with_custom_data(NOT_CAPABLE);
    complete_session(&world, [4, 5, 6, 7, 8, 9]).await;
    connect(&world.h).await;

    let payload = connect_payload(&world.h);
    assert_eq!(payload.object("configuration").unwrap().bool("ContactsV2Sync"), Some(true));
    assert_eq!(payload.object("configuration").unwrap().bool("ContactThumbnails"), Some(true));
    let numbers = payload.object("sequenceNumbers").unwrap();
    assert!(!numbers.contains_key("contact"));
    assert!(!numbers.contains_key("contacturl"));
}

#[tokio::test(start_paused = true)]
async fn the_response_sets_the_expected_numbers_only_when_capable_and_result_0() {
    let world = world();
    let mut rx = world.contacts.subscribe();

    // Top-level keys; a mistyped or missing key stores -1.
    world.h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("contact", 50i64)
            .with("phonenumber", 3i32)
            .with("postaladdress", 7i64)
            .with("sequenceNumbers", seq([9, 9, 9, 9, 9, 9])),
    ));
    connect(&world.h).await;

    let state = sync_state(&world);
    assert_eq!(state.expected, [50, -1, -1, 7, -1, -1]);
    assert!(!state.up_to_date, "current -1 < expected 50");
    assert!(drain(&mut rx).contains(&ContactsEvent::SyncStateChanged { phone: phone() }));

    // result 1, or a result that is not an Int32: nothing stored.
    world.h.messenger.answer_all(response_with(ValueSet::new().with("result", 1i32).with("contact", 99i64)));
    connect(&world.h).await;
    world.h.messenger.answer_all(response_with(ValueSet::new().with("result", Value::Int64(0)).with("contact", 98i64)));
    connect(&world.h).await;
    world.h.messenger.answer_all(response_with(ValueSet::new().with("contact", 97i64)));
    connect(&world.h).await;
    assert_eq!(sync_state(&world).expected[0], 50);

    // Up to date once current reaches expected for every stored type.
    complete_session(&world, [50, 1, 1, 7, 1, 1]).await;
    assert!(sync_state(&world).up_to_date);
    complete_session(&world, [49, -1, -1, -1, -1, -1]).await;
    assert!(!sync_state(&world).up_to_date);
}

#[tokio::test(start_paused = true)]
async fn a_new_store_is_up_to_date() {
    let world = world();
    let state = sync_state(&world);
    assert_eq!((state.current, state.expected), ([-1; 6], [-1; 6]));
    assert!(state.up_to_date);
    assert_eq!(state.last_updated, None);
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_is_not_capable_stores_no_expected_numbers() {
    let world = world_with_custom_data(NOT_CAPABLE);
    world.h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32).with("contact", 50i64)));
    connect(&world.h).await;
    assert_eq!(sync_state(&world).expected, [-1; 6]);
}

#[tokio::test(start_paused = true)]
async fn capability_bits_from_the_same_response_count() {
    // Not capable before the connect; the response reports Contacts (0x800): DeviceInfo applies
    // first, so the expected numbers are stored.
    let world = world_with_custom_data(NOT_CAPABLE);
    world.h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("capabilities", ValueSet::new().with("registeredPermissionTypes", 0x801i32))
            .with("contact", 50i64),
    ));
    connect(&world.h).await;
    assert_eq!(sync_state(&world).expected[0], 50);
    // The request of that connect was built before: no sequence numbers in it.
    assert!(!connect_payload(&world.h).object("sequenceNumbers").unwrap().contains_key("contact"));
}

#[tokio::test(start_paused = true)]
async fn permissions_contacts_is_applied_whatever_the_result() {
    let world = world();
    let permitted = || world.h.app.phone_record().unwrap().permission_or(permission::CONTACTS, true);
    assert!(permitted(), "default true");

    world.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 1i32).with("permissions", ValueSet::new().with("contacts", false)),
    ));
    connect(&world.h).await;
    assert!(!permitted());

    // A mistyped value is ignored.
    world.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("permissions", ValueSet::new().with("contacts", 1i32)),
    ));
    connect(&world.h).await;
    assert!(!permitted());

    world.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("permissions", ValueSet::new().with("contacts", true)),
    ));
    connect(&world.h).await;
    assert!(permitted());
}

#[tokio::test(start_paused = true)]
async fn the_contacts_permission_and_pc_consent_gate_no_connect_key() {
    let world = world();
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    world.contacts.set_pc_consent(&phone(), false).unwrap();
    connect(&world.h).await;

    let payload = connect_payload(&world.h);
    assert_eq!(payload.object("configuration").unwrap().bool("ContactsV2Sync"), Some(true));
    let numbers = payload.object("sequenceNumbers").unwrap();
    for k in ["contact", "phonenumber", "emailaddress", "postaladdress", "contactdate", "contacturl"] {
        assert_eq!(numbers.get(k), Some(&Value::Int64(-1)), "{k}");
    }
}

#[tokio::test(start_paused = true)]
async fn the_response_capabilities_contacts_are_stored() {
    let world = world();
    let capabilities = || world.contacts.permission_state(&phone()).unwrap().contacts_capabilities;
    assert_eq!(capabilities(), 0, "default None");

    world.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 1i32).with("capabilities", ValueSet::new().with("contacts", 5i32)),
    ));
    connect(&world.h).await;
    assert_eq!(capabilities(), 5, "whatever the result");
}
