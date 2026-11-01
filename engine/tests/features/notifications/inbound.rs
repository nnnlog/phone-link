//! Inbound notification batches, phone → PC: content messages, the order of a batch, duplicates,
//! malformed items and arrays, batch announcements, "last updated", and the store across a reopen.

use crate::support::app_world;
use crate::support::notifications_world as support;

use std::time::Duration;

use serde_json::json;

use app_world::*;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::notifications::NotificationsEvent;
use support::*;

fn keys_of(w: &World) -> Vec<String> {
    w.n.list().unwrap().into_iter().map(|v| v.key).collect()
}

fn last_updated(w: &World) -> Option<i64> {
    w.n.status().unwrap().last_updated_ms
}

#[tokio::test(start_paused = true)]
async fn a_batch_applies_session_changes_then_upserts_then_removals() {
    let w = world();
    // Seed: two notifications of session S1 and one of S2.
    let a = item("0|S1|a", NOW_MS - 3, json!({}));
    let b = item("0|S1|b", NOW_MS - 2, json!({}));
    let c = item("0|S2|c", NOW_MS - 1, json!({}));
    let ans = deliver(&w.h, batch(&["x", "y", "z"], &[3, 3, 3], &[a, b, c.clone()])).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
    assert_eq!(keys_of(&w), ["0|S2|c", "0|S1|b", "0|S1|a"], "newest postTime first");

    // One batch: a removal listed before the upsert of the same key, an upsert of a key outside the
    // new session, and a session change listed last.
    let d = item("0|S9|d", NOW_MS, json!({}));
    let b2 = item("0|S1|b", NOW_MS + 5, json!({"title": "updated"}));
    let ans = deliver(
        &w.h,
        batch(
            &["0|S1|b", "ignored-for-upserts", "unused", "S1"],
            &[2, 3, 3, 7],
            &[String::new(), d, b2, String::new()],
        ),
    )
    .await;
    assert_eq!(ans.i32("result"), Some(0));

    // Session S1 purged c (not S1) before the upserts; d (outside S1) was upserted after the purge,
    // so it survives; b was upserted and then removed: removed, whatever the array order.
    assert_eq!(keys_of(&w), ["0|S9|d", "0|S1|a"]);
    let answers = w.h.messenger.responses();
    assert_eq!(answers.last().unwrap().header("result").as_deref(), Some("0"));
}

#[tokio::test(start_paused = true)]
async fn upserts_are_keyed_by_the_json_key_and_keep_the_pinned_state() {
    let w = world();
    deliver(&w.h, batch(&["whatever"], &[1], &[item("k1", NOW_MS, json!({"title": "one"}))])).await;
    w.n.set_pinned("k1", true).unwrap();
    deliver(&w.h, batch(&["other"], &[3], &[item("k1", NOW_MS + 1, json!({"title": "two"}))])).await;

    let list = w.n.list().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].title.as_deref(), Some("two"));
    assert!(list[0].pinned, "an update keeps the pinned state");
    assert_eq!(list[0].time_ms, NOW_MS + 1);

    // Unknown operations count as processed and change nothing.
    let ans = deliver(&w.h, batch(&["k1", "k1"], &[0, 6], &[String::new(), String::new()])).await;
    assert_eq!(ans.i32("result"), Some(0));
    assert_eq!(keys_of(&w), ["k1"]);
}

#[tokio::test(start_paused = true)]
async fn duplicate_item_json_rejects_the_message_before_anything_changes() {
    let w = world();
    deliver(&w.h, batch(&["a"], &[3], &[item("a", NOW_MS, json!({}))])).await;
    let mut events = w.n.subscribe();
    let dup = item("b", NOW_MS, json!({}));

    // A session change before the duplicates would purge `a`; the rejection applies nothing.
    let ans = deliver(&w.h, batch(&["zzz", "b", "b"], &[7, 1, 3], &[String::new(), dup.clone(), dup])).await;
    assert_eq!(ans, ValueSet::new().with("result", 1i32).with("message", "duplicate notification item 80004005"));
    assert_eq!(w.h.messenger.responses().last().unwrap().header("result").as_deref(), Some("1"));
    assert_eq!(keys_of(&w), ["a"]);
    assert!(drain(&mut events).iter().all(|e| !matches!(e, NotificationsEvent::ListChanged { .. })));
}

#[tokio::test(start_paused = true)]
async fn duplicates_are_identical_upsert_texts_whether_or_not_they_parse() {
    let w = world();
    // Two empty upsert texts: duplicates, although neither is an item.
    let ans = deliver(&w.h, batch(&["", ""], &[1, 3], &[String::new(), String::new()])).await;
    assert_eq!(ans, ValueSet::new().with("result", 1i32).with("message", "duplicate notification item 80004005"));

    // The texts of other operations are not compared: two empty removal texts are no duplicates.
    let ans = deliver(&w.h, batch(&["a", "b"], &[2, 2], &[String::new(), String::new()])).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
}

#[tokio::test(start_paused = true)]
async fn malformed_items_are_skipped_and_the_rest_applies() {
    let w = world();
    let items = [
        "not json".to_string(),
        r#"{"packageName":"com.a","postTime":1}"#.to_string(),
        r#"{"key":"k2","packageName":"com.a","postTime":1.5}"#.to_string(),
        r#"{"key":"k3","packageName":"com.a","postTime":1,"platform":1,"version":2}"#.to_string(),
        r#"{"key":"k4","packageName":7,"postTime":1}"#.to_string(),
        // Beyond 9999-12-31T23:59:59.999Z (253402300799999 ms).
        r#"{"key":"k5","packageName":"com.a","postTime":253402300800000}"#.to_string(),
        item("good", NOW_MS, json!({"platform": 0, "version": 1})),
        item("ios-without-version", NOW_MS - 1, json!({"platform": 1})),
    ];

    let ans = deliver(&w.h, batch(&[""; 8], &[1, 1, 3, 3, 1, 3, 1, 3], &items)).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
    assert_eq!(keys_of(&w), ["good", "ios-without-version"]);
}

#[tokio::test(start_paused = true)]
async fn missing_mistyped_or_unequal_arrays_get_an_empty_answer_and_change_nothing() {
    let w = world();
    deliver(&w.h, batch(&["a"], &[3], &[item("a", NOW_MS, json!({}))])).await;
    let before = last_updated(&w);
    w.w.clock.advance(Duration::from_secs(60));

    let cases = [
        // Unequal lengths.
        batch_values(&["a", "b"], &[2], &[String::new()]),
        // `operations` as an Int64Array.
        ValueSet::new()
            .with("notificationKeys", vec!["a"])
            .with("operations", Value::Int64Array(vec![2]))
            .with("notifications", vec![""]),
        // `notifications` missing.
        ValueSet::new().with("notificationKeys", vec!["a"]).with("operations", vec![2i32]),
    ];

    for values in cases {
        let ans = deliver(&w.h, phone_content("notifications", values)).await;
        assert_eq!(ans, ValueSet::new(), "an empty answer");
        assert_eq!(w.h.messenger.responses().last().unwrap().header("result"), None, "no result header");
    }

    assert_eq!(keys_of(&w), ["a"]);
    assert_eq!(last_updated(&w), before, "last updated unchanged");
}

#[tokio::test(start_paused = true)]
async fn a_rejected_message_discards_its_announced_transaction() {
    let w = world();
    // An announcement of 3 keeps a transaction open under (phone, cv-1).
    let meta = phone_content(
        "notifications",
        ValueSet::new().with("metadata", true).with("notificationKeys", vec!["a", "b", "c"]),
    );
    let ans = deliver(&w.h, meta).await;
    assert_eq!(ans.i32("itemCount"), Some(3));

    // A malformed content message is rejected and its transaction discarded…
    deliver(&w.h, phone_content("notifications", ValueSet::new().with("notificationKeys", vec!["a"]))).await;
    assert_eq!(last_updated(&w), None);

    // …so the next content message under the same key is complete by itself.
    deliver(&w.h, batch(&["a"], &[3], &[item("a", NOW_MS, json!({}))])).await;
    assert_eq!(last_updated(&w), Some(NOW_MS));
}

#[tokio::test(start_paused = true)]
async fn an_announcement_is_answered_and_completed_by_one_content_message() {
    let w = world();
    let meta = phone_content(
        "notifications",
        ValueSet::new().with("metadata", 1i32).with("syncID", "s-7").with("notificationKeys", vec!["a", "b"]),
    );
    let ans = deliver(&w.h, meta).await;
    assert_eq!(
        ans.to_json(),
        json!({
            "correlationVector": {"String": "cv-1"},
            "contentType": {"String": "notifications"},
            "contractVersion": {"Double": 3.99},
            "itemCount": {"Int32": 2},
            "result": {"Int32": 0}
        })
    );

    // The content message pairs by syncID and carries exactly 2 operations: complete.
    let mut content = batch_values(&["a", "b"], &[3, 3], &[item("a", NOW_MS, json!({})), item("b", NOW_MS, json!({}))]);
    content.insert("syncID", "s-7");
    deliver(&w.h, phone_content("notifications", content)).await;
    assert_eq!(last_updated(&w), Some(NOW_MS));

    // A batch announced as 2 but split over two messages applies both parts and never completes.
    w.w.clock.advance(Duration::from_secs(30));
    let meta = phone_content(
        "notifications",
        ValueSet::new().with("metadata", true).with("syncID", "s-8").with("notificationKeys", vec!["c", "d"]),
    );
    deliver(&w.h, meta).await;
    for k in ["c", "d"] {
        let mut part = batch_values(&[k], &[3], &[item(k, NOW_MS, json!({}))]);
        part.insert("syncID", "s-8");
        assert_eq!(deliver(&w.h, phone_content("notifications", part)).await.i32("result"), Some(0));
    }
    assert_eq!(keys_of(&w).len(), 4);
    assert_eq!(last_updated(&w), Some(NOW_MS), "not refreshed");
}

#[tokio::test(start_paused = true)]
async fn without_a_correlation_id_a_batch_applies_but_never_completes() {
    let w = world();
    let meta = raw_content(
        ValueSet::new()
            .with("contentType", "notifications")
            .with("contractVersion", 3.99f64)
            .with("metadata", true)
            .with("notificationKeys", vec!["a"]),
    );

    let ans = deliver(&w.h, meta).await;
    assert_eq!(ans.get("correlationVector"), Some(&Value::Empty), "a null correlation id");
    assert_eq!(ans.i32("itemCount"), Some(0));

    let mut content = batch_values(&["a"], &[3], &[item("a", NOW_MS, json!({}))]);
    content.insert("contentType", "notifications");
    content.insert("contractVersion", 3.99f64);
    let ans = deliver(&w.h, raw_content(content)).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
    assert_eq!(keys_of(&w), ["a"]);
    assert_eq!(last_updated(&w), None);

    // An empty batch without one completes: a processor made without a correlation id expects 0
    // operations and this message carries 0, so it records "last updated".
    w.w.clock.advance(Duration::from_secs(10));
    let mut empty = batch_values(&[], &[], &[]);
    empty.insert("contentType", "notifications");
    empty.insert("contractVersion", 3.99f64);
    let ans = deliver(&w.h, raw_content(empty)).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
    assert_eq!(last_updated(&w), Some(NOW_MS + 10_000));
}

#[tokio::test(start_paused = true)]
async fn a_text_form_correlation_vector_counts() {
    let w = world();
    let mut content = batch_values(&["a"], &[3], &[item("a", NOW_MS, json!({}))]);
    content.insert("contentType", "notifications");
    content.insert("contractVersion", 3.99f64);
    content.insert("correlationVector", 77i32);

    deliver(&w.h, raw_content(content)).await;
    assert_eq!(last_updated(&w), Some(NOW_MS), "an Int32 correlationVector has a text form");
}

#[tokio::test(start_paused = true)]
async fn the_store_survives_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let stores = StoreLocation::Directory(dir.path().to_path_buf());
    {
        let w = world_with(Setup { stores: stores.clone(), ..Setup::default() });
        deliver(&w.h, batch(&["a", "b"], &[3, 1], &[item("a", NOW_MS - 1, json!({})), item("b", NOW_MS, json!({}))]))
            .await;
        w.n.set_pinned("a", true).unwrap();
        w.n.set_pc_permission(false).unwrap();
    }

    let w = world_with(Setup { stores, ..Setup::default() });
    let list = w.n.list().unwrap();
    assert_eq!(list.iter().map(|v| (v.key.as_str(), v.pinned)).collect::<Vec<_>>(), [("b", false), ("a", true)]);

    let status = w.n.status().unwrap();
    assert!(!status.pc_permission);
    assert_eq!(status.last_updated_ms, Some(NOW_MS));
}

#[tokio::test(start_paused = true)]
async fn the_list_hides_media_style_items() {
    let w = world();
    let media = item("m", NOW_MS, json!({"notificationClass": 2}));
    let chat = item("c", NOW_MS - 1, json!({"notificationClass": 1, "subtitle": "s"}));
    let call = item("p", NOW_MS - 2, json!({"notificationClass": 3, "featureFlags": 2}));
    deliver(&w.h, batch(&["", "", ""], &[3, 3, 3], &[media, chat, call])).await;

    let list = w.n.list().unwrap();
    assert_eq!(list.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["c", "p"]);

    use phonelink_engine::features::notifications::OpenTarget;
    assert_eq!(list[0].open_target, OpenTarget::Messaging);
    assert_eq!(list[0].subtitle.as_deref(), Some("s"));
    assert_eq!(list[1].open_target, OpenTarget::Calls);
    assert!(list[1].launch_supported);
}

#[tokio::test(start_paused = true)]
async fn a_failing_store_transaction_applies_nothing_and_is_answered_failure() {
    let dir = tempfile::tempdir().unwrap();
    let w = world_with(Setup { stores: StoreLocation::Directory(dir.path().to_path_buf()), ..Setup::default() });
    deliver(&w.h, batch(&["a"], &[3], &[item("a", NOW_MS, json!({}))])).await;

    // Another connection drops the queue table: the removal's queue drop fails after the upsert ran.
    let db = dir.path().join("phones").join(format!("{PHONE_ID}.Prod")).join("notifications.sqlite3");
    rusqlite::Connection::open(db).unwrap().execute_batch("DROP TABLE phone_requests").unwrap();

    let ans = deliver(&w.h, batch(&["", "a"], &[1, 2], &[item("b", NOW_MS, json!({})), String::new()])).await;
    assert_eq!(ans, ValueSet::new().with("result", 1i32).with("message", "notification store failure 80004005"));
    assert_eq!(keys_of(&w), ["a"], "the upsert of b was rolled back with the failed removal");
    assert!(w.w.notifier.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn mistyped_stored_keys_make_an_item_malformed() {
    let w = world();
    let items = [
        // JSON nulls in keys read while storing.
        item("title-null", NOW_MS, json!({"title": null})),
        item("text-null", NOW_MS, json!({"text": null})),
        item("template-null", NOW_MS, json!({"template": null})),
        item("big-null", NOW_MS, json!({"template": "x$BigTextStyle", "bigText": null})),
        // publisherWebsite is read as a string for every item.
        item("site-number", NOW_MS, json!({"publisherWebsite": 5})),
        item("site-null", NOW_MS, json!({"publisherWebsite": null})),
        // Not read while storing: bigText under another template, a string publisherWebsite.
        item("big-under-inbox", NOW_MS - 1, json!({"template": "x$InboxStyle", "bigText": null})),
        item("site-string", NOW_MS - 2, json!({"publisherWebsite": "https://example.com"})),
    ];

    let ans = deliver(&w.h, batch(&[""; 8], &[3; 8], &items)).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));
    assert_eq!(keys_of(&w), ["big-under-inbox", "site-string"]);
}

#[tokio::test(start_paused = true)]
async fn keys_read_only_later_read_as_absent_when_mistyped() {
    let w = world();
    // notificationClass "2" is no media-style class: the item is stored and listed as ordinary;
    // importance "5" reads as absent (3): no alert.
    let it = item("m", NOW_MS, json!({"notificationClass": "2", "importance": "5", "appName": "A", "title": "t"}));
    let ans = deliver(&w.h, batch(&[""], &[1], &[it])).await;
    assert_eq!(ans, ValueSet::new().with("result", 0i32));

    let view = w.n.list().unwrap().remove(0);
    assert_eq!(view.key, "m");
    assert_eq!(view.open_target, phonelink_engine::features::notifications::OpenTarget::Notifications);
    assert!(w.w.notifier.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn every_scalar_correlation_vector_has_a_text_form() {
    use phonelink_engine::codec::valueset::{Point, Rect, Size};
    use phonelink_engine::time::filetime_from_unix_ms;

    let w = world();
    let cases = [
        (Value::DateTime(filetime_from_unix_ms(NOW_MS)), "2026-10-20T00:00:00.0000000+00:00"),
        (Value::TimeSpan(36_000_000_000), "01:00:00"),
        (Value::TimeSpan(-(864_000_000_000 + 5)), "-1.00:00:00.0000005"),
        (Value::Point(Point { x: 1.0, y: 2.5 }), "1,2.5"),
        (Value::Size(Size { width: 3.0, height: 4.0 }), "3,4"),
        (Value::Rect(Rect { x: 1.0, y: 2.0, width: 3.0, height: 4.5 }), "1,2,3,4.5"),
        (Value::Guid(uuid::Uuid::nil()), "00000000-0000-0000-0000-000000000000"),
        (Value::Bool(true), "True"),
    ];

    for (n, (cv, text)) in cases.into_iter().enumerate() {
        let mut meta = ValueSet::new().with("contentType", "notifications").with("contractVersion", 3.99f64);
        meta.insert("metadata", true);
        meta.insert("correlationVector", cv.clone());
        let ans = deliver(&w.h, raw_content(meta)).await;
        assert_eq!(ans.str("correlationVector"), Some(text), "{cv:?}");

        // A content message with it is complete by itself.
        w.w.clock.advance(Duration::from_secs(1));
        let mut content = batch_values(&["a"], &[3], &[item("a", NOW_MS, json!({}))]);
        content.insert("contentType", "notifications");
        content.insert("contractVersion", 3.99f64);
        content.insert("correlationVector", cv);
        deliver(&w.h, raw_content(content)).await;
        assert_eq!(last_updated(&w), Some(NOW_MS + 1_000 * (n as i64 + 1)), "case {n}");
    }
}
