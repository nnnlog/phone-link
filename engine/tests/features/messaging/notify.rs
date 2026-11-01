//! New-message notifications and their inline reply (composition rule 3; posted through
//! `UserNotifications`), and the `MessagingAlerts` the notifications feature reads.

use crate::support::app_world;
use crate::support::messaging_world as support;

use std::collections::HashMap;
use std::time::Duration;

use app_world::*;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::features::MessagingAlerts;
use phonelink_engine::features::messaging::{MessageKind, MessagingEvent, PcSettings, ProximityState, SendStatus};
use phonelink_engine::features::shared::notifier::NotificationResponse;
use support::*;

/// An item with subscription 1 (KR), conversation 5 and `sms`.
fn sms_item(sms: Vec<Sms>) -> ValueSet {
    ValueSet::new()
        .with("subscriptions", subscriptions_object(&[Sub::new(1, "kr")]))
        .with("smsMessages", sms_object(&sms))
        .with_all(conversations(&[(5, &["+821012345678"])]))
}

#[tokio::test(start_paused = true)]
async fn a_new_inbox_message_is_notified_with_the_contact_name_and_a_reply() {
    let wd = world();
    wd.contacts.names.lock().unwrap().insert("+821012345678".into(), "Alice".into());
    push(&wd.h, item_msg(sms_item(vec![Sms::inbox(1, 5, "+821012345678", "hello")]))).await;

    let posted = wd.w.notifier.posted();
    assert_eq!(posted.len(), 1);
    let n = &posted[0];
    assert_eq!(n.id, format!("messaging/{}/sms/1", phone()));
    assert_eq!(n.thread_id, Some(format!("messaging/{}/5", phone())));
    assert_eq!(n.title, "Alice");
    assert_eq!(n.body, "hello");
    assert!(n.alert);
    assert_eq!(n.actions.len(), 1);
    assert!(n.actions[0].text_input.is_some());
    assert_eq!(n.user_info.get("thread").map(String::as_str), Some("5"));

    // The lookup used the subscription's country.
    assert!(wd.contacts.lookups.lock().unwrap().contains(&("+821012345678".into(), Some("kr".into()))));
}

#[tokio::test(start_paused = true)]
async fn without_a_contact_the_title_is_the_address() {
    let wd = world();
    push(&wd.h, item_msg(sms_item(vec![Sms::inbox(1, 5, "+821012345678", "hello")]))).await;
    assert_eq!(wd.w.notifier.posted()[0].title, "+821012345678");
}

#[tokio::test(start_paused = true)]
async fn rows_that_are_not_notified() {
    let wd = world();
    let old = Sms { date: NOW_MS - 5 * 60 * 1000 - 1, ..Sms::inbox(1, 5, "+1", "old") };
    let read = Sms { read: true, ..Sms::inbox(2, 5, "+1", "read") };
    let sent = Sms::sent(3, 5, "+1", "sent");
    let other_thread = Sms::inbox(4, 99, "+1", "no conversation");
    let just_in_time = Sms { date: NOW_MS - 5 * 60 * 1000, ..Sms::inbox(5, 5, "+1", "recent") };

    push(&wd.h, item_msg(sms_item(vec![old, read, sent, other_thread, just_in_time]))).await;

    let ids: Vec<String> = wd.w.notifier.posted().into_iter().map(|n| n.id).collect();
    assert_eq!(ids, vec![format!("messaging/{}/sms/5", phone())]);
}

/// Delivers inbox SMS `n` of conversation 5 in its own exchange.
async fn deliver(wd: &World, n: i64) {
    let item = sms_item(vec![Sms::inbox(n, 5, "+1", "x")]).with("syncID", format!("b{n}"));
    push(&wd.h, item_msg(item)).await;
}

#[tokio::test(start_paused = true)]
async fn batch_conditions_focus_settings_mute_and_proximity() {
    let wd = world();
    let mut n = 0;
    wd.h.app.set_foreground(true);
    n += 1;
    deliver(&wd, n).await;
    assert!(wd.w.notifier.posted().is_empty(), "the window is focused");
    wd.h.app.set_foreground(false);

    wd.m.set_pc_settings(PcSettings { message_notifications: false, ..PcSettings::default() }).unwrap();
    n += 1;
    deliver(&wd, n).await;
    wd.m.set_pc_settings(PcSettings { pc_consent: false, ..PcSettings::default() }).unwrap();
    n += 1;
    deliver(&wd, n).await;
    assert!(wd.w.notifier.posted().is_empty(), "settings off");
    wd.m.set_pc_settings(PcSettings::default()).unwrap();

    wd.m.set_conversation_flags(5, true, false).unwrap();
    n += 1;
    deliver(&wd, n).await;
    wd.m.set_conversation_flags(5, false, true).unwrap();
    n += 1;
    deliver(&wd, n).await;
    assert!(wd.w.notifier.posted().is_empty(), "muted, hidden");
    wd.m.set_conversation_flags(5, false, false).unwrap();

    // The proximity gate: the phone requires a Bluetooth Classic connection (0x80) and none is active.
    push(
        &wd.h,
        item_msg(
            ValueSet::new().with("syncID", "caps").with("settings", ValueSet::new().with("capabilities", 0x80i32)),
        ),
    )
    .await;

    wd.proximity.set(ProximityState::NotConnected);
    n += 1;
    deliver(&wd, n).await;
    assert!(wd.w.notifier.posted().is_empty(), "proximity gate");

    wd.proximity.set(ProximityState::Connected);
    n += 1;
    deliver(&wd, n).await;
    assert_eq!(wd.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_mms_needs_a_part_and_a_sender() {
    let wd = world();
    let mms = |id: i64, from: &str| Mms {
        id,
        thread: 5,
        date: NOW_MS,
        from: from.into(),
        to: String::new(),
        mailbox: 1,
        read: false,
        sub: 1,
    };

    let parts = [
        Part { id: 1, message: 20, content_type: "text/plain".into(), text: "look".into(), bytes: None },
        Part { id: 2, message: 20, content_type: "image/png".into(), text: String::new(), bytes: Some(vec![7, 7]) },
        Part { id: 3, message: 22, content_type: "text/plain".into(), text: "x".into(), bytes: None },
    ];

    let item = sms_item(Vec::new())
        .with("mmsParts", parts_object(&parts))
        .with("mmsMessages", mms_object(&[mms(20, "+1"), mms(21, "+1"), mms(22, "")], true));

    push(&wd.h, item_msg(item)).await;

    let posted = wd.w.notifier.posted();
    assert_eq!(posted.len(), 1, "21 has no part, 22 no sender");
    assert_eq!(posted[0].id, format!("messaging/{}/mms/20", phone()));
    assert_eq!(posted[0].body, "look");
    assert_eq!(posted[0].image, Some(vec![7, 7]));
}

#[tokio::test(start_paused = true)]
async fn rcs_notifications_offer_a_reply_only_with_rcs_send() {
    let wd = world();
    let item = ValueSet::new()
        .with("syncID", "r")
        .with("settings", ValueSet::new())
        .with_all(rcs_conversations(&[(70, 5, 1, &["+1"]), (71, 6, 3, &["+2", "+3"])]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+1", "rcs hi"), (2, 71, "+2", "group")]));

    push(&wd.h, item_msg(item)).await;

    let posted = wd.w.notifier.posted();
    assert_eq!(posted.len(), 2);
    assert!(posted.iter().all(|n| n.actions.is_empty()), "no RCS send support");
    assert_eq!(posted[0].thread_id, Some(format!("messaging/{}/5", phone())), "grouped by the shown thread");

    let item = ValueSet::new()
        .with("syncID", "r2")
        .with("settings", ValueSet::new().with("rcsSendAvailable", true))
        .with("rcsChats", rcs_chats_object(&[(3, 70, "+1", "again"), (4, 71, "+2", "inactive group")]));
    push(&wd.h, item_msg(item)).await;

    let posted = wd.w.notifier.posted();
    assert_eq!(posted[2].actions.len(), 1);
    assert!(posted[3].actions.is_empty(), "an inactive group cannot be replied to");
}

#[tokio::test(start_paused = true)]
async fn the_inline_reply_sends_to_the_notified_thread() {
    let wd = world();
    push(&wd.h, item_msg(sms_item(vec![Sms::inbox(1, 5, "+821012345678", "hello")]))).await;
    let n = wd.w.notifier.posted().remove(0);

    wd.w.responses.route(
        &n.id,
        &n.user_info,
        NotificationResponse::Reply { action_id: "reply".into(), text: "sure".into() },
    );

    wd.h.messenger.wait_requests(1).await;
    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.i32("requestType"), Some(2));
    assert_eq!(vs.i64("threadId"), Some(5));
    assert_eq!(vs.i32("subscriptionId"), Some(-1));
    assert_eq!(vs.str("body"), Some("sure"));
    assert!(vs.get("recipients").is_none());
    assert_eq!(vs.i64("id"), Some(1), "a pending send was stored");

    // A failed reply raises the "failed to send" notification while message notifications are on.
    wd.h.messenger.answer_all(result_response(1));
    wd.w.responses.route(
        &n.id,
        &n.user_info,
        NotificationResponse::Reply { action_id: "reply".into(), text: "again".into() },
    );
    wd.h.messenger.wait_requests(2).await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    assert!(wd.w.notifier.posted().iter().any(|p| p.id == format!("messaging/{}/failed/2", phone())));
}

#[tokio::test(start_paused = true)]
async fn activating_a_notification_opens_its_conversation() {
    let mut wd = world();
    push(&wd.h, item_msg(sms_item(vec![Sms::inbox(1, 5, "+1", "x")]))).await;
    let n = wd.w.notifier.posted().remove(0);

    drain(&mut wd.events);
    wd.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    assert!(drain(&mut wd.events).contains(&MessagingEvent::OpenConversation { phone: phone(), thread_id: 5 }));

    // A response for another phone is dropped.
    let other = HashMap::from([("phone".to_string(), "x.Prod".to_string()), ("thread".to_string(), "5".to_string())]);
    wd.w.responses.route(&n.id, &other, NotificationResponse::Activated);
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(drain(&mut wd.events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn group_message_alerts_follow_consent_and_notifications() {
    let wd = world();
    let alerts: std::sync::Arc<dyn MessagingAlerts> = wd.m.alerts();
    assert!(alerts.group_message_alerts_allowed(&phone()));

    wd.m.set_pc_settings(PcSettings { message_notifications: false, ..PcSettings::default() }).unwrap();
    assert!(!alerts.group_message_alerts_allowed(&phone()));

    wd.m.set_pc_settings(PcSettings { pc_consent: false, ..PcSettings::default() }).unwrap();
    assert!(!alerts.group_message_alerts_allowed(&phone()));
}

#[tokio::test(start_paused = true)]
async fn smil_parts_do_not_count_as_parts() {
    let wd = world();
    let mms =
        Mms { id: 20, thread: 5, date: NOW_MS, from: "+1".into(), to: String::new(), mailbox: 1, read: false, sub: 1 };
    let parts =
        [Part { id: 1, message: 20, content_type: "application/smil".into(), text: String::new(), bytes: None }];
    let item =
        sms_item(Vec::new()).with("mmsParts", parts_object(&parts)).with("mmsMessages", mms_object(&[mms], true));

    push(&wd.h, item_msg(item)).await;
    assert!(wd.w.notifier.posted().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_message_whose_conversation_arrives_in_the_same_item_is_notified() {
    let wd = world();
    let item = ValueSet::new()
        .with("syncID", "n")
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "hi")]))
        .with_all(conversations(&[(5, &["+1"])]));

    push(&wd.h, item_msg(item)).await;
    assert_eq!(wd.w.notifier.posted().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_notification_reply_in_a_google_rcs_conversation_carries_the_participants() {
    // Rule 3, notification inline reply: in a Google-RCS conversation on a phone that reports
    // `Participants` the reply lists the participants' normalised addresses as `recipients`.
    let wd = world();
    let caps = ValueSet::new().with("syncID", "c").with("settings", ValueSet::new().with("capabilities", 0x20i32));
    push(&wd.h, item_msg(caps)).await;

    let item = ValueSet::new()
        .with("syncID", "i")
        .with("subscriptions", subscriptions_object(&[Sub::new(1, "us")]))
        .with("smsMessages", sms_object(&[Sms::inbox(1, 7, "(650) 555-0101", "hello")]))
        .with_all(conversations(&[(7, &["x@rcs.google.com"])]));
    push(&wd.h, item_msg(item)).await;
    let n = wd.w.notifier.posted().remove(0);
    wd.w.responses.route(
        &n.id,
        &n.user_info,
        NotificationResponse::Reply { action_id: "reply".into(), text: "sure".into() },
    );

    // The settings turned RCS receive on (sync trigger (b)); then the reply.
    wd.h.messenger.wait_requests(2).await;
    let reply = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    assert_eq!(reply.i64("threadId"), Some(7));
    assert_eq!(reply.i32("subscriptionId"), Some(-1));
    assert_eq!(reply.string_array("recipients"), Some(["+16505550101".to_string()].as_slice()));
}

/// Lets spawned replies run until `n` send requests went out and the last one's outcome is stored.
async fn wait_sends(wd: &World, n: usize) {
    for _ in 0..1000 {
        let sent = chat_requests(&wd.h).iter().filter(|r| r.i32("requestType") == Some(2)).count();
        let settled = wd.m.pending_sends().unwrap().iter().all(|s| s.status != SendStatus::Sending);

        if sent >= n && settled {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    panic!("{n} sends never went out");
}

/// Conversation 5 (an SMS thread) with the RCS conversation 70 merged into it, and an unread RCS
/// chat 2 in 70; RCS send is supported, so the notification offers the reply.
async fn rcs_in_sms_thread(wd: &World) {
    let sms = ValueSet::new()
        .with("syncID", "a")
        .with("settings", ValueSet::new().with("rcsSendAvailable", true))
        .with("smsMessages", sms_object(&[Sms { read: true, ..Sms::inbox(1, 5, "+1", "sms") }]))
        .with_all(conversations(&[(5, &["+1"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(sms)).await), Some(0));

    let rcs = ValueSet::new()
        .with("syncID", "b")
        .with_all(rcs_conversations(&[(70, 5, 1, &["+1"])]))
        .with("rcsChats", rcs_chats_object(&[(2, 70, "+1", "rcs hi")]));
    assert_eq!(result_of(&push(&wd.h, item_msg(rcs)).await), Some(0));
}

#[tokio::test(start_paused = true)]
async fn an_rcs_notification_opens_the_listed_conversation_and_its_reply_shows_there() {
    // The RCS notification opens the conversation the list shows — the RCS conversation's fallback
    // id — while its reply's `threadId` is the message's own thread, the RCS conversation's id. The
    // reply shows (and is retried) in the listed conversation.
    let mut wd = world();
    rcs_in_sms_thread(&wd).await;
    let n = wd.w.notifier.posted().pop().unwrap();
    assert_eq!(n.id, format!("messaging/{}/rcschat/2", phone()));
    assert_eq!(n.thread_id, Some(format!("messaging/{}/5", phone())));
    let info = |k: &str| n.user_info.get(k).map(String::as_str);
    assert_eq!((info("thread"), info("replyThread"), info("replyRcs")), (Some("5"), Some("70"), Some("1")));

    drain(&mut wd.events);
    wd.w.responses.route(&n.id, &n.user_info, NotificationResponse::Activated);
    assert!(drain(&mut wd.events).contains(&MessagingEvent::OpenConversation { phone: phone(), thread_id: 5 }));

    // The reply goes to the RCS conversation, as on Windows, and shows in conversation 5.
    let reply = |text: &str| NotificationResponse::Reply { action_id: "reply".into(), text: text.into() };
    wd.w.responses.route(&n.id, &n.user_info, reply("sure"));
    wait_sends(&wd, 1).await;
    let sent = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    assert_eq!((sent.i64("threadId"), sent.i32("subscriptionId")), (Some(70), Some(-1)));
    assert!(sent.get("recipients").is_none());

    let pending = |wd: &World, thread: i64| -> Vec<(i64, Option<SendStatus>)> {
        wd.m.messages(thread)
            .unwrap()
            .into_iter()
            .filter(|m| m.kind == MessageKind::PendingSend)
            .map(|m| (m.id, m.send_status))
            .collect()
    };

    assert_eq!(pending(&wd, 5), [(1, Some(SendStatus::Sent))]);
    assert!(pending(&wd, 70).is_empty(), "no conversation is shown under the RCS conversation's id");
    assert!(wd.m.new_conversation_messages().unwrap().is_empty());

    // A failed reply's notification belongs to conversation 5 as well, and offers no reply.
    wd.h.messenger.answer_all(result_response(1));
    wd.w.responses.route(&n.id, &n.user_info, reply("again"));
    wait_sends(&wd, 2).await;

    let failed = wd.w.notifier.posted().pop().unwrap();
    assert_eq!(failed.id, format!("messaging/{}/failed/2", phone()));
    assert_eq!(failed.thread_id, Some(format!("messaging/{}/5", phone())));
    assert_eq!(failed.user_info.get("thread").map(String::as_str), Some("5"));
    assert!(!failed.user_info.contains_key("replyThread") && failed.actions.is_empty());
    assert_eq!(pending(&wd, 5), [(1, Some(SendStatus::Sent)), (2, Some(SendStatus::Failed(11)))]);

    // Its retry is the conversation view's: the same ids to the same thread; the outcome updates the
    // permission and raises no "failed to send" notification.
    let first = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    wd.h.messenger.answer_all(result_response(5));
    assert_eq!(wd.m.retry_send(2).await.unwrap().status, SendStatus::Failed(11));

    let retry = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    assert_eq!((retry.i64("id"), retry.str("tempId")), (first.i64("id"), first.str("tempId")));
    assert_eq!(retry.i64("threadId"), Some(70));
    assert!(!wd.m.phone_permission().unwrap(), "result 5 denies the messages permission");
    let failures = wd.w.notifier.posted().into_iter().filter(|p| p.id.contains("/failed/")).count();
    assert_eq!(failures, 1, "no notification for the retry");
}
