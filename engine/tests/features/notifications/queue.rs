//! The persistent action queue: actions and replies are stored, sent one at a time with a fresh
//! `correlationVector`, never retried, dropped unsent after 10 minutes, deleted with their
//! notification, and drained when the queue starts.

use crate::assert_matches;
use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;

use std::time::Duration;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::AppOutcome;
use phonelink_engine::features::notifications::{NotificationsError, NotificationsEvent};
use phonelink_engine::features::{NotificationResponse, StoreLocation};
use phonelink_engine::seams::PlatformRequest;
use support::*;

fn actions() -> serde_json::Value {
    json!([
        {"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0},
        {"actionName": "Archive", "isActionInlineReply": false, "actionIndex": 1}
    ])
}

async fn seed(w: &World, keys: &[&str]) {
    let items: Vec<String> =
        keys.iter().map(|k| item(k, NOW_MS, json!({"notificationActions": actions(), "importance": 4}))).collect();
    let ops = vec![3; keys.len()];
    deliver(&w.h, batch(keys, &ops, &items)).await;
}

fn launches(w: &World) -> Vec<PlatformRequest> {
    notification_requests(&w.h).into_iter().filter(|r| payload(r).i32("operation") == Some(5)).collect()
}

/// Holds every operation-5 request until released; answers the rest `0` at once.
fn hold_launches(w: &World) -> Held {
    hold_requests(&w.h, |r| r.message.route() == Some("/notifications") && payload(r).i32("operation") == Some(5))
}

#[tokio::test(start_paused = true)]
async fn an_action_is_stored_then_sent_with_a_fresh_correlation_vector() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_with(Setup { stores: StoreLocation::Directory(dir.path().to_path_buf()), ..Setup::default() });
    seed(&w, &["k1"]).await;
    let mut events = w.n.subscribe();

    w.n.press_action("k1", 1).unwrap();
    w.h.messenger.wait_requests(1).await;
    settle().await;

    let req = &launches(&w)[0];
    let vs = payload(req);
    assert_eq!(vs.str("contentType"), Some("notifications"));
    assert_eq!((vs.str("key"), vs.i32("actionIndex")), (Some("k1"), Some(1)));
    assert!(!vs.contains_key("inlineReplyMessage"));
    assert_eq!(vs.bool("isuserinitiated"), Some(false));
    assert_eq!(req.priority, 100);
    assert_eq!(trace_names(req), ("NotificationsFeedAction".into(), "Notifications".into()));
    let cv = vs.str("correlationVector").unwrap();
    assert_eq!(cv, req.trace.correlation_id);

    // The stored row: its request id is the request as first built, not the one sent.
    let db = dir.path().join("phones").join(format!("{}.Prod", PHONE_ID)).join("notifications.sqlite3");
    let conn = rusqlite::Connection::open(db).unwrap();
    let (request_id, ext1, ext2, status): (String, String, String, String) = conn
        .query_row("SELECT request_id, external_id1, external_id2, status FROM phone_requests", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!(request_id.len(), 16);
    assert_ne!(request_id, cv);
    assert_eq!((ext1.as_str(), ext2.as_str(), status.as_str()), ("k1", "1", "Success"));

    let finished: Vec<_> =
        drain(&mut events).into_iter().filter(|e| matches!(e, NotificationsEvent::ActionFinished { .. })).collect();
    assert_eq!(
        finished,
        [NotificationsEvent::ActionFinished {
            phone: phone(),
            key: "k1".into(),
            reply: false,
            outcome: Some(AppOutcome::Success)
        }]
    );

    // An action the item does not have, or a reply index, is refused; so is an unknown key.
    assert_matches!(w.n.press_action("k1", 7), Err(NotificationsError::UnknownAction));
    assert_matches!(w.n.press_action("k1", 0), Err(NotificationsError::UnknownAction));
    assert_matches!(w.n.press_action("nope", 1), Err(NotificationsError::UnknownNotification));
}

#[tokio::test(start_paused = true)]
async fn a_list_reply_carries_its_text_and_a_failure_posts_no_notice() {
    let w = world();
    seed(&w, &["k1"]).await;
    w.h.messenger.answer_all(result_response(1));
    w.w.notifier.take();

    w.n.reply("k1", 0, "see you").unwrap();
    w.h.messenger.wait_requests(1).await;

    let req = &launches(&w)[0];
    let vs = payload(req);
    assert_eq!(vs.i32("actionIndex"), Some(0));
    assert_eq!(vs.str("inlineReplyMessage"), Some("see you"));
    assert_eq!(trace_names(req), ("NotificationsFeedInlineReply".into(), "Notifications".into()));
    settle().await;
    assert!(w.w.notifier.posted().is_empty(), "only a banner reply's failure is notified");

    // An empty reply sends no inlineReplyMessage.
    w.n.reply("k1", 0, "").unwrap();
    w.h.messenger.wait_requests(2).await;
    assert!(!payload(&launches(&w)[1]).contains_key("inlineReplyMessage"));
    assert_matches!(w.n.reply("k1", 1, "x"), Err(NotificationsError::UnknownAction));
}

#[tokio::test(start_paused = true)]
async fn entries_go_one_at_a_time_in_order_without_retry() {
    let w = world();
    seed(&w, &["a", "b"]).await;
    let held = hold_launches(&w);

    w.n.press_action("a", 1).unwrap();
    w.n.reply("b", 0, "two").unwrap();
    settle().await;
    assert_eq!(launches(&w).len(), 1, "the next waits for the outcome");

    release(&held, 1);
    settle().await;
    let sent = launches(&w);
    assert_eq!(sent.len(), 2);
    assert_eq!(payload(&sent[1]).str("key"), Some("b"));

    release(&held, 0);
    settle().await;
    assert_eq!(launches(&w).len(), 2, "a failed entry is not retried");
}

#[tokio::test(start_paused = true)]
async fn an_entry_older_than_ten_minutes_is_dropped_unsent() {
    let w = world();
    seed(&w, &["a", "b"]).await;
    w.h.messenger.set_responder(|req| {
        if payload(req).i32("operation") == Some(5) { Reply::Hang } else { Reply::Now(result_response(0)) }
    });
    let mut events = w.n.subscribe();
    w.n.press_action("a", 1).unwrap();

    // A banner reply queued behind it.
    let info = std::collections::HashMap::from([
        ("kind".to_string(), "notification".to_string()),
        ("phoneId".to_string(), PHONE_ID.to_string()),
        ("phoneEnv".to_string(), "Prod".to_string()),
        ("key".to_string(), "b".to_string()),
    ]);
    w.w.responses.route(
        &format!("notifications/{}/b", phone()),
        &info,
        NotificationResponse::Reply { action_id: "reply/0/0".into(), text: "late".into() },
    );
    settle().await;

    w.w.clock.advance(Duration::from_secs(10 * 60 + 1));
    // The first request times out (120 s); the reply is then 10 minutes old.
    tokio::time::sleep(Duration::from_secs(121)).await;
    settle().await;
    assert_eq!(launches(&w).len(), 1, "the expired reply is never sent");

    let finished: Vec<_> = drain(&mut events)
        .into_iter()
        .filter_map(|e| match e {
            NotificationsEvent::ActionFinished { key, outcome, .. } => Some((key, outcome)),
            _ => None,
        })
        .collect();
    assert_eq!(finished, [("a".to_string(), Some(AppOutcome::TimedOut)), ("b".to_string(), None)]);

    let notice = w.w.notifier.posted().last().unwrap().clone();
    assert_eq!(notice.id, format!("notifications/{}:reply-failed/b", phone()), "an expired banner reply failed too");
}

#[tokio::test(start_paused = true)]
async fn removals_session_changes_and_clear_all_drop_queued_entries() {
    let w = world();
    seed(&w, &["0|S1|a", "0|S1|b", "0|S2|c", "0|S1|d"]).await;
    let held = hold_launches(&w);

    w.n.press_action("0|S1|a", 1).unwrap();
    for k in ["0|S1|b", "0|S2|c", "0|S1|d"] {
        w.n.press_action(k, 1).unwrap();
    }
    settle().await;

    // The phone removes b; a session change excludes c; clear all takes d (and a).
    deliver(&w.h, batch(&["0|S1|b", "S1"], &[2, 7], &[String::new(), String::new()])).await;
    assert_eq!(w.n.clear_all().await.unwrap(), Some(AppOutcome::Success));
    release(&held, 0);
    settle().await;
    assert_eq!(launches(&w).len(), 1, "every queued entry went with its notification");
}

#[tokio::test(start_paused = true)]
async fn unexpired_entries_left_by_a_previous_run_are_drained_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());
    let first = world_with(Setup { stores: stores.clone(), ..Setup::default() });

    seed(&first, &["k1"]).await;
    first.h.messenger.set_responder(|_| Reply::Hang);
    first.n.reply("k1", 0, "still there?").unwrap();
    first.h.messenger.wait_requests(1).await;

    // The first run never got an outcome; the next run sends the entry again on its own.
    let second = world_with(Setup { stores, ..Setup::default() });
    second.h.messenger.wait_requests(1).await;
    let vs = payload(&second.h.messenger.requests()[0]);
    assert_eq!(vs.i32("operation"), Some(5));
    assert_eq!(vs.str("inlineReplyMessage"), Some("still there?"));
    drop(first);
}

#[tokio::test(start_paused = true)]
async fn a_successful_banner_reply_posts_no_notice() {
    let w = world();
    deliver(
        &w.h,
        batch(
            &["k1"],
            &[1],
            &[item("k1", NOW_MS, json!({"importance": 4, "title": "t", "notificationActions": actions()}))],
        ),
    )
    .await;

    let posted = w.w.notifier.posted()[0].clone();
    w.w.responses.route(
        &posted.id,
        &posted.user_info,
        NotificationResponse::Reply { action_id: posted.actions[0].id.clone(), text: "ok".into() },
    );

    w.h.messenger.wait_requests(1).await;
    settle().await;
    assert_eq!(w.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_reply_is_stored_under_inline_reply() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_with(Setup { stores: StoreLocation::Directory(dir.path().to_path_buf()), ..Setup::default() });
    seed(&w, &["k1"]).await;

    w.n.reply("k1", 0, "on my way").unwrap();
    w.h.messenger.wait_requests(1).await;
    settle().await;

    let db = dir.path().join("phones").join(format!("{PHONE_ID}.Prod")).join("notifications.sqlite3");
    let conn = rusqlite::Connection::open(db).unwrap();
    let (ext1, ext2): (String, String) = conn
        .query_row("SELECT external_id1, external_id2 FROM phone_requests", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((ext1.as_str(), ext2.as_str()), ("k1", "inline_reply"));
}
