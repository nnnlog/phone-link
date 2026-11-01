//! The messaging feature and its runtime: installed inside a tokio runtime (it fails loudly
//! otherwise) and running every later task there, whichever thread triggers it — a notification
//! response routed from a non-tokio thread still sends its reply.

use crate::support::app_world;
use crate::support::feature_world;
use crate::support::messaging_world as support;
use phonelink_engine::config::features::MessagingConfig;

use std::sync::Arc;

use app_world::*;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::messaging::edge::standins::{FakeFileTypes, FixedProximity};
use phonelink_engine::features::messaging::edge::testing::FakeImaging;
use phonelink_engine::features::messaging::{Messaging, MessagingEdges, ProximityState};
use phonelink_engine::features::shared::notifier::NotificationResponse;
use phonelink_engine::features::{ContactDirectory, FeatureError, StoreLocation};
use support::*;

#[test]
fn installing_outside_a_tokio_runtime_fails() {
    let h = Harness::new();
    let w = feature_world::feature_world_at(&h, StoreLocation::Memory, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)));
    let edges = MessagingEdges {
        imaging: FakeImaging::new(),
        file_types: Arc::new(FakeFileTypes),
        proximity: FixedProximity::new(ProximityState::Connected),
    };
    let contacts: Arc<dyn ContactDirectory> = FakeContacts::with(&[]);

    let r = Messaging::install(&w.env, MessagingConfig::default(), Some(contacts), edges);
    assert!(matches!(r, Err(FeatureError::Programming(_))), "installed without a runtime");
}

#[tokio::test(start_paused = true)]
async fn a_reply_routed_from_a_non_tokio_thread_is_sent() {
    let wd = world();
    let item = ValueSet::new()
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "hello")]))
        .with_all(conversations(&[(5, &["+1"])]));
    push(&wd.h, item_msg(item)).await;

    let n = wd.w.notifier.posted().remove(0);
    let router = wd.w.responses.clone();
    std::thread::spawn(move || {
        router.route(
            &n.id,
            &n.user_info,
            NotificationResponse::Reply { action_id: "reply".into(), text: "sure".into() },
        );
    })
    .join()
    .unwrap();

    wd.h.messenger.wait_requests(1).await;
    let reply = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(reply.i32("requestType"), Some(2));
    assert_eq!(reply.i64("threadId"), Some(5));
    assert_eq!(reply.str("body"), Some("sure"));
}

#[tokio::test(start_paused = true)]
async fn ui_triggers_from_a_non_tokio_thread_run_on_the_runtime() {
    let wd = world();
    let item = ValueSet::new()
        .with("settings", ValueSet::new().with("markAsReadAvailable", true))
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]))
        .with_all(conversations(&[(5, &["+1"])]));
    push(&wd.h, item_msg(item)).await;
    wd.h.messenger.wait_requests(1).await; // the RCS-flip sync request

    let m = wd.m.clone();
    std::thread::spawn(move || {
        m.on_conversation_selected(Some(5));
        m.on_messages_view_changed(true);
    })
    .join()
    .unwrap();

    wd.h.messenger.wait_requests(2).await;
    let mark = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(mark.i32("requestType"), Some(3));
}
