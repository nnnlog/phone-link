//! Outbound messaging requests: the keys common to every `/chat/v1` request, the sync, media
//! download and send requests, the composition rules, the send response, marking a conversation as
//! read, and the messages permission request.

use crate::assert_matches;
use crate::support::app_world;
use crate::support::common::{eventually, payload};
use crate::support::feature_world;
use crate::support::messaging_world as support;
use phonelink_engine::config::features::MessagingConfig;

use std::time::Duration;

use app_world::*;
use phonelink_engine::app::AppOutcome;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::messaging::{
    Attachment, MessagingError, PartRef, PcSettings, SendOutcome, SendStatus, SendTarget,
};
use phonelink_engine::seams::PlatformRequest;
use support::*;

fn is_lower_guid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}

/// Answers `/permission` with `permission_result` and everything else `{result: 0}`.
fn answer_permission(h: &Harness, permission_result: i32) {
    h.messenger.set_responder(move |r: &PlatformRequest| {
        if r.message.route() == Some("/permission") {
            Reply::Now(result_response(permission_result))
        } else {
            Reply::Now(result_response(0))
        }
    });
}

/// Seeds conversation 5 with one inbox SMS (subscription 1 in the US) and the subscription.
async fn seed(wd: &World, sub: Sub) {
    let item = ValueSet::new()
        .with("syncID", "seed")
        .with("subscriptions", subscriptions_object(&[sub]))
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+15550100", "hello")]))
        .with_all(conversations(&[(5, &["+15550100"])]));
    let v = push(&wd.h, item_msg(item)).await;
    assert_eq!(result_of(&v), Some(0));
}

// ------------------------------------------------------------------------------------------------
// Permission prompt and the sync request
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_accepted_prompt_sends_the_sync_and_the_contacts_sync() {
    let wd = world();
    answer_permission(&wd.h, 4);

    let outcome = wd.m.request_messages_permission().await.unwrap();
    assert_eq!(outcome, AppOutcome::Accepted);

    let prompt = wd.h.messenger.requests_on("/permission")[0].clone();
    let vs = payload(&prompt);
    assert_eq!(vs.str("contentType"), Some("permission"));
    assert_eq!(vs.i32("permissionType"), Some(2));
    assert_eq!(vs.bool("silent"), Some(false));
    assert!(vs.str("permissionSessionId").unwrap().starts_with('{'));
    assert_eq!(vs.get("isuserinitiated"), None, "a KVP request: no legacy keys");
    assert_eq!(prompt.priority, 0, "user-initiated");

    wd.h.messenger.wait_requests(2).await;
    let sync = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(sync.str("contentType"), Some("messaging"));
    let cv = sync.str("correlationVector").unwrap();
    assert_eq!(cv.len(), 16);
    assert_eq!(sync.bool("isuserinitiated"), Some(true));
    assert_eq!(sync.bool("ignoreResponse"), Some(false));
    assert_eq!(sync.i32("transportType"), Some(0));
    assert_eq!(
        sync.string_array("subcontentTypes").unwrap(),
        ["messages", "messagesMms", "conversations", "subscriptions", "messagesMmsMedia"]
    );
    assert_eq!(
        sync.keys().collect::<Vec<_>>()[..4],
        ["correlationVector", "contentType", "subcontentTypes", "configuration"]
    );
    assert_eq!(*wd.contacts.syncs.lock().unwrap(), vec![true]);
    assert!(wd.m.phone_permission().unwrap());
}

#[tokio::test(start_paused = true)]
async fn a_denied_prompt_sends_no_sync_and_a_timed_out_one_does() {
    let wd = world();
    answer_permission(&wd.h, 5);

    assert_eq!(wd.m.request_messages_permission().await.unwrap(), AppOutcome::Denied);
    tokio::task::yield_now().await;
    assert!(chat_requests(&wd.h).is_empty());
    assert!(!wd.m.phone_permission().unwrap());

    // An outcome that leaves the permission unchanged at its default sends the sync.
    let wd = world();
    wd.h.messenger.set_responder(|r: &PlatformRequest| {
        if r.message.route() == Some("/permission") { Reply::Hang } else { Reply::Now(result_response(0)) }
    });

    assert_eq!(wd.m.request_messages_permission().await.unwrap(), AppOutcome::TimedOut);

    wd.h.messenger.wait_requests(2).await;
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn no_second_sync_while_one_is_running_and_its_outcome_sets_the_permission() {
    let wd = world();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut rx = Some(rx);
    wd.h.messenger.set_responder(move |r: &PlatformRequest| match r.message.route() {
        Some("/permission") => Reply::Now(result_response(4)),
        _ => match rx.take() {
            Some(rx) => Reply::Later(rx),
            None => Reply::Now(result_response(0)),
        },
    });

    wd.m.request_messages_permission().await.unwrap();
    wd.h.messenger.wait_requests(2).await;
    assert!(wd.m.sync_state().unwrap().syncing);

    wd.m.request_messages_permission().await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(chat_requests(&wd.h).len(), 1, "the second prompt sends no sync");

    // The sync answers "needs permission": the permission becomes denied.
    tx.send(result_response(7)).unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    assert!(!wd.m.phone_permission().unwrap());
    assert!(!wd.m.sync_state().unwrap().syncing);
}

#[tokio::test(start_paused = true)]
async fn the_sync_omits_mms_media_without_receive_or_auto_download() {
    let wd = world();
    answer_permission(&wd.h, 4);
    let without_media = ["messages", "messagesMms", "conversations", "subscriptions"];

    // Auto-download off.
    wd.m.set_pc_settings(PcSettings { auto_download_mms: false, ..PcSettings::default() }).unwrap();
    wd.m.request_messages_permission().await.unwrap();
    wd.h.messenger.wait_requests(2).await;
    assert_eq!(chat_requests(&wd.h).pop().unwrap().string_array("subcontentTypes").unwrap(), without_media);

    // Receive MMS off (auto-download on).
    wd.m.set_pc_settings(PcSettings { receive_mms: false, ..PcSettings::default() }).unwrap();
    wd.m.request_messages_permission().await.unwrap();
    wd.h.messenger.wait_requests(4).await;
    assert_eq!(chat_requests(&wd.h).pop().unwrap().string_array("subcontentTypes").unwrap(), without_media);
}

// ------------------------------------------------------------------------------------------------
// Media download
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn only_parts_shown_as_downloadable_can_be_downloaded() {
    let wd = world();
    let parts = [
        Part { id: 1, message: 20, content_type: "image/jpeg".into(), text: String::new(), bytes: None },
        Part { id: 2, message: 20, content_type: "image/png".into(), text: String::new(), bytes: Some(vec![1]) },
        Part { id: 3, message: 20, content_type: "audio/mpeg".into(), text: String::new(), bytes: None },
        Part { id: 4, message: 20, content_type: "text/plain".into(), text: "t".into(), bytes: None },
    ];
    push(&wd.h, item_msg(ValueSet::new().with("mmsParts", parts_object(&parts)))).await;

    wd.m.download_part(PartRef::MmsPart(1)).await.unwrap();

    let req = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(req.string_array("subcontentTypes").unwrap(), ["messagesMmsMedia"]);
    assert_eq!(req.i64_array("messagingMmsPartIds"), Some([1i64].as_slice()));
    assert_eq!(req.i32("requestedItems"), Some(1));
    assert_eq!(req.bool("isuserinitiated"), Some(false), "default options although the user acted");
    assert!(req.get("messagingRcsFileTransferMediaIds").is_none());
    assert_eq!(
        req.keys().collect::<Vec<_>>()[..6],
        [
            "correlationVector",
            "contentType",
            "subcontentTypes",
            "messagingMmsPartIds",
            "requestedItems",
            "configuration"
        ]
    );

    assert_matches!(
        wd.m.download_part(PartRef::MmsPart(2)).await,
        Err(MessagingError::NotDownloadable),
        "an image with bytes"
    );
    assert_matches!(
        wd.m.download_part(PartRef::MmsPart(3)).await,
        Err(MessagingError::NotDownloadable),
        "audio parts off"
    );
    assert_matches!(wd.m.download_part(PartRef::MmsPart(4)).await, Err(MessagingError::NotDownloadable), "MMS text");
    assert_matches!(wd.m.download_part(PartRef::MmsPart(9)).await, Err(MessagingError::UnknownPart(9)));
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_text_rcs_file_is_downloadable_even_with_bytes() {
    let wd = world();
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "c").with("settings", ValueSet::new()))).await; // RCS receive
    let item = ValueSet::new()
        .with_all(rcs_conversations(&[(70, 70, 1, &["+1"])]))
        .with("rcsFileTransfers", rcs_files_object(&[(8, 70, "text/plain", Some(vec![1, 2]))], &[0]));
    push(&wd.h, item_msg(item)).await;

    wd.m.download_part(PartRef::RcsFile(8)).await.unwrap();

    let req = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(req.i64_array("messagingRcsFileTransferMediaIds"), Some([8i64].as_slice()));
    assert!(req.get("messagingMmsPartIds").is_none());
}

// ------------------------------------------------------------------------------------------------
// Sending
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn send_to_an_existing_conversation() {
    let mut wd = world();
    seed(&wd, Sub::new(1, "us")).await;

    let out = wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "hi", Vec::new()).await.unwrap();

    assert_eq!(out.send_id, 1, "the counter starts at 1");
    assert_eq!(out.status, SendStatus::Sent);

    let req = wd.h.messenger.requests_on("/chat/v1").pop().unwrap();
    assert_eq!(req.priority, 0, "user-initiated");
    let vs = payload(&req);
    assert_eq!(vs.i32("requestType"), Some(2));
    assert_eq!(vs.i64("id"), Some(1));
    assert!(is_lower_guid(vs.str("tempId").unwrap()));
    assert_eq!(vs.i32("subscriptionId"), Some(1), "the last message's subscription");
    assert_eq!(vs.i64("threadId"), Some(5));
    assert!(vs.get("recipients").is_none());
    assert_eq!(vs.str("body"), Some("hi"));
    assert!(vs.get("partCount").is_none());
    assert_eq!(vs.bool("isuserinitiated"), Some(true));
    assert_eq!(
        vs.keys().collect::<Vec<_>>()[..9],
        [
            "correlationVector",
            "contentType",
            "requestType",
            "id",
            "tempId",
            "subscriptionId",
            "threadId",
            "body",
            "configuration"
        ]
    );

    // The pending send stays until the phone acks it; then it is deleted.
    assert_eq!(wd.m.pending_sends().unwrap().len(), 1);
    let views = wd.m.messages(5).unwrap();
    assert_eq!(views.last().unwrap().send_status, Some(SendStatus::Sent));

    drain(&mut wd.events);
    push(&wd.h, item_msg(ValueSet::new().with("acks", ValueSet::new().with("ids", vec![1i64])))).await;
    assert!(wd.m.pending_sends().unwrap().is_empty());
    assert!(
        drain(&mut wd.events).iter().any(|e| matches!(
            e,
            phonelink_engine::features::messaging::MessagingEvent::SendChanged { send_id: 1, .. }
        ))
    );

    // Ids are never reused.
    let out = wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "again", Vec::new()).await.unwrap();
    assert_eq!(out.send_id, 2);
}

#[tokio::test(start_paused = true)]
async fn send_to_a_new_conversation_uses_the_entered_addresses() {
    let wd = world();
    let target = SendTarget::NewConversation { recipients: vec!["(650) 555-0100".into()] };
    wd.m.send_message(target, "hey", Vec::new()).await.unwrap();

    let vs = chat_requests(&wd.h).pop().unwrap();
    assert!(vs.get("threadId").is_none());
    assert_eq!(vs.string_array("recipients"), Some(["(650) 555-0100".to_string()].as_slice()), "not normalised");
    assert_eq!(vs.i32("subscriptionId"), Some(-1), "no subscription applies");
}

#[tokio::test(start_paused = true)]
async fn subscription_choice_order() {
    let wd = world();
    let mut a = Sub::new(1, "us");
    a.default_sms = false;
    let b = Sub::new(2, "us"); // default for SMS
    let item = ValueSet::new()
        .with("subscriptions", subscriptions_object(&[a, b]))
        .with("smsMessages", sms_object(&[Sms { sub: 9, ..Sms::inbox(1, 5, "+1", "x") }]))
        .with_all(conversations(&[(5, &["+1"])]));
    push(&wd.h, item_msg(item)).await;

    let sent_sub = |wd: &World| chat_requests(&wd.h).pop().unwrap().i32("subscriptionId");
    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "x", Vec::new()).await.unwrap();
    assert_eq!(sent_sub(&wd), Some(2), "the last message's subscription 9 does not exist: the default for SMS");

    wd.m.select_subscription(5, Some(1)).unwrap();
    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "x", Vec::new()).await.unwrap();
    assert_eq!(sent_sub(&wd), Some(1), "the user's choice");
}

#[tokio::test(start_paused = true)]
async fn attachments_fit_the_carrier_limits_and_get_names() {
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.max_size = 1000;
    sub.max_w = 640;
    seed(&wd, sub).await;

    let png = [b"\x89PNG".as_slice(), &[0u8; 1996]].concat(); // 2000 bytes
    let jpeg = [[0xFFu8, 0xD8, 0xFF].as_slice(), &[0u8; 1997]].concat(); // 2000 bytes
    let attachments = vec![
        Attachment { name: "a<b>:c.png".into(), content_type: Some("image/png".into()), bytes: png },
        Attachment { name: String::new(), content_type: None, bytes: jpeg },
    ];

    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "", attachments).await.unwrap();

    // ratio = 1000 / 4000: each image gets 500 bytes; width 640, the default height 2592.
    let calls = wd.imaging.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!((calls[0].max_width, calls[0].max_height, calls[0].max_bytes), (640, 2592, 500));

    let vs = chat_requests(&wd.h).pop().unwrap();
    assert!(vs.get("body").is_none(), "empty text is omitted");
    assert_eq!(vs.i32("partCount"), Some(2));
    assert_eq!(vs.string_array("partNames"), Some(["abc.png".to_string(), "image-1.jpg".to_string()].as_slice()));
    assert_eq!(
        vs.string_array("partContentTypes"),
        Some(["image/png".to_string(), "image/jpeg".to_string()].as_slice())
    );
    assert_eq!(vs.u32_array("partByteCounts"), Some([500u32, 500].as_slice()));
    assert_eq!(vs.bytes("partBytes").map(<[u8]>::len), Some(1000));
}

#[tokio::test(start_paused = true)]
async fn the_produced_content_type_is_sent_and_a_failed_fit_sends_the_original() {
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;
    wd.imaging.set_output_type(Some("image/jpeg"));
    let png = [b"\x89PNG".as_slice(), &[0u8; 6]].concat();
    let a = Attachment { name: "pic.png".into(), content_type: Some("image/png".into()), bytes: png.clone() };

    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "", vec![a.clone()]).await.unwrap();
    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.string_array("partContentTypes"), Some(["image/jpeg".to_string()].as_slice()));
    assert_eq!(vs.string_array("partNames"), Some(["pic.png.jpg".to_string()].as_slice()));

    wd.imaging.set_fail(true);
    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "", vec![a]).await.unwrap();
    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.bytes("partBytes"), Some(png.as_slice()));
    assert_eq!(vs.string_array("partContentTypes"), Some(["image/png".to_string()].as_slice()));
}

#[tokio::test(start_paused = true)]
async fn send_gates_block_without_creating_a_pending_send() {
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.max_text = 3;
    seed(&wd, sub).await;
    let c5 = SendTarget::Conversation { thread_id: 5 };

    assert_matches!(wd.m.send_message(c5.clone(), "", Vec::new()).await, Err(MessagingError::NothingToSend));
    let unknown = Attachment { name: "x".into(), content_type: None, bytes: b"plain".to_vec() };
    assert_matches!(wd.m.send_message(c5.clone(), "", vec![unknown]).await, Err(MessagingError::UnknownAttachmentType));
    let png = Attachment { name: "x.png".into(), content_type: Some("image/png".into()), bytes: b"\x89PNG".to_vec() };
    assert_matches!(
        wd.m.send_message(c5.clone(), "", vec![png.clone(), png.clone(), png.clone()]).await,
        Err(MessagingError::TooManyAttachments { max: 2 })
    );
    assert!(matches!(wd.m.send_message(c5.clone(), "long", Vec::new()).await, Err(MessagingError::NotAllowed(_))));
    wd.m.set_pc_settings(PcSettings { send_mms: false, ..PcSettings::default() }).unwrap();
    assert!(matches!(wd.m.send_message(c5.clone(), "", vec![png]).await, Err(MessagingError::NotAllowed(_))));
    assert_matches!(
        wd.m.send_message(SendTarget::Conversation { thread_id: 77 }, "x", Vec::new()).await,
        Err(MessagingError::UnknownConversation(77))
    );
    assert!(wd.m.pending_sends().unwrap().is_empty());
    assert!(chat_requests(&wd.h).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failed_send_keeps_its_ids_for_the_retry_and_updates_the_permission() {
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;
    wd.h.messenger
        .answer_all(response_with(ValueSet::new().with("result", 1i32).with("sendMessageFailureReason", 2i32)));

    let out = wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "hi", Vec::new()).await.unwrap();
    assert_eq!(out.status, SendStatus::Failed(2));
    let first = chat_requests(&wd.h).pop().unwrap();

    // Without a reason: Unknown (11); result 5 also denies the messages permission.
    wd.h.messenger.answer_all(result_response(5));
    let out = wd.m.retry_send(out.send_id).await.unwrap();
    assert_eq!(out.status, SendStatus::Failed(11));
    assert!(!wd.h.app.phone_record().unwrap().permission_or(permission::MESSAGES, true));

    let retry = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(retry.i64("id"), first.i64("id"));
    assert_eq!(retry.str("tempId"), first.str("tempId"));
    assert_ne!(retry.str("correlationVector"), first.str("correlationVector"));

    wd.h.messenger.answer_all(result_response(0));
    assert_eq!(wd.m.retry_send(out.send_id).await.unwrap().status, SendStatus::Sent);
    assert!(wd.m.phone_permission().unwrap(), "success grants");
    assert_matches!(wd.m.retry_send(99).await, Err(MessagingError::UnknownSend(99)));
}

#[tokio::test(start_paused = true)]
async fn a_retry_is_a_send_from_the_conversation_whatever_sent_the_message_first() {
    // The send response: the app offers the retry for every failed send, a notification reply's and
    // the send-message API's included; each retry's outcome updates the messages permission and
    // raises no "failed to send" notification (Phone Link on Windows offers it only in the
    // conversation view).
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;
    let failures = |wd: &World| wd.w.notifier.posted().into_iter().filter(|p| p.id.contains("/failed/")).count();

    wd.h.messenger.answer_all(result_response(1));
    assert!(!wd.m.send_text_to("+15550100", "on my way").await.unwrap());
    assert_eq!(failures(&wd), 0, "the send-message API's failure is not notified");

    let send_id = wd.m.pending_sends().unwrap()[0].id;
    wd.h.messenger.answer_all(result_response(5));
    assert_eq!(wd.m.retry_send(send_id).await.unwrap().status, SendStatus::Failed(11));
    assert!(!wd.m.phone_permission().unwrap(), "the retry's outcome denies the permission");
    assert_eq!(failures(&wd), 0, "the retry raises no notification");

    let retry = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(retry.string_array("recipients"), Some(["+15550100".to_string()].as_slice()));
    assert!(retry.get("threadId").is_none());

    wd.h.messenger.answer_all(result_response(0));
    assert_eq!(wd.m.retry_send(send_id).await.unwrap().status, SendStatus::Sent);
    assert!(wd.m.phone_permission().unwrap(), "success grants");
}

#[tokio::test(start_paused = true)]
async fn the_send_message_api_sends_text_to_one_number() {
    let wd = world();
    assert!(wd.m.send_text_to("+15550100", "on my way").await.unwrap());

    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.string_array("recipients"), Some(["+15550100".to_string()].as_slice()));
    assert_eq!(vs.i32("subscriptionId"), Some(-1));
    assert!(vs.get("threadId").is_none());

    // A failure raises no notification (none on Phone Link on Windows either).
    wd.h.messenger.answer_all(result_response(1));
    assert!(!wd.m.send_text_to("+15550100", "x").await.unwrap());
    assert!(!wd.w.notifier.posted().iter().any(|n| n.id.contains("/failed/")));
}

// ------------------------------------------------------------------------------------------------
// Mark conversation as read
// ------------------------------------------------------------------------------------------------

/// The conversation gets phone-unread messages; the phone supports mark-as-read (0x8).
async fn unread_conversation(wd: &World) {
    let item = ValueSet::new()
        .with("syncID", "u")
        .with("settings", ValueSet::new().with("markAsReadAvailable", true))
        .with(
            "smsMessages",
            sms_object(&[Sms { date: NOW_MS - 10, ..Sms::inbox(1, 5, "+1", "a") }, Sms::inbox(2, 5, "+1", "b")]),
        )
        .with_all(conversations(&[(5, &["+1"])]));
    push(&wd.h, item_msg(item)).await;

    // The reported capabilities turn RCS receive on: sync trigger (b) sends a sync request first.
    wd.h.messenger.wait_requests(1).await;
}

fn mark_requests(h: &Harness) -> Vec<ValueSet> {
    chat_requests(h).into_iter().filter(|r| r.i32("requestType") == Some(3)).collect()
}

/// Lets spawned triggers run until `n` mark-as-read requests were sent (at most 1 s of paused time).
async fn wait_marks(h: &Harness, n: usize) {
    for _ in 0..1000 {
        if mark_requests(h).len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn selecting_a_conversation_marks_it_read_after_500_ms_when_focused() {
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_window_focused(true);
    wd.m.on_conversation_selected(Some(5));

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(mark_requests(&wd.h).is_empty());

    tokio::time::sleep(Duration::from_millis(200)).await;
    let reqs = mark_requests(&wd.h);
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].i64("threadId"), Some(5));
    assert_eq!(reqs[0].i64("timestamp"), Some(NOW_MS), "the latest message");
    assert_eq!(reqs[0].bool("isuserinitiated"), Some(true));

    let conv = wd.m.conversations().unwrap();
    assert_eq!(conv[0].pc_unread, 0, "the PC cleared its own unread count");
    assert_eq!(conv[0].phone_unread, 2, "until the phone syncs the read state");
}

#[tokio::test(start_paused = true)]
async fn the_delayed_trigger_needs_focus_and_a_newer_one_cancels_it() {
    let wd = world();
    unread_conversation(&wd).await;
    let item = ValueSet::new()
        .with("syncID", "six")
        .with("smsMessages", sms_object(&[Sms::inbox(9, 6, "+2", "c")]))
        .with_all(conversations(&[(6, &["+2"])]));
    push(&wd.h, item_msg(item)).await;

    wd.m.on_conversation_selected(Some(5));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mark_requests(&wd.h).is_empty(), "not focused when the delay ended");

    wd.m.on_window_focused(true); // the messages view is not active: no trigger

    // A newer trigger cancels the older one: only conversation 6 is marked.
    wd.m.on_conversation_selected(Some(5));
    tokio::time::sleep(Duration::from_millis(300)).await;
    wd.m.on_conversation_selected(Some(6));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let marks = mark_requests(&wd.h);
    assert_eq!(marks.len(), 1);
    assert_eq!(marks[0].i64("threadId"), Some(6));

    // Losing focus cancels.
    wd.m.on_conversation_selected(Some(5));
    wd.m.on_window_focused(false);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(mark_requests(&wd.h).len(), 1, "losing focus cancels");

    // Gaining focus outside the active messages view is no trigger.
    wd.m.on_messages_view_changed(false);
    wd.m.on_window_focused(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(mark_requests(&wd.h).len(), 1, "the messages view is not active");
}

#[tokio::test(start_paused = true)]
async fn clearing_the_selection_does_not_cancel_the_pending_mark() {
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_window_focused(true);
    wd.m.on_conversation_selected(Some(5));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // No selection is not a trigger of marking the conversation read: it cancels nothing.
    wd.m.on_conversation_selected(None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let marks = mark_requests(&wd.h);
    assert_eq!(marks.len(), 1, "the selection's 500 ms trigger still runs");
    assert_eq!(marks[0].i64("threadId"), Some(5));
}

#[tokio::test(start_paused = true)]
async fn navigating_to_the_view_and_a_message_in_the_open_conversation_mark_at_once() {
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    wait_marks(&wd.h, 1).await;
    tokio::task::yield_now().await;
    assert_eq!(mark_requests(&wd.h).len(), 1, "at once, without the focus check (the 500 ms one saw no focus)");

    // A message arrives in the open conversation while the window is focused in the active view.
    wd.m.on_window_focused(true);
    push(
        &wd.h,
        item_msg(ValueSet::new().with("syncID", "n").with("smsMessages", sms_object(&[Sms::inbox(3, 5, "+1", "c")]))),
    )
    .await;
    wait_marks(&wd.h, 2).await;
    assert_eq!(mark_requests(&wd.h).len(), 2);
}

#[tokio::test(start_paused = true)]
async fn without_support_or_connection_only_the_pc_count_is_cleared() {
    let wd = world();
    let item = ValueSet::new()
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]))
        .with_all(conversations(&[(5, &["+1"])]));
    push(&wd.h, item_msg(item)).await;

    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    assert!(mark_requests(&wd.h).is_empty(), "no MarkConversationAsRead capability");
    assert_eq!(wd.m.conversations().unwrap()[0].pc_unread, 0);

    // Not connected: nothing happens at all.
    let wd = world();
    unread_conversation(&wd).await;
    wd.h.sessions.set_connected(false);
    wd.h.sessions.set_behavior(ConnectBehavior::Pending);

    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }

    assert!(mark_requests(&wd.h).is_empty());
    assert_eq!(wd.m.conversations().unwrap()[0].pc_unread, 2);
}

#[tokio::test(start_paused = true)]
async fn group_copies_are_hidden_and_events_announce_content() {
    let wd = world();
    let rows = [Sms { ..Sms::inbox(10, 5, "+1", "copy a") }, Sms { ..Sms::inbox(11, 5, "+1", "copy b") }];
    let mut obj = sms_object(&rows);
    obj.insert("groupIds", vec![10i64, 10]);
    push(&wd.h, item_msg(ValueSet::new().with("smsMessages", obj).with_all(conversations(&[(5, &["+1"])])))).await;

    let ids: Vec<i64> = wd.m.messages(5).unwrap().iter().map(|m| m.id).collect();
    assert_eq!(ids, vec![10], "only the copy whose id is the group id is shown");
    let _ = Value::Bool(true);
}

#[tokio::test(start_paused = true)]
async fn no_permission_flow_sync_while_the_syncing_state_shows() {
    let wd = world();
    // The connect reports an expected anchor ahead of the current one: not up to date, syncing shows.
    wd.h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32).with("sms", 5i64)));
    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    assert_eq!(
        wd.m.sync_state().unwrap(),
        phonelink_engine::features::messaging::SyncState { up_to_date: false, syncing: true }
    );

    answer_permission(&wd.h, 4);
    wd.m.request_messages_permission().await.unwrap();
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(chat_requests(&wd.h).is_empty(), "skipped while syncing shows");

    // After at most 2 minutes the syncing state ends and the flow sends its sync again.
    tokio::time::sleep(Duration::from_secs(121)).await;
    assert!(!wd.m.sync_state().unwrap().syncing);
    wd.m.request_messages_permission().await.unwrap();
    wd.h.messenger.wait_requests(4).await;
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_message_that_did_not_fail_cannot_be_retried() {
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;
    let out = wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "hi", Vec::new()).await.unwrap();

    assert_eq!(out.status, SendStatus::Sent);
    assert_matches!(wd.m.retry_send(out.send_id).await, Err(MessagingError::NotFailed(got0)) if got0 == out.send_id);
    let sends = chat_requests(&wd.h).into_iter().filter(|r| r.i32("requestType") == Some(2)).count();
    assert_eq!(sends, 1, "nothing sent again");
}

#[tokio::test(start_paused = true)]
async fn a_send_to_an_rcs_only_conversation_uses_its_fallback_id() {
    let wd = world();
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "c").with("settings", ValueSet::new()))).await; // RCS receive
    let item = ValueSet::new()
        .with("syncID", "r")
        .with_all(rcs_conversations(&[(70, 55, 1, &["+1"])]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+1", "a")]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    wd.m.send_message(SendTarget::Conversation { thread_id: 55 }, "x", Vec::new()).await.unwrap();
    let send = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    assert_eq!(send.i64("threadId"), Some(55));
}

#[tokio::test(start_paused = true)]
async fn marking_an_rcs_only_conversation_uses_its_fallback_id_and_skips_group_events() {
    let wd = world();
    let caps = ValueSet::new().with("syncID", "c").with("settings", ValueSet::new().with("markAsReadAvailable", true));
    assert_eq!(result_of(&push(&wd.h, item_msg(caps)).await), Some(0));

    let mut chats = rcs_chats_object(&[(1, 70, "+1", "a"), (2, 70, "+1", "b")]);
    chats.insert("dates", vec![NOW_MS - 1000, NOW_MS]);
    chats.insert("rcsMessageTypes", vec![0i32, 3]); // the newer one is a group event
    let item = ValueSet::new()
        .with("syncID", "r")
        .with_all(rcs_conversations(&[(70, 55, 1, &["+1"])]))
        .with("rcsChats", chats);
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    // The group event is neither unread nor the latest message.
    let conv = wd.m.conversations().unwrap().into_iter().find(|c| c.thread_id == 55).unwrap();
    assert_eq!((conv.pc_unread, conv.phone_unread, conv.latest_ms), (1, 1, NOW_MS - 1000));

    wd.h.app.set_foreground(true);
    wd.m.on_conversation_selected(Some(55));
    wd.m.on_messages_view_changed(true);
    wait_marks(&wd.h, 1).await;

    let marks = mark_requests(&wd.h);
    assert_eq!(marks[0].i64("threadId"), Some(55));
    assert_eq!(marks[0].i64("timestamp"), Some(NOW_MS - 1000));
}

/// An imaging edge whose transcode takes `ms` (`ImageIO` re-encoding a large photo).
struct SlowImaging(u64);

impl phonelink_engine::features::messaging::AttachmentImaging for SlowImaging {
    fn sniff_content_type(&self, _bytes: Vec<u8>) -> Option<String> {
        Some("image/jpeg".into())
    }
    fn fit_image(
        &self,
        bytes: Vec<u8>,
        _w: u32,
        _h: u32,
        _max: u64,
    ) -> Result<Option<phonelink_engine::features::messaging::FittedImage>, phonelink_engine::edge::EdgeError> {
        std::thread::sleep(Duration::from_millis(self.0));
        Ok(Some(phonelink_engine::features::messaging::FittedImage { bytes, content_type: "image/jpeg".into() }))
    }
}

/// The imaging edge blocks; it runs off the runtime's worker, so other tasks keep running while
/// `send_message` transcodes an attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn attachment_imaging_runs_off_the_runtime_worker() {
    use phonelink_engine::features::messaging::edge::standins::{FakeFileTypes, FixedProximity};
    use phonelink_engine::features::messaging::{Messaging, MessagingEdges, ProximityState};
    use phonelink_engine::features::{ContactDirectory, StoreLocation};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    let h = Harness::new();
    let w = feature_world::feature_world_at(&h, StoreLocation::Memory, device_item(Some(ASYNC_TRUST_CUSTOM_DATA)));
    let edges = MessagingEdges {
        imaging: Arc::new(SlowImaging(400)),
        file_types: Arc::new(FakeFileTypes),
        proximity: FixedProximity::new(ProximityState::Connected),
    };
    let contacts: Arc<dyn ContactDirectory> = FakeContacts::with(&[]);
    let m = Messaging::install(&w.env, MessagingConfig::default(), Some(contacts), edges).unwrap();

    let item = ValueSet::new()
        .with("syncID", "s")
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]))
        .with_all(conversations(&[(5, &["+1"])]));
    assert_eq!(result_of(&push(&h, item_msg(item)).await), Some(0));

    // A ticker on the same runtime records when it gets to run.
    let ticks: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
    let t2 = ticks.clone();
    let ticker = tokio::spawn(async move {
        loop {
            t2.lock().unwrap().push(Instant::now());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    assert!(eventually(|| ticks.lock().unwrap().len() >= 2).await, "the ticker runs");

    let a =
        Attachment { name: "p.jpg".into(), content_type: Some("image/jpeg".into()), bytes: vec![0xFF, 0xD8, 0xFF, 0] };
    // The UI's call runs as a task on the runtime, as a uniffi async call does.
    let m2 = m.clone();
    tokio::spawn(async move { m2.send_message(SendTarget::Conversation { thread_id: 5 }, "", vec![a]).await })
        .await
        .unwrap()
        .unwrap();

    // A tick after the send closes the gap the send may have left.
    let sent = Instant::now();
    assert!(eventually(|| ticks.lock().unwrap().last().is_some_and(|t| *t > sent)).await, "the ticker runs on");

    ticker.abort();
    let ticks = ticks.lock().unwrap().clone();
    let max_gap = ticks.windows(2).map(|w| w[1] - w[0]).max().unwrap();
    assert!(max_gap < Duration::from_millis(200), "the runtime worker was blocked for {max_gap:?}");
}

#[tokio::test(start_paused = true)]
async fn a_replaced_row_read_on_the_pc_stays_read_but_a_file_transfer_does_not() {
    let wd = world();
    unread_conversation(&wd).await; // SMS 1 and 2 in conversation 5; mark-as-read and RCS receive on
    let item = ValueSet::new()
        .with("syncID", "ft")
        .with_all(rcs_conversations(&[(70, 5, 1, &["+1"])]))
        .with("rcsFileTransfers", rcs_files_object(&[(4, 70, "image/png", Some(vec![1, 2]))], &[0]));
    push(&wd.h, item_msg(item)).await;

    let conv5 = |wd: &World| wd.m.conversations().unwrap().into_iter().find(|c| c.thread_id == 5).unwrap();
    assert_eq!(conv5(&wd).pc_unread, 3);

    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    wait_marks(&wd.h, 1).await;
    assert_eq!(conv5(&wd).pc_unread, 0, "read on the PC");

    // The phone sends the rows again, still unread on the phone, the file without its bytes.
    let item = ValueSet::new()
        .with("syncID", "again")
        .with(
            "smsMessages",
            sms_object(&[Sms { date: NOW_MS - 10, ..Sms::inbox(1, 5, "+1", "a") }, Sms::inbox(2, 5, "+1", "b")]),
        )
        .with("rcsFileTransfers", rcs_files_object(&[(4, 70, "image/png", None)], &[0]));
    push(&wd.h, item_msg(item)).await;

    assert_eq!(conv5(&wd).pc_unread, 1, "the SMS stay read on the PC; the replaced file transfer is unread again");
    let file = wd.m.messages(5).unwrap().into_iter().find(|m| m.id == 4).unwrap();
    assert!(!file.parts[0].has_bytes, "a replaced file transfer loses its bytes");
    assert_eq!(wd.m.part_bytes(file.parts[0].part).unwrap(), None);
}

// ------------------------------------------------------------------------------------------------
// Composer gates, send bytes and the remaining mark-as-read triggers (audit)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_non_image_attachment_is_refused_before_anything_is_stored() {
    // Rule 5: a message carries images (GIFs included) only. Any other attachment is refused before a
    // pending send is stored, and nothing is sent.
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;
    let pdf = Attachment {
        name: "doc.pdf".into(),
        content_type: Some("application/pdf".into()),
        bytes: b"%PDF-1.4".to_vec(),
    };
    let png = Attachment { name: "x.png".into(), content_type: Some("image/png".into()), bytes: b"\x89PNG".to_vec() };

    assert_matches!(wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "see", vec![png, pdf]).await, Err(MessagingError::NotAnImage { content_type }) if content_type == "application/pdf");
    assert!(wd.m.pending_sends().unwrap().is_empty());
    assert!(chat_requests(&wd.h).is_empty());
    assert!(wd.imaging.calls().is_empty(), "nothing was fitted");

    // A GIF is an image (its type detected from the bytes here).
    let gif = Attachment { name: "a.gif".into(), content_type: None, bytes: b"GIF89a\x01\x00".to_vec() };
    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "", vec![gif]).await.unwrap();
    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.string_array("partContentTypes"), Some(["image/gif".to_string()].as_slice()));
}

#[tokio::test(start_paused = true)]
async fn a_granted_outcome_marks_messaging_set_up_once() {
    // The sync request's outcomes: success or accepted also marks messaging "set up once".
    let wd = world();
    answer_permission(&wd.h, 5);
    wd.m.request_messages_permission().await.unwrap();
    assert!(!wd.m.setup_completed_once().unwrap(), "denied");

    answer_permission(&wd.h, 4);
    wd.m.request_messages_permission().await.unwrap();
    assert!(wd.m.setup_completed_once().unwrap(), "accepted");
}

#[tokio::test(start_paused = true)]
async fn part_bytes_are_the_attachments_in_order() {
    // The send request: `partBytes` is the attachments' bytes concatenated in order, `partByteCounts`
    // the length of each.
    let wd = world();
    seed(&wd, Sub::new(1, "us")).await;

    let png = [b"\x89PNG".as_slice(), &[1u8; 6]].concat();
    let jpeg = [[0xFFu8, 0xD8, 0xFF].as_slice(), &[2u8; 3]].concat();
    let attachments = vec![
        Attachment { name: "a.png".into(), content_type: Some("image/png".into()), bytes: png.clone() },
        Attachment { name: "b.jpg".into(), content_type: Some("image/jpeg".into()), bytes: jpeg.clone() },
    ];

    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "", attachments).await.unwrap();

    let vs = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(vs.bytes("partBytes"), Some([png.as_slice(), jpeg.as_slice()].concat().as_slice()));
    assert_eq!(vs.u32_array("partByteCounts"), Some([10u32, 6].as_slice()));
}

#[tokio::test(start_paused = true)]
async fn a_send_over_the_recipient_limit_is_refused() {
    // Rule 6: the composer limits the participants to `recipientLimits` (a value <= 1: unlimited).
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.recipient_limit = 2;
    seed(&wd, sub).await;
    let to = |n: usize| SendTarget::NewConversation { recipients: (1..=n).map(|i| format!("+1555010{i}")).collect() };

    assert!(matches!(wd.m.send_message(to(3), "hi", Vec::new()).await, Err(MessagingError::NotAllowed(_))));
    assert!(chat_requests(&wd.h).is_empty() && wd.m.pending_sends().unwrap().is_empty());

    wd.m.send_message(to(2), "hi", Vec::new()).await.unwrap();
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn attachments_are_refused_when_the_sim_disables_mms() {
    // Rule 9: attachments are offered only while the SIM's `isMmsEnableds` is true.
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.mms = false;
    seed(&wd, sub).await;
    let png = Attachment { name: "x.png".into(), content_type: Some("image/png".into()), bytes: b"\x89PNG".to_vec() };
    let c5 = SendTarget::Conversation { thread_id: 5 };

    assert!(matches!(wd.m.send_message(c5.clone(), "", vec![png]).await, Err(MessagingError::NotAllowed(_))));
    assert!(chat_requests(&wd.h).is_empty() && wd.m.pending_sends().unwrap().is_empty());

    wd.m.send_message(c5, "text only", Vec::new()).await.unwrap();
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_group_send_is_refused_when_group_mms_is_disabled() {
    // Rule 9: a group conversation cannot be sent to while the SIM's `isGroupMmsEnableds` is false.
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.group_mms = false;
    seed(&wd, sub).await;
    let group = SendTarget::NewConversation { recipients: vec!["+15550101".into(), "+15550102".into()] };

    assert!(matches!(wd.m.send_message(group, "hi", Vec::new()).await, Err(MessagingError::NotAllowed(_))));
    assert!(chat_requests(&wd.h).is_empty() && wd.m.pending_sends().unwrap().is_empty());

    wd.m.send_message(SendTarget::Conversation { thread_id: 5 }, "one recipient", Vec::new()).await.unwrap();
    assert_eq!(chat_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn rcs_group_replies_need_rcs_send_and_an_rcs_sim() {
    // Composition rule 9 and groups: a reply in an RCS group needs RCS send and the SIM's
    // `isRcsSupported`; an `InactiveGroupChat` cannot be replied to.
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.rcs = false;
    let item = ValueSet::new()
        .with("syncID", "g")
        .with("settings", ValueSet::new()) // RCS receive, no RCS send
        .with("subscriptions", subscriptions_object(&[sub.clone()]))
        .with_all(rcs_conversations(&[
            (70, 55, 2, &["+15550101", "+15550102"]),
            (71, 56, 3, &["+15550103", "+15550104"]),
        ]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+15550101", "a"), (2, 71, "+15550103", "b")]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let to = |t: i64| SendTarget::Conversation { thread_id: t };
    let refused = |r: Result<SendOutcome, MessagingError>| matches!(r, Err(MessagingError::NotAllowed(_)));

    assert!(refused(wd.m.send_message(to(55), "x", Vec::new()).await), "no RCS send");

    let rcs_send = ValueSet::new().with("syncID", "s").with("settings", ValueSet::new().with("rcsSendAvailable", true));
    push(&wd.h, item_msg(rcs_send)).await;
    assert!(refused(wd.m.send_message(to(55), "x", Vec::new()).await), "the SIM does not support RCS");

    sub.rcs = true;
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "t").with("subscriptions", subscriptions_object(&[sub]))))
        .await;
    wd.m.send_message(to(55), "x", Vec::new()).await.unwrap();
    assert!(refused(wd.m.send_message(to(56), "x", Vec::new()).await), "an inactive group");

    let sends = chat_requests(&wd.h).into_iter().filter(|r| r.i32("requestType") == Some(2)).count();
    assert_eq!(sends, 1);
}

#[tokio::test(start_paused = true)]
async fn an_rcs_group_reply_needs_a_subscription_that_supports_rcs() {
    // Rule 9: a reply in an RCS group needs the subscription's `isRcsSupported`; with no subscription
    // the "unknown" SIM applies, whose RCS send is off, while one-to-one RCS conversations and
    // attachments stay allowed (`isMmsEnableds` and `isGroupMmsEnableds` true).
    let wd = world();
    let item = ValueSet::new()
        .with("syncID", "g")
        .with("settings", ValueSet::new().with("rcsSendAvailable", true))
        .with_all(rcs_conversations(&[(70, 55, 2, &["+15550101", "+15550102"]), (71, 56, 1, &["+15550103"])]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+15550101", "a"), (2, 71, "+15550103", "b")]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    assert!(wd.m.subscriptions().unwrap().is_empty());
    let view = |t: i64| wd.m.conversations().unwrap().into_iter().find(|c| c.thread_id == t).unwrap();
    assert!(!view(55).can_reply, "an RCS group without a subscription");
    assert!(view(56).can_reply && view(56).attachments_allowed, "a one-to-one RCS conversation");
    assert_eq!(view(55).active_subscription, -1);

    assert!(matches!(
        wd.m.send_message(SendTarget::Conversation { thread_id: 55 }, "x", Vec::new()).await,
        Err(MessagingError::NotAllowed(_))
    ));

    wd.m.send_message(SendTarget::Conversation { thread_id: 56 }, "x", Vec::new()).await.unwrap();
    let sends: Vec<_> = chat_requests(&wd.h).into_iter().filter(|r| r.i32("requestType") == Some(2)).collect();
    assert_eq!(sends.len(), 1);
    assert_eq!((sends[0].i64("threadId"), sends[0].i32("subscriptionId")), (Some(56), Some(-1)));
}

#[tokio::test(start_paused = true)]
async fn gaining_focus_with_a_selection_in_the_active_view_marks_after_500_ms() {
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_messages_view_changed(true); // nothing selected yet: no trigger
    wd.m.on_conversation_selected(Some(5)); // not focused when its 500 ms end: nothing
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mark_requests(&wd.h).is_empty());

    wd.h.app.set_foreground(true);
    wd.m.on_window_focused(true);

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(mark_requests(&wd.h).is_empty());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mark_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_composed_conversation_replaced_by_its_synced_one_is_marked_at_once() {
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_messages_view_changed(true);
    wd.m.on_composed_conversation_replaced(5); // not focused: nothing
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(mark_requests(&wd.h).is_empty());

    wd.m.on_window_focused(true);
    wd.m.on_composed_conversation_replaced(5);
    wait_marks(&wd.h, 1).await;
    assert_eq!(mark_requests(&wd.h).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_triggers_that_need_a_focused_window_read_the_focus_the_ui_reports() {
    // Mark-as-read triggers: "the window is focused" is the focus the UI reports
    // (`on_window_focused`), not the app being active — another of its windows may be key.
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    wait_marks(&wd.h, 1).await; // navigating to the view marks at once, focus or not
    assert_eq!(mark_requests(&wd.h).len(), 1);

    wd.h.app.set_foreground(true); // the app is active; its main window did not report focus
    let arrive = |n: i64| {
        item_msg(
            ValueSet::new()
                .with("syncID", format!("n{n}"))
                .with("smsMessages", sms_object(&[Sms::inbox(n, 5, "+1", "c")])),
        )
    };

    push(&wd.h, arrive(3)).await;
    wd.m.on_composed_conversation_replaced(5);
    wd.m.on_conversation_selected(Some(5));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(mark_requests(&wd.h).len(), 1, "no trigger saw a focused window");

    wd.m.on_window_focused(true);
    wd.m.on_window_focused(false); // and lost again: a later delayed trigger finds no focus
    wd.m.on_conversation_selected(Some(5));
    push(&wd.h, arrive(4)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(mark_requests(&wd.h).len(), 1);

    wd.m.on_window_focused(true);
    push(&wd.h, arrive(7)).await;
    wait_marks(&wd.h, 2).await;
    assert_eq!(mark_requests(&wd.h).len(), 2, "a message in the open conversation of the focused window");
}

#[tokio::test(start_paused = true)]
async fn a_focus_change_that_is_no_trigger_cancels_the_pending_selection_trigger() {
    // Mark-as-read triggers: a focus change that is not the focus trigger — here focus gained
    // outside the active messages view — cancels the pending 500 ms trigger.
    let wd = world();
    unread_conversation(&wd).await;
    wd.m.on_conversation_selected(Some(5)); // the messages view is not active
    tokio::time::sleep(Duration::from_millis(100)).await;

    wd.h.app.set_foreground(true);
    wd.m.on_window_focused(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mark_requests(&wd.h).is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unread_rcs_group_event_alone_is_not_marked_read() {
    // Marking a conversation read: RCS chats whose `rcsMessageTypes` is a group event (1–8) count as
    // neither PC- nor phone-unread.
    let wd = world();
    let caps = ValueSet::new().with("syncID", "c").with("settings", ValueSet::new().with("markAsReadAvailable", true));
    assert_eq!(result_of(&push(&wd.h, item_msg(caps)).await), Some(0));

    let mut chats = rcs_chats_object(&[(1, 70, "+15550100", "left")]);
    chats.insert("rcsMessageTypes", vec![3i32]);
    let item = ValueSet::new()
        .with("syncID", "r")
        .with_all(rcs_conversations(&[(70, 55, 2, &["+15550100", "+15550101"])]))
        .with("rcsChats", chats);
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    wd.h.app.set_foreground(true);
    wd.m.on_conversation_selected(Some(55));
    wd.m.on_messages_view_changed(true);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(mark_requests(&wd.h).is_empty());
}

// ------------------------------------------------------------------------------------------------
// New conversations, interrupted sends, part bytes and download outcomes (audit)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn new_conversation_sends_are_listed_with_their_addresses_until_acked() {
    let wd = world();
    wd.contacts.names.lock().unwrap().insert("010-5555-6666".into(), "Minji".into());
    let png = Attachment { name: "a.png".into(), content_type: Some("image/png".into()), bytes: b"\x89PNG".to_vec() };
    let target = SendTarget::NewConversation { recipients: vec!["010-5555-6666".into(), "+15550100".into()] };

    wd.h.messenger
        .answer_all(response_with(ValueSet::new().with("result", 1i32).with("sendMessageFailureReason", 17i32)));
    let out = wd.m.send_message(target, "hi", vec![png]).await.unwrap();
    assert_eq!(out.status, SendStatus::Failed(17));
    wd.m.send_text_to("+15550199", "on my way").await.unwrap();

    let listed = wd.m.new_conversation_messages().unwrap();
    assert_eq!(listed.len(), 2, "both have no thread yet");

    let first = &listed[0];
    let to: Vec<(&str, Option<&str>)> =
        first.recipients.iter().map(|r| (r.address.as_str(), r.name.as_deref())).collect();
    assert_eq!(to, [("010-5555-6666", Some("Minji")), ("+15550100", None)], "as entered, with their contacts");
    assert_eq!(first.message.send_status, Some(SendStatus::Failed(17)));
    assert_eq!(first.message.body, "hi");

    // The attachment is listed without its bytes; they are read per part.
    let part = &first.message.parts[0];
    assert_eq!((part.part, part.size, part.has_bytes), (PartRef::SentPart { send_id: out.send_id, index: 0 }, 4, true));
    assert_eq!(wd.m.part_bytes(part.part).unwrap(), Some(b"\x89PNG".to_vec()));
    assert_matches!(wd.m.part_bytes(PartRef::SentPart { send_id: out.send_id, index: 1 }), Err(MessagingError::UnknownSend(got0)) if got0 == out.send_id);
    assert_eq!(listed[1].recipients[0].address, "+15550199");
    assert!(
        wd.m.messages(-1)
            .unwrap()
            .iter()
            .all(|m| m.kind == phonelink_engine::features::messaging::MessageKind::PendingSend)
    );

    push(&wd.h, item_msg(ValueSet::new().with("acks", ValueSet::new().with("ids", vec![out.send_id])))).await;
    assert_eq!(wd.m.new_conversation_messages().unwrap().len(), 1, "an acked send is gone");
}

#[tokio::test(start_paused = true)]
async fn a_send_an_earlier_run_left_sending_fails_and_can_be_retried() {
    // When the store is first opened in a run, a pending send still marked sending (its run ended
    // before the outcome) is marked failed with reason Unknown (differs from Phone Link on Windows).
    let dir = tempfile::tempdir().unwrap();
    let location = phonelink_engine::features::StoreLocation::Directory(dir.path().to_path_buf());
    {
        let wd = world_with(Harness::new(), location.clone(), MessagingConfig::default());
        wd.h.messenger.set_responder(|_r: &PlatformRequest| Reply::Hang);

        let m = wd.m.clone();
        let sending = tokio::spawn(async move {
            m.send_message(SendTarget::NewConversation { recipients: vec!["+15550100".into()] }, "hi", Vec::new()).await
        });

        wd.h.messenger.wait_requests(1).await;
        assert_eq!(wd.m.pending_sends().unwrap()[0].status, SendStatus::Sending);
        sending.abort();
    }

    let wd = world_with(Harness::new(), location, MessagingConfig::default());
    let sends = wd.m.pending_sends().unwrap();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].status, SendStatus::Failed(11));

    let first = chat_requests(&wd.h).len();
    let out = wd.m.retry_send(sends[0].id).await.unwrap();
    assert_eq!(out.status, SendStatus::Sent);
    let retry = chat_requests(&wd.h).into_iter().nth(first).unwrap();
    assert_eq!((retry.i64("id"), retry.str("tempId")), (Some(sends[0].id), Some(sends[0].temp_id.as_str())));

    // A send of this run is not touched by a later open of the store in the same run.
    wd.h.messenger.set_responder(|_r: &PlatformRequest| Reply::Hang);
    let m = wd.m.clone();
    let sending = tokio::spawn(async move { m.send_text_to("+15550100", "again").await });
    wd.h.messenger.wait_requests(first + 2).await;
    assert_eq!(wd.m.pending_sends().unwrap().last().unwrap().status, SendStatus::Sending);
    sending.abort();
}

#[tokio::test(start_paused = true)]
async fn the_new_message_limits_follow_the_default_sim_and_the_send_mms_setting() {
    let wd = world();
    let limits = wd.m.new_message_limits().unwrap();
    assert!(limits.attachments_allowed && limits.group_allowed);
    assert_eq!((limits.max_text_length, limits.max_recipients, limits.active_subscription), (None, None, -1));

    let mut other = Sub::new(1, "us");
    other.default_sms = false;
    other.max_text = 10;
    let mut default = Sub::new(2, "us");
    default.mms = false;
    default.group_mms = false;
    default.max_text = 160;
    default.recipient_limit = 5;
    let item = ValueSet::new().with("syncID", "s").with("subscriptions", subscriptions_object(&[other, default]));
    push(&wd.h, item_msg(item)).await;

    let limits = wd.m.new_message_limits().unwrap();
    assert!(!limits.attachments_allowed, "the default SIM disables MMS");
    assert!(!limits.group_allowed);
    assert_eq!((limits.max_text_length, limits.max_recipients, limits.active_subscription), (Some(160), Some(5), 2));

    // "Send MMS" off on the PC also takes attachments away.
    let wd = world();
    wd.m.set_pc_settings(PcSettings { send_mms: false, ..PcSettings::default() }).unwrap();
    assert!(!wd.m.new_message_limits().unwrap().attachments_allowed);
}

#[tokio::test(start_paused = true)]
async fn a_download_returns_its_outcome_and_runs_once_at_a_time() {
    let wd = world();
    let parts = [Part { id: 1, message: 20, content_type: "image/jpeg".into(), text: String::new(), bytes: None }];
    push(&wd.h, item_msg(ValueSet::new().with("mmsParts", parts_object(&parts)))).await;

    assert_eq!(wd.m.download_part(PartRef::MmsPart(1)).await.expect("Ok"), AppOutcome::Success);
    wd.h.messenger.answer_all(result_response(1));
    assert_eq!(wd.m.download_part(PartRef::MmsPart(1)).await.expect("Ok"), AppOutcome::Failure);

    // A second request for the same part while one is under way is refused.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut rx = Some(rx);
    wd.h.messenger.set_responder(move |_r: &PlatformRequest| match rx.take() {
        Some(rx) => Reply::Later(rx),
        None => Reply::Now(result_response(0)),
    });

    let m = wd.m.clone();
    let first = tokio::spawn(async move { m.download_part(PartRef::MmsPart(1)).await });
    wd.h.messenger.wait_requests(3).await;

    assert!(matches!(wd.m.download_part(PartRef::MmsPart(1)).await, Err(MessagingError::NotAllowed(_))));
    tx.send(result_response(0)).unwrap();
    assert_eq!(first.await.unwrap().expect("Ok"), AppOutcome::Success);

    assert_matches!(
        wd.m.download_part(PartRef::SentPart { send_id: 1, index: 0 }).await,
        Err(MessagingError::NotDownloadable),
        "a sent attachment is never downloaded"
    );
}
