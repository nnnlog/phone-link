//! Presentation on macOS: the banner decision, the 20 s duplicate check, per-app flags and mutes,
//! group-message consent, throttling with its notice, the content of a posted notification,
//! withdrawals, and the responses to them.

use crate::assert_matches;
use crate::support::app_world;
use crate::support::common::payload;
use crate::support::notifications_world as support;
use phonelink_engine::config::features::NotificationsConfig;
use phonelink_engine::config::features::PhoneAppsConfig;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use app_world::*;
use phonelink_engine::app::consts::capability;
use phonelink_engine::features::NotificationResponse;
use phonelink_engine::features::notifications::{AppFlags, MuteDuration, NotificationsEvent, OpenTarget};
use phonelink_engine::features::shared::notifier::testing::NotifierCall;
use support::*;

fn reply_actions() -> serde_json::Value {
    json!([
        {"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0},
        {"actionName": "Mark read", "isActionInlineReply": false, "actionIndex": 1}
    ])
}

/// Delivers a new (operation 1) alerting item posted at `post_time`, with `extra` keys.
async fn post_new_at(w: &World, key: &str, post_time: i64, extra: serde_json::Value) {
    let mut v = json!({"importance": 4, "appName": "Chat", "title": format!("t-{key}"), "text": "hi"});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    deliver(&w.h, batch(&[key], &[1], &[item(key, post_time, v)])).await;
}

async fn post_new(w: &World, key: &str, extra: serde_json::Value) {
    post_new_at(w, key, NOW_MS, extra).await;
}

/// With the phone's app list: the list names a known app from it, and a banner carries its icon; an
/// unknown app keeps the item's name and no icon.
#[tokio::test(start_paused = true)]
async fn the_phone_app_list_names_the_app_and_gives_the_banner_its_icon() {
    use phonelink_engine::codec::valueset::ValueSet;
    use phonelink_engine::features::phone_apps::{PhoneApps, PlainAppsHost};

    let w = world();
    let apps = PhoneApps::install(&w.w.env, PhoneAppsConfig::default(), Arc::new(PlainAppsHost)).unwrap();
    w.n.set_app_directory(apps.directory()).unwrap();
    assert!(w.n.set_app_directory(apps.directory()).is_err(), "set once");

    let icon = vec![0x89u8, b'P', b'N', b'G'];
    let push = ValueSet::new()
        .with("itemCount", 1i32)
        .with("phoneAppsIds", vec![1i64])
        .with("appPackageNames", vec!["com.example.chat"])
        .with("appNames", vec!["Chat\nApp"])
        .with("appVersions", vec!["1"])
        .with("phoneAppsLastUpdatedTimes", vec![0i64])
        .with("checksums", vec![1i64])
        .with("iconSizes", vec![icon.len() as i64])
        .with("content", icon.clone());
    assert_eq!(deliver(&w.h, phone_content("phoneApps", push)).await, ValueSet::new().with("result", 0i32));

    post_new(&w, "k1", json!({})).await;
    let posted = w.w.notifier.posted();
    assert_eq!(posted[0].title, "Chat", "banners use the item's appName");
    assert_eq!(posted[0].image, Some(icon));
    assert_eq!(w.n.list().unwrap()[0].app_name.as_deref(), Some("Chat App"));

    let other = item("k2", NOW_MS, json!({"importance": 4, "appName": "Other", "packageName": "com.other"}));
    deliver(&w.h, batch(&["k2"], &[1], &[other])).await;
    let posted = w.w.notifier.posted();
    assert_eq!(posted.last().unwrap().image, None);
    let list = w.n.list().unwrap();
    assert_eq!(list.iter().find(|v| v.key == "k2").unwrap().app_name.as_deref(), Some("Other"));
}

#[tokio::test(start_paused = true)]
async fn a_new_important_item_posts_its_content_with_reply_actions() {
    let w = world();
    post_new(&w, "k1", json!({"title": "Ann", "notificationActions": reply_actions()})).await;

    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    let n = &posted[0];
    assert_eq!(n.id, format!("notifications/{}/k1", phone()));
    assert_eq!((n.title.as_str(), n.subtitle.as_deref(), n.body.as_str()), ("Chat", Some("Ann"), "hi"));
    assert_eq!(n.thread_id.as_deref(), Some("com.example.chat"));
    assert!(n.alert);
    assert_eq!(n.actions.len(), 1, "only the reply; banners offer no buttons");
    assert_eq!(n.actions[0].id, "reply/0/0", "reply/<actionIndex>/<position>");
    let input = n.actions[0].text_input.as_ref().unwrap();
    assert_eq!((input.button_title.as_str(), input.placeholder.as_str()), ("Send", "Reply"));
    assert_eq!(n.user_info.get("key").map(String::as_str), Some("k1"));

    // An existing item (operation 3) never alerts; neither does an unimportant one.
    deliver(&w.h, batch(&["k2"], &[3], &[alerting("k2")])).await;
    post_new(&w, "k3", json!({"importance": 3})).await;
    post_new(&w, "k4", json!({"importance": null})).await;
    assert_eq!(w.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn only_alert_once_updates_alert_again_only_with_multi_line_content() {
    let w = world();
    post_new(&w, "k", json!({"flags": 8, "text": "one"})).await;
    post_new(&w, "k", json!({"flags": 8, "text": "two"})).await;
    assert_eq!(w.w.notifier.posted().len(), 1, "an update of a stored key with only-alert-once");

    post_new(&w, "k", json!({"flags": 8, "text": "three", "template": "x$InboxStyle", "textLines": ["a", "b"]})).await;
    assert_eq!(w.w.notifier.posted().len(), 2);

    // Without the flag an update alerts again.
    post_new(&w, "k", json!({"text": "four"})).await;
    assert_eq!(w.w.notifier.posted().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_post_delivered_twice_within_20_s_alerts_once() {
    let w = world();
    let texts = [alerting("k")];
    deliver(&w.h, batch(&["k"], &[1], &texts)).await;

    w.w.clock.advance(Duration::from_secs(19));
    deliver(&w.h, batch(&["k"], &[1], &texts)).await;
    assert_eq!(w.w.notifier.posted().len(), 1, "both hashes seen within 20 s");

    w.w.clock.advance(Duration::from_secs(20));
    deliver(&w.h, batch(&["k"], &[1], &texts)).await;
    assert_eq!(w.w.notifier.posted().len(), 2, "remembered for 20 s only");
}

#[tokio::test(start_paused = true)]
async fn the_duplicate_check_remembers_a_post_that_did_not_alert() {
    let w = world();
    // Not important enough to alert; its two hashes are remembered all the same.
    post_new(&w, "k", json!({"importance": 3, "title": "Ann"})).await;
    assert!(w.w.notifier.posted().is_empty());

    // The same post, now important, within 20 s: a duplicate.
    post_new(&w, "k", json!({"importance": 4, "title": "Ann"})).await;
    assert!(w.w.notifier.posted().is_empty(), "both hashes were remembered by the post that did not alert");

    // 20 s after it was last seen it alerts.
    w.w.clock.advance(Duration::from_secs(20));
    post_new(&w, "k", json!({"importance": 4, "title": "Ann"})).await;
    assert_eq!(w.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn permissions_mutes_key_window_class_and_recency_suppress_alerts() {
    let w = world();
    // The key window.
    w.h.app.set_foreground(true);
    post_new(&w, "a", json!({})).await;
    w.h.app.set_foreground(false);

    // Messaging class; media-style class.
    post_new(&w, "b", json!({"notificationClass": 1})).await;
    post_new(&w, "c", json!({"notificationClass": 2})).await;

    // Too old; a recent `timestamp` makes an old post recent.
    let old = NOW_MS - 5 * 60 * 1000 - 1;
    deliver(&w.h, batch(&["d"], &[1], &[item("d", old, json!({"importance": 5, "title": "d"}))])).await;
    assert!(w.w.notifier.posted().is_empty());
    deliver(&w.h, batch(&["e"], &[1], &[item("e", old, json!({"importance": 5, "title": "e", "timestamp": NOW_MS}))]))
        .await;
    assert_eq!(w.w.notifier.posted().len(), 1);

    // Mute-all, then a per-app mute.
    w.n.set_mute_all(true).unwrap();
    post_new(&w, "f", json!({})).await;
    w.n.set_mute_all(false).unwrap();
    w.n.mute_app("com.example.chat", MuteDuration::OneHour).unwrap();
    post_new(&w, "g", json!({})).await;
    assert_eq!(w.w.notifier.posted().len(), 1);
    w.w.clock.advance(Duration::from_secs(3601));
    post_new_at(&w, "h", NOW_MS + 3_601_000, json!({"text": "after the mute"})).await;

    // The PC permission off.
    w.n.set_pc_permission(false).unwrap();
    post_new(&w, "i", json!({})).await;
    let keys: Vec<_> = w.w.notifier.posted().into_iter().map(|n| n.user_info["key"].clone()).collect();
    assert_eq!(keys, ["e", "h"]);
}

#[tokio::test(start_paused = true)]
async fn without_the_device_permission_nothing_alerts() {
    let w = world_with(Setup { device_permission: false, ..Setup::default() });
    post_new(&w, "a", json!({})).await;
    assert!(w.w.notifier.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn app_flags_alert_silently_or_not_at_all_with_the_per_app_list() {
    let w = world();
    // Feed on, banner off: delivered silently.
    w.n.set_app_local_setting("com.example.chat", AppFlags::FEED).unwrap();
    post_new(&w, "a", json!({})).await;
    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    assert!(!posted[0].alert);

    // Both off (the phone's sync row says off, the local row has no banner): no notification.
    w.h.messenger.answer_all(answer(
        0,
        phonelink_engine::codec::valueset::ValueSet::new().with(
            "savedSetting",
            r#"{"settingKey":"com.example.chat","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting"}"#,
        ),
    ));
    w.n.toggle_app_from_list("com.example.chat", false).await.unwrap();
    post_new(&w, "b", json!({})).await;
    assert_eq!(w.w.notifier.posted().len(), 1);

    // Without per-app list support (no PhoneApps bit) the flags and mutes are ignored.
    w.h.app.update_phone_record(|r| r.capability_bits = capability::NOTIFICATIONS_LISTENER);
    post_new(&w, "c", json!({})).await;
    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 2);
    assert!(posted[1].alert);
}

#[tokio::test(start_paused = true)]
async fn group_messages_need_the_messages_consent_and_ignore_the_banner_flag() {
    let consent = Arc::new(Consent(Mutex::new(false)));
    let w = world_with(Setup { messaging: Some(consent.clone()), ..Setup::default() });
    post_new(&w, "a", json!({"isReadOnlyGroupMessage": true})).await;
    assert!(w.w.notifier.posted().is_empty());

    *consent.0.lock().unwrap() = true;
    w.n.set_app_local_setting("com.example.chat", AppFlags::FEED).unwrap();
    post_new(&w, "b", json!({"isReadOnlyGroupMessage": true})).await;
    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    assert!(posted[0].alert, "a group message is not silenced by the banner flag");

    // Without a messaging feature, never.
    let w2 = world();
    post_new(&w2, "c", json!({"isReadOnlyGroupMessage": true})).await;
    assert!(w2.w.notifier.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn throttling_pauses_a_package_with_one_notice_and_can_be_turned_off() {
    let w = world();
    for n in 0..5 {
        post_new(&w, &format!("k{n}"), json!({})).await;
    }

    let calls = w.w.notifier.posted();
    assert_eq!(calls.len(), 6, "5 alerts and the notice");
    assert!(calls[..4].iter().all(|n| n.alert));
    assert!(!calls[4].alert, "the 5th within 10 s is delivered silently");
    let notice = &calls[5];
    assert_eq!(notice.id, format!("notifications/{}:paused/com.example.chat", phone()));
    assert!(notice.alert);
    assert!(notice.body.contains("3 minutes"));
    assert_eq!(notice.actions[0].id, "disableThrottling");

    // During the cooldown: silent, no second notice.
    post_new(&w, "k5", json!({})).await;
    let calls = w.w.notifier.posted();
    assert_eq!(calls.len(), 7);
    assert!(!calls[6].alert);

    // The notice's button turns throttling off for the phone.
    w.w.responses.route(
        &notice.id,
        &notice.user_info,
        NotificationResponse::Action { action_id: "disableThrottling".into() },
    );
    assert!(!w.n.status().unwrap().throttling_enabled);
    post_new(&w, "k6", json!({})).await;
    assert!(w.w.notifier.posted().last().unwrap().alert);
}

#[tokio::test(start_paused = true)]
async fn throttling_follows_the_config() {
    let config = NotificationsConfig { toast_throttling: false, ..NotificationsConfig::default() };
    let w = world_with(Setup { config, ..Setup::default() });
    for n in 0..6 {
        post_new(&w, &format!("k{n}"), json!({})).await;
    }
    assert!(w.w.notifier.posted().iter().all(|n| n.alert));
}

#[tokio::test(start_paused = true)]
async fn throttling_counts_group_messages_without_silencing_them() {
    let consent = Arc::new(Consent(Mutex::new(true)));
    let w = world_with(Setup { messaging: Some(consent), ..Setup::default() });
    for n in 0..6 {
        post_new(&w, &format!("g{n}"), json!({"isReadOnlyGroupMessage": true})).await;
    }

    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 7, "6 alerts and the notice of the 5th");
    let notice = &posted[5];
    assert_eq!(notice.id, format!("notifications/{}:paused/com.example.chat", phone()));
    assert!(!notice.alert, "the notice of a group message does not pop up");
    assert!(posted.iter().filter(|n| n.id != notice.id).all(|n| n.alert), "a group message is never made silent");

    // The group messages started the package's cooldown: an ordinary item is silent now.
    post_new(&w, "plain", json!({})).await;
    assert!(!w.w.notifier.posted().last().unwrap().alert);
}

#[tokio::test(start_paused = true)]
async fn a_package_less_item_is_never_throttled_and_a_nameless_app_gets_no_notice() {
    let w = world();
    for n in 0..6 {
        let key = format!("e{n}");
        let text = json!({"key": key, "packageName": "", "postTime": NOW_MS, "importance": 4, "appName": "A",
            "title": format!("t{n}"), "text": "x"})
        .to_string();
        deliver(&w.h, batch(&[&key], &[1], &[text])).await;
    }

    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 6);
    assert!(posted.iter().all(|n| n.alert), "an empty package is never throttled");

    // An app without a name is throttled like any other, but no notice is posted for it.
    w.w.notifier.take();
    for n in 0..5 {
        let key = format!("u{n}");
        let text = item(&key, NOW_MS, json!({"importance": 4, "title": format!("u{n}"), "text": "x"}));
        deliver(&w.h, batch(&[&key], &[1], &[text])).await;
    }

    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 5, "no paused notice");
    assert!(!posted[4].alert, "the 5th is silent all the same");
}

#[tokio::test(start_paused = true)]
async fn an_unauthorized_notifier_gets_nothing() {
    let w = world();
    w.w.notifier.set_authorized(false);
    post_new(&w, "a", json!({})).await;
    assert!(w.w.notifier.calls().iter().all(|c| !matches!(c, NotifierCall::Post(_))));
}

#[tokio::test(start_paused = true)]
async fn removals_and_session_changes_withdraw_and_a_removed_post_never_alerts() {
    let w = world();
    post_new(&w, "0|S1|a", json!({})).await;
    post_new(&w, "0|S2|b", json!({})).await;
    w.w.notifier.take();

    deliver(&w.h, batch(&["S2"], &[7], &[String::new()])).await;
    assert_eq!(w.w.notifier.withdrawn(), [format!("notifications/{}/0|S1|a", phone())]);

    deliver(&w.h, batch(&["0|S2|b"], &[2], &[String::new()])).await;
    assert_eq!(w.w.notifier.withdrawn().len(), 2);

    // Posted and removed in one batch: no alert.
    w.w.notifier.take();
    deliver(&w.h, batch(&["", "c"], &[1, 2], &[alerting("c"), String::new()])).await;
    assert!(w.w.notifier.posted().is_empty());

    // Removing a key the PC does not have withdraws nothing.
    w.w.notifier.take();
    deliver(&w.h, batch(&["nope"], &[2], &[String::new()])).await;
    assert!(w.w.notifier.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_banner_reply_is_queued_and_its_failure_is_notified() {
    let w = world();
    post_new(&w, "k1", json!({"notificationActions": reply_actions()})).await;
    let posted = w.w.notifier.posted()[0].clone();
    w.h.messenger.answer_all(answer(1, Default::default()));

    w.w.responses.route(
        &posted.id,
        &posted.user_info,
        NotificationResponse::Reply { action_id: posted.actions[0].id.clone(), text: "on my way".into() },
    );
    w.h.messenger.wait_requests(1).await;

    let req = &notification_requests(&w.h)[0];
    let vs = payload(req);
    assert_eq!(vs.i32("operation"), Some(5));
    assert_eq!(vs.str("key"), Some("k1"));
    assert_eq!(vs.i32("actionIndex"), Some(0));
    assert_eq!(vs.str("inlineReplyMessage"), Some("on my way"));
    assert_eq!(vs.bool("isuserinitiated"), Some(false));
    assert_eq!(req.priority, 100);
    assert_eq!(trace_names(req), ("NotificationsToastInlineReply".to_string(), "Notifications".to_string()));

    settle().await;
    let notice = w.w.notifier.posted().last().unwrap().clone();
    assert_eq!(notice.id, format!("notifications/{}:reply-failed/k1", phone()));
}

#[tokio::test(start_paused = true)]
async fn clicks_open_a_local_view_and_dismissals_send_nothing() {
    let w = world();
    post_new(&w, "k1", json!({})).await;
    deliver(&w.h, batch(&["c"], &[3], &[item("c", NOW_MS, json!({"notificationClass": 3}))])).await;
    let mut events = w.n.subscribe();

    let posted = w.w.notifier.posted()[0].clone();
    w.w.responses.route(&posted.id, &posted.user_info, NotificationResponse::Activated);
    w.w.responses.route(&posted.id, &posted.user_info, NotificationResponse::Dismissed);

    let mut call_info: HashMap<String, String> = posted.user_info.clone();
    call_info.insert("key".into(), "c".into());
    w.w.responses.route("notifications/x/c", &call_info, NotificationResponse::Activated);

    let opened: Vec<_> = drain(&mut events)
        .into_iter()
        .filter_map(|e| match e {
            NotificationsEvent::OpenRequested { key, target, .. } => Some((key, target)),
            _ => None,
        })
        .collect();
    assert_eq!(opened, [("k1".to_string(), OpenTarget::Notifications), ("c".to_string(), OpenTarget::Calls)]);

    settle().await;
    assert!(w.h.messenger.requests().is_empty(), "nothing is sent to the phone");
}

#[tokio::test(start_paused = true)]
async fn a_key_purged_and_posted_again_by_one_batch_is_new() {
    let w = world();
    post_new(&w, "0|S1|k", json!({"flags": 8, "text": "one"})).await;
    assert_eq!(w.w.notifier.posted().len(), 1);

    // A session change that excludes the key, then the key again: it is new, so only-alert-once does
    // not hold it back; the purge withdraws before the new post.
    let again = item("0|S1|k", NOW_MS, json!({"importance": 4, "flags": 8, "title": "t2", "text": "two"}));
    deliver(&w.h, batch(&["S2", ""], &[7, 1], &[String::new(), again])).await;

    let calls = w.w.notifier.calls();
    let id = format!("notifications/{}/0|S1|k", phone());
    assert_eq!(calls[calls.len() - 2], NotifierCall::Withdraw(vec![id.clone()]));
    assert!(matches!(&calls[calls.len() - 1], NotifierCall::Post(n) if n.id == id && n.body == "two"));
}

#[tokio::test(start_paused = true)]
async fn each_reply_entry_gets_its_own_action() {
    let w = world();
    let entries = json!([
        {"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0},
        {"actionName": "Mark read", "isActionInlineReply": false, "actionIndex": 1},
        {"actionName": "Answer", "isActionInlineReply": true, "actionIndex": 0},
        {"actionName": "Late", "isActionInlineReply": true, "actionIndex": 3}
    ]);

    post_new(&w, "k", json!({"notificationActions": entries})).await;

    let posted = w.w.notifier.posted()[0].clone();
    let titles: Vec<&str> = posted.actions.iter().map(|a| a.title.as_str()).collect();
    assert_eq!(titles, ["Reply", "Answer"], "one action per reply entry with actionIndex < 3");
    assert_ne!(posted.actions[0].id, posted.actions[1].id);

    for a in &posted.actions {
        assert_eq!(
            a.text_input.as_ref().unwrap().placeholder,
            "Reply",
            "several reply actions: the generic placeholder"
        );
    }

    // A reply through the second action carries that entry's actionIndex.
    w.w.responses.route(
        &posted.id,
        &posted.user_info,
        NotificationResponse::Reply { action_id: posted.actions[1].id.clone(), text: "yes".into() },
    );
    w.h.messenger.wait_requests(1).await;
    let vs = payload(&notification_requests(&w.h)[0]);
    assert_eq!((vs.i32("actionIndex"), vs.str("inlineReplyMessage")), (Some(0), Some("yes")));
}

#[tokio::test(start_paused = true)]
async fn an_action_entry_with_a_negative_index_is_never_offered() {
    let w = world();
    let entries = json!([
        {"actionName": "Open", "isActionInlineReply": false, "actionIndex": -1},
        {"actionName": "Reply", "isActionInlineReply": true, "actionIndex": -2}
    ]);

    post_new(&w, "k", json!({"notificationActions": entries})).await;

    assert!(w.w.notifier.posted()[0].actions.is_empty());
    let view = w.n.list().unwrap().remove(0);
    assert!(view.actions.is_empty() && view.reply.is_none());
    assert_matches!(
        w.n.press_action("k", -1),
        Err(phonelink_engine::features::notifications::NotificationsError::UnknownAction)
    );
}

#[tokio::test(start_paused = true)]
async fn a_post_removed_by_its_own_batch_is_neither_deduplicated_nor_throttled() {
    let w = world();
    // Five posts each removed by the batch that posts them: no alert, nothing remembered or counted.
    for n in 0..5 {
        let key = format!("r{n}");
        deliver(&w.h, batch(&["", &key], &[1, 2], &[alerting(&key), String::new()])).await;
    }
    assert!(w.w.notifier.posted().is_empty());

    // The same post again, and a sixth of the package: both alert with a popup.
    deliver(&w.h, batch(&[""], &[1], &[alerting("r0")])).await;
    post_new(&w, "fresh", json!({})).await;

    let posted = w.w.notifier.posted();
    assert_eq!(posted.len(), 2, "r0 was not remembered as a duplicate");
    assert!(posted.iter().all(|n| n.alert), "the removed posts were not counted by throttling");
}
