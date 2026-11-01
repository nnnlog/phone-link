//! The messaging connect provider: its contribution to the connect message, the gating value before
//! any report, and sync trigger (b).

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::messaging_world as support;
use phonelink_engine::config::features::MessagingConfig;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{ConnectKind, ConnectSendResult};
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::testing::MemoryStore;
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::messaging::MessagingEvent;
use support::*;

async fn connect(wd: &World, response: ValueSet) -> ValueSet {
    wd.h.messenger.answer_all(response_with(response));
    let r = wd.h.app.send_connect(ConnectKind::Background, "T").await;
    assert!(matches!(r, ConnectSendResult::Sent(_)), "{r:?}");
    payload(wd.h.messenger.requests_on("/connect/v1").last().unwrap())
}

fn messaging_config(c: &ValueSet) -> serde_json::Value {
    let all = c.object("configuration").unwrap();
    let mut out = ValueSet::new();
    for (k, v) in all.iter().filter(|(k, _)| k.starts_with("Messaging")) {
        out.insert(k, v.clone());
    }
    out.to_json()
}

fn messaging_anchors(c: &ValueSet) -> Vec<(String, i64)> {
    let seq = c.object("sequenceNumbers").unwrap();
    seq.iter()
        .filter(|(k, _)| !matches!(*k, "browserHistory" | "continuityAppContext"))
        .map(|(k, _)| (k.to_string(), seq.i64(k).unwrap()))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn the_pairing_run_presents_no_rcs_and_stores_no_rcs_anchors() {
    let wd = world();
    let response = ValueSet::new()
        .with("result", 0i32)
        .with("sms", 11i64)
        .with("mms", 12i32) // not Int64 ⇒ -1
        .with("conversation", 13i64)
        .with("rcs_chat", 14i64)
        .with("capabilities", ValueSet::new().with("messaging", 0x18i32))
        .with("permissions", ValueSet::new().with("messaging", false))
        .with("settings", ValueSet::new().with("mmsAutoDownloadEnabled", true));
    let c = connect(&wd, response).await;

    assert_eq!(
        messaging_config(&c),
        json!({
            "MessagingSync": {"Bool": true},
            "MessagingMassText": {"Bool": true},
            "MessagingSend": {"Bool": false},
            "MessagingRcsReceive": {"Bool": false},
            "MessagingRcsSend": {"Bool": false}
        })
    );
    assert_eq!(
        messaging_anchors(&c),
        [("mms", -1), ("sms", -1), ("conversation", -1), ("subscription", -1)]
            .map(|(k, v)| (k.to_string(), v))
            .to_vec()
    );

    // Applied: capabilities | 0x2, the permission, the phone's MMS auto-download.
    assert_eq!(wd.m.predicates().unwrap().capabilities, 0x1A);
    assert!(!wd.m.phone_permission().unwrap());
    assert!(!wd.h.app.phone_record().unwrap().permission_or(permission::MESSAGES, true));
    assert!(wd.m.mms_auto_download_on_phone().unwrap());

    // Expected anchors: sms 11 (> current -1), mms/subscription -1; no RCS anchors (read before the
    // capabilities made RCS receive true).
    assert!(!wd.m.sync_state().unwrap().up_to_date);

    // The next connect presents RCS (0x2 forced on, contract 3.99) and mark-as-read, Send.
    let c = connect(&wd, ValueSet::new().with("result", 0i32).with("rcs_chat", 5i64)).await;
    assert_eq!(
        messaging_config(&c),
        json!({
            "MessagingSync": {"Bool": true},
            "MessagingMassText": {"Bool": true},
            "MessagingSend": {"Bool": true},
            "MessagingRcsReceive": {"Bool": true},
            "MessagingRcsSend": {"Bool": false},
            "MessagingRcsUnsupportedMessageTypes": {"Int32Array": [10, 9]}
        })
    );
    let anchors: Vec<String> = messaging_anchors(&c).into_iter().map(|(k, _)| k).collect();
    assert_eq!(
        anchors,
        ["mms", "sms", "rcs_chat", "rcs_filetransfer", "conversation", "subscription", "rcs_conversation"]
    );
}

#[tokio::test(start_paused = true)]
async fn anchors_need_result_zero_and_participants_force_minus_one() {
    let wd = world();
    // A failed connect: anchors not stored, the rest applied.
    connect(
        &wd,
        ValueSet::new()
            .with("result", 1i32)
            .with("sms", 9i64)
            .with("capabilities", ValueSet::new().with("messaging", 0x20i32)),
    )
    .await;

    assert!(wd.m.sync_state().unwrap().up_to_date, "no expected anchor stored");
    assert_eq!(wd.m.predicates().unwrap().capabilities, 0x22);

    // Store current anchors through a completed exchange …
    let md = ValueSet::new()
        .with("messagingSmsChecksums", Vec::<i64>::new())
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("settings", ValueSet::new().with("capabilities", 0x20i32))
        .with(
            "sequenceNumbers",
            ValueSet::new().with("sms", 1i64).with("mms", 2i64).with("conversation", 3i64).with("rcs_chat", 4i64),
        );

    // Participants with an empty table: a forced resync (no checksums needed, nothing listed).
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(result_of(&v), Some(0));

    // … and the connect forces sms, mms, rcs_chat, rcs_filetransfer to -1 while the table is empty.
    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;
    let anchors = messaging_anchors(&c);
    assert_eq!(
        anchors,
        [
            ("mms", -1),
            ("sms", -1),
            ("rcs_chat", -1),
            ("rcs_filetransfer", -1),
            ("conversation", 3),
            ("subscription", -1),
            ("rcs_conversation", -1)
        ]
        .map(|(k, v)| (k.to_string(), v))
        .to_vec()
    );
}

#[tokio::test(start_paused = true)]
async fn a_phone_that_is_not_messaging_capable_gets_no_anchors() {
    let h = Harness::with_custom_data(Some(r#"{"yppCapabilities":32,"registeredPermissionTypes":"1"}"#));
    let wd = world_on(
        h,
        StoreLocation::Memory,
        MessagingConfig::default(),
        device_item(Some(r#"{"yppCapabilities":32,"registeredPermissionTypes":"1"}"#)),
    );

    let c = connect(&wd, ValueSet::new().with("result", 0i32).with("sms", 4i64)).await;

    assert!(messaging_anchors(&c).is_empty());
    assert_eq!(messaging_config(&c).as_object().unwrap().len(), 5, "configuration is always sent");
    assert!(wd.m.sync_state().unwrap().up_to_date, "anchors not applied");
}

#[tokio::test(start_paused = true)]
async fn an_rcs_flip_sends_a_background_sync_request() {
    let mut wd = world();
    // The pairing run has no RCS; the first report (0x2 forced on) flips RCS receive.
    connect(&wd, ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("messaging", 0i32)))
        .await;
    wd.h.messenger.wait_requests(2).await;

    let sync = chat_requests(&wd.h).pop().expect("a sync request");
    assert_eq!(sync.get("requestType"), None);
    assert_eq!(sync.bool("isuserinitiated"), Some(false));
    assert_eq!(
        sync.string_array("subcontentTypes").unwrap(),
        [
            "messages",
            "messagesMms",
            "conversations",
            "subscriptions",
            "messagesMmsMedia",
            "messagesRcsChat",
            "messagesRcsFileTransfer",
            "conversationsRcs"
        ]
    );
    assert!(drain(&mut wd.events).iter().any(|e| matches!(e, MessagingEvent::SettingsChanged { .. })));

    // The same value again flips nothing: no further request.
    let before = wd.h.messenger.requests().len();
    connect(&wd, ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("messaging", 0i32)))
        .await;
    tokio::task::yield_now().await;
    assert_eq!(wd.h.messenger.requests().len(), before + 1, "only the connect");

    // RcsSend appears: RCS send flips.
    connect(&wd, ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("messaging", 0x4i32)))
        .await;
    wd.h.messenger.wait_requests(before + 3).await;
    assert!(chat_requests(&wd.h).len() >= 2);
}

#[tokio::test(start_paused = true)]
async fn a_later_run_presents_rcs_from_the_first_connect() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    drop(world_with(Harness::new(), location.clone(), MessagingConfig::default()));
    let wd = world_with(Harness::new(), location, MessagingConfig::default());

    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(c.object("configuration").unwrap().bool("MessagingRcsReceive"), Some(true));
}

#[tokio::test(start_paused = true)]
async fn mistyped_connect_keys_are_ignored() {
    let wd = world();
    connect(
        &wd,
        ValueSet::new()
            .with("result", 0i32)
            .with("capabilities", ValueSet::new().with("messaging", "x"))
            .with("permissions", ValueSet::new().with("messaging", 1i32))
            .with("settings", 5i32),
    )
    .await;

    assert_eq!(wd.m.predicates().unwrap().capabilities, 0);
    assert!(wd.m.phone_permission().unwrap(), "default granted");
}

#[tokio::test(start_paused = true)]
async fn a_connect_result_of_another_type_applies_no_anchor() {
    let wd = world();
    connect(&wd, ValueSet::new().with("result", 0i64).with("sms", 5i64)).await;
    assert!(wd.m.sync_state().unwrap().up_to_date, "no expected anchor stored");
}

#[tokio::test(start_paused = true)]
async fn rcs_expected_anchors_follow_rcs_receive_before_the_capabilities() {
    let wd = world();
    // The pairing run: RCS receive is off until this response's capabilities turn it on, so its
    // RCS anchor is not stored.
    connect(
        &wd,
        ValueSet::new()
            .with("result", 0i32)
            .with("rcs_chat", 5i64)
            .with("capabilities", ValueSet::new().with("messaging", 0i32)),
    )
    .await;

    assert!(wd.m.predicates().unwrap().rcs_receive());
    assert!(wd.m.sync_state().unwrap().up_to_date, "the RCS anchor was not stored");

    // Now RCS receive holds: the expected RCS anchor is stored (and the PC is behind).
    connect(&wd, ValueSet::new().with("result", 0i32).with("rcs_chat", 5i64)).await;
    assert!(!wd.m.sync_state().unwrap().up_to_date);
}

#[tokio::test(start_paused = true)]
async fn the_default_capabilities_are_fixed_when_the_store_is_first_opened() {
    // Before any report the default is derived once, when the phone's record is loaded —
    // the engine fixes it when it first opens the phone's messaging store in a run. A later run that
    // starts with a reported contract 3.8 keeps `0` although the phone then reports 3.99.
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    let secure = Arc::new(MemoryStore::new());
    let harness = || {
        let h = Harness::without_phone(secure.clone(), EngineConfig::default());
        h.app.set_phone(phone(), &device_item(Some(ASYNC_TRUST_CUSTOM_DATA))).unwrap();
        h
    };

    {
        // The pairing run: the phone reports contract 3.8 and no capabilities.
        let wd = world_with(harness(), location.clone(), MessagingConfig::default());
        connect(&wd, ValueSet::new().with("result", 0i32).with("contractVersion", 3.8f64)).await;
    }

    let wd = world_with(harness(), location, MessagingConfig::default());
    assert_eq!(wd.h.app.phone_record().unwrap().reported_contract_version, Some(3.8));
    connect(&wd, ValueSet::new().with("result", 0i32).with("contractVersion", 3.99f64)).await;
    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;

    assert_eq!(c.object("configuration").unwrap().bool("MessagingRcsReceive"), Some(false));
    assert_eq!(wd.m.predicates().unwrap().capabilities, 0, "the run's default");

    // A report replaces the default.
    connect(&wd, ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("messaging", 0i32)))
        .await;
    assert!(wd.m.predicates().unwrap().rcs_receive());
}

/// The phone's messaging store file under `dir`.
fn messaging_store(dir: &std::path::Path) -> std::path::PathBuf {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                pending.push(p);
            } else if p.ends_with("messaging.sqlite3") {
                return p;
            }
        }
    }
    panic!("no messaging store under {}", dir.display());
}

#[tokio::test(start_paused = true)]
async fn a_store_failure_while_building_the_connect_keeps_the_configuration() {
    // A store that cannot be read loses only its stored values: the configuration keys are sent,
    // the sequence numbers are not, and nothing is invented.
    let dir = tempfile::tempdir().unwrap();
    let wd = world_with(Harness::new(), StoreLocation::Directory(dir.path().to_path_buf()), MessagingConfig::default());
    connect(&wd, ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("messaging", 0x14i32)))
        .await;

    let file = messaging_store(dir.path());

    // The anchors cannot be read.
    rusqlite::Connection::open(&file).unwrap().execute_batch("DROP TABLE anchor;").unwrap();
    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;

    let configuration = c.object("configuration").unwrap();
    assert_eq!(configuration.bool("MessagingSync"), Some(true));
    assert_eq!(configuration.bool("MessagingSend"), Some(true), "the stored capabilities 0x16");
    assert_eq!(configuration.bool("MessagingRcsReceive"), Some(true));
    assert!(messaging_anchors(&c).is_empty(), "no sequence numbers");

    // The capabilities cannot be read either: the configuration of a phone that has reported none
    // (the pairing run's default, 0).
    rusqlite::Connection::open(&file).unwrap().execute_batch("DROP TABLE phone_state;").unwrap();
    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;
    assert_eq!(
        messaging_config(&c),
        json!({
            "MessagingSync": {"Bool": true},
            "MessagingMassText": {"Bool": true},
            "MessagingSend": {"Bool": false},
            "MessagingRcsReceive": {"Bool": false},
            "MessagingRcsSend": {"Bool": false}
        })
    );
    assert!(messaging_anchors(&c).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_rcs_conversation_anchor_is_never_forced() {
    // Like `conversation` and `subscription`, `rcs_conversation` is the stored current anchor even
    // while a phone that tracks participants has none stored (`sms` is forced to -1).
    let wd = world();
    let md = ValueSet::new()
        .with("messagingSmsChecksums", Vec::<i64>::new())
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("settings", ValueSet::new().with("capabilities", 0x20i32))
        .with(
            "sequenceNumbers",
            ValueSet::new().with("sms", 1i64).with("subscription", 5i64).with("rcs_conversation", 7i64),
        );

    assert_eq!(result_of(&push(&wd.h, metadata_msg(md)).await), Some(0));
    let c = connect(&wd, ValueSet::new().with("result", 0i32)).await;

    let anchors: HashMap<String, i64> = messaging_anchors(&c).into_iter().collect();
    assert_eq!(anchors["sms"], -1, "forced");
    assert_eq!(anchors["subscription"], 5);
    assert_eq!(anchors["rcs_conversation"], 7);
}
