//! The `SyncNotifications` connect provider and its contribution to the connect message:
//! `AudioInfoSyncEnabled`, the reconcile keys or the separate full sync (a), and the response's
//! `permissions.notifications`, `capabilities.notifications` and `notificationSyncCompleted`.

use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::consts::{capability, platform};
use phonelink_engine::app::{AppOutcome, ConnectKind, ConnectSendResult};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use support::*;

fn connect_payload(w: &World) -> ValueSet {
    payload(w.h.messenger.requests_on("/connect/v1").last().expect("a connect was sent"))
}

async fn connect(w: &World, trigger: &str) -> AppOutcome {
    match w.h.app.send_connect(ConnectKind::Background, trigger).await {
        ConnectSendResult::Sent(r) => r.outcome,
        other => panic!("connect not sent: {other:?}"),
    }
}

async fn seed(w: &World) {
    let items = [item("a", NOW_MS - 1, json!({})), item("b", NOW_MS, json!({}))];
    deliver(&w.h, batch(&["a", "b"], &[3, 3], &items)).await;
}

#[tokio::test(start_paused = true)]
async fn without_ecr_the_connect_starts_a_separate_full_sync() {
    let w = world();
    seed(&w).await;
    w.w.clock.advance(std::time::Duration::from_secs(5));

    assert_eq!(connect(&w, "EngineStart").await, AppOutcome::Success);
    let vs = connect_payload(&w);
    assert_eq!(vs.object("configuration").unwrap().bool("AudioInfoSyncEnabled"), Some(true));
    for k in
        ["operation", "notificationKeysCount", "notificationKeys", "postTimes", "supportsNotificationSyncCompletion"]
    {
        assert!(!vs.contains_key(k), "{k}");
    }

    w.h.messenger.wait_requests(2).await;
    settle().await;
    let syncs = full_syncs(&w.h);
    assert_eq!(syncs.len(), 1);
    let req = &syncs[0];
    assert_eq!(trace_names(req), ("EngineStart".into(), "ConnectToDeviceNotifications".into()));
    assert_eq!(req.priority, 100);
    assert_eq!(payload(req).get("notificationKeys"), Some(&Value::StringArray(vec!["b".into(), "a".into()])));
    assert_eq!(w.n.status().unwrap().last_updated_ms, Some(NOW_MS + 5_000), "(a) records last updated on Success");
    assert!(permission_requests(&w.h).is_empty(), "no silent check after (a)");
}

#[tokio::test(start_paused = true)]
async fn with_ecr_the_connect_carries_the_reconcile_keys() {
    let w = world();
    seed(&w).await;
    w.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("notifications", 2i32)),
    ));

    connect(&w, "EngineStart").await;
    settle().await;
    assert_eq!(full_syncs(&w.h).len(), 1, "the first connect knew no capabilities yet");

    connect(&w, "AppActivated").await;
    settle().await;

    let vs = connect_payload(&w);
    assert_eq!(vs.i32("operation"), Some(3));
    assert_eq!(vs.i32("notificationKeysCount"), Some(2));
    assert_eq!(vs.get("notificationKeys"), Some(&Value::StringArray(vec!["b".into(), "a".into()])));
    assert_eq!(vs.get("postTimes"), Some(&Value::Int64Array(vec![NOW_MS, NOW_MS - 1])));
    assert_eq!(vs.bool("supportsNotificationSyncCompletion"), Some(true));
    assert_eq!(full_syncs(&w.h).len(), 1, "no separate full sync with ECR");
}

#[tokio::test(start_paused = true)]
async fn with_either_permission_off_only_the_audio_switch_is_added() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    seed(&w).await;
    connect(&w, "EngineStart").await;
    settle().await;

    let vs = connect_payload(&w);
    assert_eq!(vs.object("configuration").unwrap().bool("AudioInfoSyncEnabled"), Some(true));
    assert!(!vs.contains_key("operation"));
    assert!(full_syncs(&w.h).is_empty());

    let w = world();
    w.n.set_pc_permission(false).unwrap();
    w.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("notifications", 2i32)),
    ));

    connect(&w, "EngineStart").await;
    connect(&w, "EngineStart").await;
    settle().await;

    assert!(!connect_payload(&w).contains_key("operation"));
    assert!(full_syncs(&w.h).is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_response_sets_the_device_permission_and_the_capabilities() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    // Mistyped values are ignored.
    w.h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("permissions", ValueSet::new().with("notifications", 1i32))
            .with("capabilities", ValueSet::new().with("notifications", "12")),
    ));

    connect(&w, "EngineStart").await;
    let status = w.n.status().unwrap();
    assert!(!status.device_permission);
    assert_eq!(status.capabilities, 0);

    w.h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("result", 1i32)
            .with("permissions", ValueSet::new().with("notifications", true))
            .with("capabilities", ValueSet::new().with("notifications", 12i32)),
    ));
    connect(&w, "EngineStart").await;
    let status = w.n.status().unwrap();
    assert!(status.device_permission, "applied whatever the result");
    assert_eq!(status.capabilities, 12);

    // The grant is a device-permission change with the PC permission on: full sync (b).
    w.h.messenger.answer_all(result_response(0));
    settle().await;
    let syncs = full_syncs(&w.h);
    assert_eq!(syncs.len(), 1);
    assert_eq!(trace_names(&syncs[0]).0, "NotificationsPermissionsChanged");
}

#[tokio::test(start_paused = true)]
async fn a_reconcile_the_phone_finished_before_replying_records_last_updated() {
    // Device permission off: no full sync (a) runs to record the time itself.
    let w = world_with(Setup { device_permission: false, ..Setup::default() });

    for (completed, why) in
        [(Value::Int32(1), "a mistyped value is ignored"), (Value::Bool(false), "false records nothing")]
    {
        w.h.messenger.answer_all(response_with(
            ValueSet::new().with("result", 0i32).with("notificationSyncCompleted", completed),
        ));
        connect(&w, "EngineStart").await;
        assert_eq!(w.n.status().unwrap().last_updated_ms, None, "{why}");
    }

    w.w.clock.advance(std::time::Duration::from_secs(5));
    w.h.messenger
        .answer_all(response_with(ValueSet::new().with("result", 0i32).with("notificationSyncCompleted", true)));
    connect(&w, "EngineStart").await;
    assert_eq!(w.n.status().unwrap().last_updated_ms, Some(NOW_MS + 5_000));
}

#[tokio::test(start_paused = true)]
async fn the_audio_switch_follows_the_capability_and_an_ios_phone_gets_nothing() {
    let w = world();
    w.h.app.update_phone_record(|r| r.capability_bits = capability::NOTIFICATIONS_LISTENER);

    connect(&w, "EngineStart").await;
    assert_eq!(connect_payload(&w).object("configuration").unwrap().bool("AudioInfoSyncEnabled"), Some(false));

    let w = world();
    w.h.app.update_phone_record(|r| r.platform = platform::IOS);
    w.h.messenger.answer_all(response_with(
        ValueSet::new().with("result", 0i32).with("capabilities", ValueSet::new().with("notifications", 2i32)),
    ));

    connect(&w, "EngineStart").await;
    settle().await;

    assert!(!connect_payload(&w).object("configuration").unwrap().contains_key("AudioInfoSyncEnabled"));
    assert!(full_syncs(&w.h).is_empty());
    assert_eq!(w.n.status().unwrap().capabilities, 0, "not applied for iOS");
}
