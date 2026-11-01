//! Inbound messaging content: the exchange model, the metadata message, delta computation, the
//! metadata response, the item message, item processing and response, sync anchors, identity,
//! timestamps and enumerations, error handling, and a worked example.

use crate::assert_matches;
use crate::support::app_world;
use crate::support::common::payload;
use crate::support::messaging_world as support;
use phonelink_engine::config::features::MessagingConfig;

use serde_json::json;

use app_world::*;
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::StoreLocation;
use phonelink_engine::features::messaging::PcSettings;
use support::*;

/// The `(id, checksum)` rows of a store table, through the views.
fn sms_ids(wd: &World, thread: i64) -> Vec<i64> {
    wd.m.messages(thread).unwrap().iter().map(|m| m.id).collect()
}

/// Reports `capabilities` through an item's `settings` (an empty exchange: completes at once).
async fn report_caps(wd: &World, caps: i32) {
    let v = push(
        &wd.h,
        item_msg(ValueSet::new().with("syncID", "caps").with("settings", ValueSet::new().with("capabilities", caps))),
    )
    .await;
    assert_eq!(result_of(&v), Some(0));
}

fn base_metadata() -> ValueSet {
    ValueSet::new()
        .with("messagingSmsIds", vec![101i64, 102])
        .with("messagingSmsChecksums", vec![7i64, 9])
        .with("messagingMmsIds", Vec::<i64>::new())
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", vec![5i64])
        .with("messagingSubscriptionIds", vec![1i64])
}

#[tokio::test(start_paused = true)]
async fn example_metadata_response_and_completion() {
    let wd = world();
    // The PC holds SMS 101 with checksum 7 (thread 5) and nothing else.
    let mut held = Sms::inbox(101, 5, "+15550100", "hi");
    held.checksum = 7;
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "seed").with("smsMessages", sms_object(&[held])))).await;

    let md = base_metadata().with(
        "sequenceNumbers",
        ValueSet::new().with("sms", 42i64).with("mms", 3i64).with("conversation", 8i64).with("subscription", 1i64),
    );
    let answer = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(
        answer.to_json(),
        json!({
            "correlationVector": {"String": "cv-1"},
            "contentType": {"String": "messaging"},
            "contractVersion": {"Double": 3.99},
            "itemCount": {"Int32": 3},
            "messagingSmsIds": {"Int64Array": [102]},
            "messagingMmsIds": {"Int64Array": []},
            "messagingMmsPartIds": {"Int64Array": []},
            "messagingConversationIds": {"Int64Array": [5]},
            "messagingSubscriptionIds": {"Int64Array": [1]},
            "result": {"Int32": 0}
        })
    );
    // The answer's envelope carries the result header.
    let r = wd.h.messenger.responses().last().cloned().unwrap();
    assert_eq!(r.header("result").as_deref(), Some("0"));

    // Items carrying SMS 102, subscription 1 and thread 5 complete the exchange — when the last
    // pending type other than subscriptions is done.
    wd.w.clock.advance(std::time::Duration::from_secs(1));
    let v = push(
        &wd.h,
        item_msg(ValueSet::new().with("smsMessages", sms_object(&[Sms::inbox(102, 5, "+15550100", "yo")]))),
    )
    .await;
    assert_eq!(v.to_json(), json!({"result": {"Int32": 0}}));

    let v =
        push(&wd.h, item_msg(ValueSet::new().with("subscriptions", subscriptions_object(&[Sub::new(1, "us")])))).await;
    assert_eq!(result_of(&v), Some(0));
    assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS), "still the seed's completion");

    let v = push(&wd.h, item_msg(conversations(&[(5, &["+15550100"])]))).await;
    assert_eq!(result_of(&v), Some(0));
    assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS + 1000), "completed");
    assert_eq!(sms_ids(&wd, 5), vec![101, 102]);

    // The anchors of the metadata are the current ones the next connect sends.
    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    let seq = payload(&wd.h.messenger.requests_on("/connect/v1")[0]).object("sequenceNumbers").unwrap().clone();
    assert_eq!(
        (seq.i64("sms"), seq.i64("mms"), seq.i64("conversation"), seq.i64("subscription")),
        (Some(42), Some(3), Some(8), Some(1))
    );
}

#[tokio::test(start_paused = true)]
async fn forced_resync_requests_every_message_and_rcs_keys_follow_rcs_receive() {
    let wd = world();
    let mut held = Sms::inbox(101, 5, "+15550100", "hi");
    held.checksum = 7;
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "seed").with("smsMessages", sms_object(&[held])))).await;

    // Participants (0x20) and an empty participants table: a forced resync. The settings also report
    // RcsReceive (always forced on), so the answer carries the RCS keys.
    let md = base_metadata().with("syncID", "x").with("settings", ValueSet::new().with("capabilities", 0x20i32));
    let answer = push(&wd.h, metadata_msg(md)).await;

    assert_eq!(answer.i64_array("messagingSmsIds"), Some([101i64, 102].as_slice()));
    assert_eq!(answer.i32("itemCount"), Some(4));
    assert_eq!(answer.i64_array("messagingRcsChatIds"), Some([].as_slice()));
    assert_eq!(answer.i64_array("messagingRcsFileTransferChecksums"), Some([].as_slice()));
    assert_eq!(answer.i64_array("messagingRcsConversationIds"), Some([].as_slice()));
    assert_eq!(
        answer.keys().collect::<Vec<_>>()[10..],
        ["messagingRcsChatIds", "messagingRcsFileTransferChecksums", "messagingRcsConversationIds"]
    );
    assert_eq!(wd.m.predicates().unwrap().capabilities, 0x22);
}

#[tokio::test(start_paused = true)]
async fn forced_resync_without_checksums_marks_unlisted_rows_read() {
    let wd = world();
    // Participants (0x20) reported; the SMS senders below are empty, so no participant is stored.
    report_caps(&wd, 0x20).await;
    let rows = [Sms::inbox(1, 5, "", "x"), Sms::inbox(2, 5, "", "y")];
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "s").with("smsMessages", sms_object(&rows)))).await;

    let md = ValueSet::new()
        .with("syncID", "m")
        .with("messagingSmsIds", vec![1i64, 2])
        .with("messagingUnreadSmsIds", vec![2i64])
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", vec![5i64])
        .with("messagingSubscriptionIds", Vec::<i64>::new());
    let v = push(&wd.h, metadata_msg(md.clone())).await;
    assert_eq!(result_of(&v), Some(0), "forced: SMS checksums not required");
    assert_eq!(v.i64_array("messagingSmsIds"), Some([1i64, 2].as_slice()), "every id requested");

    let read: Vec<(i64, bool)> = wd.m.messages(5).unwrap().iter().map(|m| (m.id, m.read_on_phone)).collect();
    assert_eq!(read, vec![(1, true), (2, false)], "rows not listed unread are read");

    // A phone that does not report Participants: not forced, the checksums are required.
    report_caps(&wd, 0).await;
    let v = push(&wd.h, metadata_msg(md.with("syncID", "m2"))).await;
    assert_eq!(result_of(&v), Some(1));
}

#[tokio::test(start_paused = true)]
async fn delta_deletes_absent_rows_and_an_empty_list_deletes_all() {
    let wd = world();
    let rows = [Sms::inbox(1, 5, "+1", "a"), Sms::inbox(2, 5, "+1", "b"), Sms::inbox(3, 6, "+2", "c")];
    push(
        &wd.h,
        item_msg(
            ValueSet::new()
                .with("syncID", "s")
                .with("smsMessages", sms_object(&rows))
                .with_all(conversations(&[(5, &["+1"]), (6, &["+2"])])),
        ),
    )
    .await;

    assert_eq!(wd.m.conversations().unwrap().len(), 2);

    // SMS 2 is gone on the phone, 3 changed; no MMS key at all (absent ⇒ empty ⇒ delete all MMS).
    let md = ValueSet::new()
        .with("syncID", "m")
        .with("messagingSmsIds", vec![1i64, 3])
        .with("messagingSmsChecksums", vec![10i64, 31])
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", vec![5i64, 6])
        .with("messagingSubscriptionIds", vec![1i64]);
    let v = push(&wd.h, metadata_msg(md)).await;

    assert_eq!(v.i64_array("messagingSmsIds"), Some([3i64].as_slice()));
    // Conversation checksums absent ⇒ every checksum 0: both stored ones (500, 600) differ.
    assert_eq!(v.i64_array("messagingConversationIds"), Some([5i64, 6].as_slice()));
    assert_eq!(sms_ids(&wd, 5), vec![1]);

    // An empty conversation list deletes every conversation.
    let md = base_metadata()
        .with("syncID", "m2")
        .with("messagingConversationIds", Vec::<i64>::new())
        .with("messagingSmsIds", Vec::<i64>::new())
        .with("messagingSmsChecksums", Vec::<i64>::new());
    let v = push(&wd.h, metadata_msg(md)).await;

    assert_eq!(result_of(&v), Some(0));
    assert!(wd.m.conversations().unwrap().is_empty());
    assert!(sms_ids(&wd, 5).is_empty());
}

#[tokio::test(start_paused = true)]
async fn empty_request_completes_at_once_and_stores_the_anchors() {
    let wd = world();
    let md = ValueSet::new()
        .with("syncID", "a")
        .with("messagingSmsChecksums", Vec::<i64>::new())
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("sequenceNumbers", ValueSet::new().with("sms", 5i64).with("mms", -1i64));
    let v = push(&wd.h, metadata_msg(md)).await;

    assert_eq!(v.i32("itemCount"), Some(0));
    assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS));

    // The next connect sends the stored current anchor (and -1 for none).
    let resp = wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    assert!(matches!(resp, phonelink_engine::app::ConnectSendResult::Sent(_)));

    let connect = payload(&wd.h.messenger.requests_on("/connect/v1")[0]);
    let seq = connect.object("sequenceNumbers").unwrap();
    assert_eq!(seq.i64("sms"), Some(5));
    assert_eq!(seq.i64("mms"), Some(-1));
}

#[tokio::test(start_paused = true)]
async fn a_repeated_item_after_completion_without_anchors_deletes_them() {
    let wd = world();
    let md = ValueSet::new()
        .with("messagingSmsIds", vec![1i64])
        .with("messagingSmsChecksums", vec![10i64])
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", Vec::<i64>::new())
        .with("messagingSubscriptionIds", Vec::<i64>::new())
        .with("sequenceNumbers", ValueSet::new().with("sms", 9i64));
    push(&wd.h, metadata_msg(md)).await;

    let item = item_msg(ValueSet::new().with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")])));
    push(&wd.h, item.clone()).await;

    let connect_sms = |wd: &World| {
        let reqs = wd.h.messenger.requests_on("/connect/v1");
        payload(reqs.last().unwrap()).object("sequenceNumbers").unwrap().i64("sms")
    };

    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    assert_eq!(connect_sms(&wd), Some(9));

    // Delivered again: a fresh exchange that completes at once and, without sequenceNumbers,
    // deletes every stored anchor.
    push(&wd.h, item).await;
    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    assert_eq!(connect_sms(&wd), Some(-1));
}

#[tokio::test(start_paused = true)]
async fn exchange_completes_only_when_every_type_but_subscriptions_is_done() {
    let wd = world();
    let md = ValueSet::new()
        .with("messagingSmsIds", vec![1i64])
        .with("messagingSmsChecksums", vec![10i64])
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", vec![5i64])
        .with("messagingSubscriptionIds", vec![1i64])
        .with("sequenceNumbers", ValueSet::new().with("sms", 1i64));
    push(&wd.h, metadata_msg(md)).await;

    // A Delete row also leaves its pending set.
    let sms = sms_object(&[Sms::inbox(1, 5, "+1", "a")]).with("actions", vec![2i32]);
    push(&wd.h, item_msg(ValueSet::new().with("smsMessages", sms))).await;
    assert!(wd.m.last_update_ms().unwrap().is_none());

    // The last item replaces the anchor set.
    let v = push(
        &wd.h,
        item_msg(
            conversations(&[(5, &["+1"])])
                .with("sequenceNumbers", ValueSet::new().with("sms", 2i64).with("conversation", 3i64)),
        ),
    )
    .await;

    assert_eq!(result_of(&v), Some(0));
    assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS), "pending subscriptions are not counted");

    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    let seq = payload(&wd.h.messenger.requests_on("/connect/v1")[0]).object("sequenceNumbers").unwrap().clone();
    assert_eq!((seq.i64("sms"), seq.i64("conversation"), seq.i64("subscription")), (Some(2), Some(3), Some(-1)));
    assert!(sms_ids(&wd, 5).is_empty(), "the Delete row deleted SMS 1");
}

#[tokio::test(start_paused = true)]
async fn item_rows_parts_bytes_and_clamped_dates() {
    let mut wd = world();
    let mut old = Sms::inbox(1, 5, "+1", "old");
    old.date = 253_402_300_800_000; // out of range ⇒ 1601-01-01
    let mms = Mms {
        id: 20,
        thread: 5,
        date: NOW_MS,
        from: "+1".into(),
        to: "+2,,+3".into(),
        mailbox: 1,
        read: false,
        sub: 1,
    };
    let parts = [
        Part { id: 200, message: 20, content_type: "text/plain".into(), text: "caption".into(), bytes: None },
        Part {
            id: 201,
            message: 20,
            content_type: "image/png".into(),
            text: String::new(),
            bytes: Some(vec![1, 2, 3]),
        },
        Part { id: 202, message: 20, content_type: "image/jpeg".into(), text: String::new(), bytes: None },
    ];
    let item = ValueSet::new()
        .with("syncID", "i")
        .with("smsMessages", sms_object(&[old]))
        .with("mmsParts", parts_object(&parts))
        .with("mmsMessages", mms_object(&[mms], true))
        .with_all(conversations(&[(5, &["+1"])]));
    let v = push(&wd.h, item_msg(item)).await;
    assert_eq!(result_of(&v), Some(0));

    let msgs = wd.m.messages(5).unwrap();
    assert_eq!(msgs[0].timestamp_ms, -11_644_473_600_000, "clamped to 1601-01-01");
    let mms_view = msgs.iter().find(|m| m.id == 20).unwrap();
    assert_eq!(mms_view.body, "caption", "text parts form the body");
    assert_eq!(mms_view.parts.len(), 2);
    assert!(mms_view.parts[0].has_bytes, "the view lists the part, its bytes stay in the store");
    assert_eq!(wd.m.part_bytes(mms_view.parts[0].part).unwrap(), Some(vec![1, 2, 3]));
    assert!(!mms_view.parts[1].has_bytes);
    assert_eq!(wd.m.part_bytes(mms_view.parts[1].part).unwrap(), None);
    assert_eq!(mms_view.parts[1].state, phonelink_engine::features::messaging::PartState::Downloadable);

    assert!(
        drain(&mut wd.events)
            .iter()
            .any(|e| matches!(e, phonelink_engine::features::messaging::MessagingEvent::ContentChanged { .. }))
    );

    // A part re-synced without bytes keeps its stored bytes.
    let again = [Part { id: 201, message: 20, content_type: "image/png".into(), text: String::new(), bytes: None }];
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "j").with("mmsParts", parts_object(&again)))).await;

    let mms_view = wd.m.messages(5).unwrap().into_iter().find(|m| m.id == 20).unwrap();
    assert_eq!(wd.m.part_bytes(mms_view.parts[0].part).unwrap(), Some(vec![1, 2, 3]));
}

#[tokio::test(start_paused = true)]
async fn an_mms_subject_shows_as_text_decoded_with_its_charset() {
    let wd = world();
    let subject = "[알림] 이번 주 모임 장소 변경 안내";
    // The phone sends the subject's UTF-8 bytes, each as the character of the same value.
    let sent: String = subject.bytes().map(char::from).collect();

    let mms =
        Mms { id: 20, thread: 5, date: NOW_MS, from: "+1".into(), to: "+2".into(), mailbox: 1, read: false, sub: 1 };
    let mms = mms_object(&[mms], true).with("subjects", vec![sent]).with("subjectCharsets", vec![106i32]);
    let item = ValueSet::new().with("mmsMessages", mms).with_all(conversations(&[(5, &["+1"])]));

    let v = push(&wd.h, item_msg(item)).await;
    assert_eq!(result_of(&v), Some(0));

    let view = wd.m.messages(5).unwrap().into_iter().find(|m| m.id == 20).unwrap();
    assert_eq!(view.subject, subject);
}

#[tokio::test(start_paused = true)]
async fn receive_mms_off_discards_bytes_and_does_not_slice() {
    let wd = world();
    wd.m.set_pc_settings(PcSettings { receive_mms: false, ..PcSettings::default() }).unwrap();

    // contentBytes shorter than claimed: fine while receive MMS is off (nothing is sliced).
    let mut parts = parts_object(&[Part {
        id: 1,
        message: 9,
        content_type: "image/png".into(),
        text: String::new(),
        bytes: Some(vec![1, 2, 3]),
    }]);
    parts.insert("contentBytes", Value::UInt8Array(vec![1]));
    let v = push(&wd.h, item_msg(ValueSet::new().with("mmsParts", parts.clone()))).await;
    assert_eq!(result_of(&v), Some(0));

    // With receive MMS on the same item is malformed.
    wd.m.set_pc_settings(PcSettings::default()).unwrap();
    let v = push(&wd.h, item_msg(ValueSet::new().with("mmsParts", parts))).await;
    assert_eq!(result_of(&v), Some(1));
}

#[tokio::test(start_paused = true)]
async fn rcs_rows_need_rcs_receive_and_file_bytes_skip_deleted_rows() {
    let wd = world();
    // Pairing run: no RCS receive, so RCS parts are not read at all (even malformed ones).
    let bad_rcs = ValueSet::new().with("rcsChats", ValueSet::new().with("itemCount", 1i32));
    let v = push(&wd.h, item_msg(bad_rcs.clone().with("syncID", "a"))).await;
    assert_eq!(result_of(&v), Some(0));
    report_caps(&wd, 0).await; // stored 0x2: RCS receive (contract 3.99)
    let v = push(&wd.h, item_msg(bad_rcs.with("syncID", "b"))).await;
    assert_eq!(result_of(&v), Some(1), "now read and malformed");

    // File 1 is deleted (consumes no bytes), file 2 carries 2 bytes, file 3 none.
    let files = rcs_files_object(
        &[
            (1, 70, "image/png", Some(vec![9, 9, 9])),
            (2, 70, "image/png", Some(vec![4, 5])),
            (3, 70, "text/plain", None),
        ],
        &[2, 0, 0],
    )
    .with("fileByteCounts", vec![3i32, 2, -1])
    .with("fileBytes", Value::UInt8Array(vec![4, 5]));
    let item = ValueSet::new()
        .with("syncID", "c")
        .with_all(rcs_conversations(&[(70, 5, 1, &["x@example.com"])]))
        .with("rcsFileTransfers", files);

    let v = push(&wd.h, item_msg(item)).await;
    assert_eq!(result_of(&v), Some(0));

    let msgs = wd.m.messages(5).unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(wd.m.part_bytes(msgs[0].parts[0].part).unwrap(), Some(vec![4, 5]));
    assert_eq!(wd.m.part_bytes(msgs[1].parts[0].part).unwrap(), None);
    assert_matches!(
        wd.m.part_bytes(phonelink_engine::features::messaging::PartRef::RcsFile(1)),
        Err(phonelink_engine::features::messaging::MessagingError::UnknownPart(1)),
        "the deleted file transfer"
    );

    let conv = wd.m.conversations().unwrap();
    assert_eq!(conv[0].thread_id, 5, "an RCS-only conversation shows under its fallback id");
}

#[tokio::test(start_paused = true)]
async fn rcs_dates_out_of_range_are_malformed_but_sms_dates_clamp() {
    let wd = world();
    report_caps(&wd, 0).await;

    let mut chats = rcs_chats_object(&[(1, 70, "+1", "x")]);
    chats.insert("dates", vec![253_402_300_800_000i64]);
    let v = push(&wd.h, item_msg(ValueSet::new().with("syncID", "r").with("rcsChats", chats))).await;
    assert_eq!(result_of(&v), Some(1));

    // A Delete row's date is never read.
    let mut chats = rcs_chats_object(&[(1, 70, "+1", "x")]);
    chats.insert("dates", vec![253_402_300_800_000i64]);
    chats.insert("actions", vec![2i32]);
    let v = push(&wd.h, item_msg(ValueSet::new().with("syncID", "r2").with("rcsChats", chats))).await;
    assert_eq!(result_of(&v), Some(0));
}

// ------------------------------------------------------------------------------------------------
// Malformed content: the engine's shared rule
// ------------------------------------------------------------------------------------------------

async fn rejected(wd: &World, msg: phonelink_engine::types::TransportMessage) {
    let v = push(&wd.h, msg).await;
    assert_eq!(v.to_json(), json!({"result": {"Int32": 1}}));
}

#[tokio::test(start_paused = true)]
async fn malformed_metadata_is_rejected_and_changes_nothing() {
    let wd = world();
    push(
        &wd.h,
        item_msg(
            ValueSet::new().with("syncID", "seed").with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")])),
        ),
    )
    .await;

    let cases: Vec<ValueSet> = vec![
        // SMS checksums missing outside a forced resync.
        base_metadata().with("messagingSmsChecksums", Value::Int32(0)),
        base_metadata().with("messagingMmsChecksums", Value::Int32(0)),
        // Conversation ids missing while SMS ids are present.
        {
            let mut m = base_metadata();
            m.remove("messagingConversationIds");
            m
        },
        // Subscription ids missing while message ids are present.
        {
            let mut m = base_metadata();
            m.remove("messagingSubscriptionIds");
            m
        },
        // Checksums shorter than ids.
        base_metadata().with("messagingSmsChecksums", vec![7i64]),
        base_metadata().with("messagingConversationChecksums", Vec::<i64>::new()),
        base_metadata().with("messagingSubscriptionChecksums", Vec::<i64>::new()),
        // Wrong array types.
        base_metadata().with("messagingSmsIds", vec![101i32, 102]),
        base_metadata().with("messagingMmsPartIds", vec!["x"]),
        base_metadata().with("messagingUnreadSmsIds", vec![true]),
        // Typed scalars inside the objects.
        base_metadata().with("sequenceNumbers", ValueSet::new().with("sms", 1i32)),
        base_metadata().with("acks", ValueSet::new()),
        base_metadata().with("settings", ValueSet::new().with("capabilities", 1i64)),
        base_metadata().with("settings", ValueSet::new().with("rcsSendAvailable", 1i32)),
        // A correlationVector of another type.
        base_metadata().with("correlationVector", 5i32),
    ];

    for (i, md) in cases.into_iter().enumerate() {
        // The messagingSmsChecksums cases above replace the key with an Int32: a wrong type, also
        // malformed; remove it outright for the "missing" variants.
        let md = if i < 2 {
            let mut m = md;
            m.remove(if i == 0 { "messagingSmsChecksums" } else { "messagingMmsChecksums" });
            m
        } else {
            md
        };

        // Each also carries settings that would change the capabilities if applied.
        let md = if md.contains_key("settings") {
            md
        } else {
            md.with("settings", ValueSet::new().with("capabilities", 0x10i32))
        };

        rejected(&wd, metadata_msg(md.with("syncID", format!("m{i}")))).await;

        assert_eq!(sms_ids(&wd, 5), vec![1], "case {i}: nothing deleted");
        assert_eq!(wd.m.predicates().unwrap().capabilities, 0, "case {i}: settings not applied");
    }
}

#[tokio::test(start_paused = true)]
async fn rcs_conversation_lists_are_required_with_rcs_messages() {
    let wd = world();
    report_caps(&wd, 0).await;

    let md = base_metadata().with("messagingRcsChatIds", vec![1i64]).with("messagingRcsChatChecksums", vec![1i64]);
    rejected(&wd, metadata_msg(md.clone())).await;
    let md = md.with("messagingRcsConversationIds", vec![3i64]);
    rejected(&wd, metadata_msg(md.clone())).await;
    let md = md.with("messagingRcsConversationChecksums", vec![3i64]);
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(v.i64_array("messagingRcsConversationIds"), Some([3i64].as_slice()));

    // RCS chat checksums are required when its ids are non-empty.
    let md = base_metadata()
        .with("syncID", "z")
        .with("messagingRcsChatIds", vec![1i64])
        .with("messagingRcsConversationIds", Vec::<i64>::new())
        .with("messagingRcsConversationChecksums", Vec::<i64>::new());
    rejected(&wd, metadata_msg(md)).await;
}

#[tokio::test(start_paused = true)]
async fn malformed_items_are_rejected_and_discard_the_exchange() {
    let wd = world();
    report_caps(&wd, 0x20).await;

    let cases: Vec<ValueSet> = vec![
        // itemCount missing / of another type.
        ValueSet::new().with("smsMessages", {
            let mut s = sms_object(&[Sms::inbox(1, 5, "+1", "a")]);
            s.remove("itemCount");
            s
        }),
        ValueSet::new().with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]).with("itemCount", 1i64)),
        // Fewer rows than itemCount.
        ValueSet::new().with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]).with("itemCount", 2i32)),
        // A required array missing, an optional one of another type.
        ValueSet::new().with("smsMessages", {
            let mut s = sms_object(&[Sms::inbox(1, 5, "+1", "a")]);
            s.remove("bodies");
            s
        }),
        ValueSet::new().with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]).with("groupIds", vec![1i32])),
        // Conversations without canonicalAddresses.
        ValueSet::new().with("conversations", conversations(&[(5, &["+1"])]).object("conversations").unwrap().clone()),
        // A conversation array shorter than ids.
        {
            let mut c = conversations(&[(5, &["+1"]), (6, &["+2"])]);
            let mut conv = c.object("conversations").unwrap().clone();
            conv.insert("checksums", vec![1i64]);
            c.insert("conversations", conv);
            c
        },
        // canonicalAddresses.addresses shorter than its ids; duplicate ids.
        conversations(&[(5, &["+1"])]).with(
            "canonicalAddresses",
            ValueSet::new().with("ids", vec!["101"]).with("addresses", Vec::<String>::new()),
        ),
        conversations(&[(5, &["+1"])]).with(
            "canonicalAddresses",
            ValueSet::new().with("ids", vec!["101", "101"]).with("addresses", vec!["+1", "+1"]),
        ),
        // MMS rows inserted without toAddresses while the phone reports Participants.
        ValueSet::new().with(
            "mmsMessages",
            mms_object(
                &[Mms {
                    id: 1,
                    thread: 5,
                    date: NOW_MS,
                    from: "+1".into(),
                    to: String::new(),
                    mailbox: 1,
                    read: true,
                    sub: 1,
                }],
                false,
            ),
        ),
        // Conversations: fewer rows (ids) than itemCount.
        {
            let mut c = conversations(&[(5, &["+1"])]);
            let mut conv = c.object("conversations").unwrap().clone();
            conv.insert("itemCount", 2i32);
            c.insert("conversations", conv);
            c
        },
        // acks without ids.
        ValueSet::new().with("acks", ValueSet::new().with("x", 1i32)),
        // An anchor of another type.
        ValueSet::new().with("sequenceNumbers", ValueSet::new().with("conversation", "1")),
    ];

    for (i, case) in cases.into_iter().enumerate() {
        // Open an exchange that waits for SMS 9.
        let md = base_metadata()
            .with("syncID", format!("x{i}"))
            .with("messagingSmsIds", vec![9i64])
            .with("messagingSmsChecksums", vec![9i64]);
        let v = push(&wd.h, metadata_msg(md)).await;
        assert_eq!(result_of(&v), Some(0), "case {i}");

        rejected(&wd, item_msg(case.with("syncID", format!("x{i}")))).await;

        // The exchange is gone: the next item under the key starts an empty one that completes at
        // once (a kept exchange would still wait for SMS 9).
        wd.w.clock.advance(std::time::Duration::from_secs(1));
        push(&wd.h, item_msg(ValueSet::new().with("syncID", format!("x{i}")))).await;
        assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS + 1000 * (i as i64 + 1)), "case {i}");
        assert!(wd.m.conversations().unwrap().is_empty(), "case {i}: nothing applied");
    }
}

#[tokio::test(start_paused = true)]
async fn a_message_without_any_correlation_id_is_rejected() {
    let wd = world();
    let vs = ValueSet::new().with("contentType", "messaging").with("contractVersion", 3.99f64).with("syncID", "k");
    let msg = message(&[("_requestId", "r-9"), ("_route", "/legacy/phonecontent")], valueset::encode(&vs));
    rejected(&wd, msg).await;

    // With the correlation id only in the header it is copied in.
    let msg = message(
        &[("_requestId", "r-9"), ("_route", "/legacy/phonecontent"), ("correlationVector", "h-cv")],
        valueset::encode(&vs),
    );
    let v = push(&wd.h, msg).await;
    assert_eq!(result_of(&v), Some(0));
}

#[tokio::test(start_paused = true)]
async fn sub_objects_of_another_type_are_ignored_and_item_count_zero_skips() {
    let wd = world();
    let v = push(
        &wd.h,
        item_msg(
            ValueSet::new()
                .with("smsMessages", "not an object")
                .with("settings", 5i32)
                .with("conversations", ValueSet::new().with("itemCount", 0i32).with("ids", vec![1i64])),
        ),
    )
    .await;

    assert_eq!(result_of(&v), Some(0));
    assert!(wd.m.conversations().unwrap().is_empty(), "itemCount 0: the sub-object is skipped");
}

// ------------------------------------------------------------------------------------------------
// Participants
// ------------------------------------------------------------------------------------------------

/// The SMS ids a metadata message listing the held SMS (with their stored checksums) requests.
async fn requested_sms(h: &Harness, ids: Vec<i64>) -> Vec<i64> {
    let checksums: Vec<i64> = ids.iter().map(|i| i * 10).collect();
    let md = ValueSet::new()
        .with("syncID", "p")
        .with("messagingSmsIds", ids)
        .with("messagingSmsChecksums", checksums)
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", Vec::<i64>::new())
        .with("messagingSubscriptionIds", Vec::<i64>::new());

    let v = push(h, metadata_msg(md)).await;
    v.i64_array("messagingSmsIds").unwrap().to_vec()
}

#[tokio::test(start_paused = true)]
async fn participants_end_the_forced_resync() {
    let wd = world();
    report_caps(&wd, 0x20).await;

    // A Google RCS sender is never a participant: still forced, a held SMS is requested again.
    push(
        &wd.h,
        item_msg(
            ValueSet::new()
                .with("syncID", "g")
                .with("smsMessages", sms_object(&[Sms::inbox(2, 5, "bot@rcs.google.com", "a")])),
        ),
    )
    .await;

    assert_eq!(requested_sms(&wd.h, vec![2]).await, vec![2], "forced");

    // A sent SMS's recipient is a participant: the resync is no longer forced.
    push(
        &wd.h,
        item_msg(
            ValueSet::new()
                .with("syncID", "s")
                .with("smsMessages", sms_object(&[Sms::sent(3, 6, "+16505550100", "b")])),
        ),
    )
    .await;

    assert_eq!(requested_sms(&wd.h, vec![2, 3]).await, Vec::<i64>::new(), "not forced: nothing is stale");
}

#[tokio::test(start_paused = true)]
async fn participants_from_sms_mms_and_rcs_conversations() {
    let wd = world();
    report_caps(&wd, 0x20 | 0x4).await; // Participants, RcsSend (and RcsReceive)
    let mut own = Sub::new(1, "kr");
    own.number = "010-9999-0000".into();
    let mms = Mms {
        id: 30,
        thread: 7,
        date: NOW_MS,
        from: "010-1111-2222".into(),
        to: "010-9999-0000,010-3333-4444,bot@rcs.google.com".into(),
        mailbox: 1,
        read: true,
        sub: 1,
    };
    let item = ValueSet::new()
        .with("syncID", "i")
        .with("subscriptions", subscriptions_object(&[own]))
        .with("mmsMessages", mms_object(&[mms], true))
        .with_all(conversations(&[(7, &["x@rcs.google.com"])]));
    push(&wd.h, item_msg(item)).await;

    // A Google RCS conversation: a send uses the participants' E.164 addresses, the own number excluded.
    let out =
        wd.m.send_message(
            phonelink_engine::features::messaging::SendTarget::Conversation { thread_id: 7 },
            "hi",
            Vec::new(),
        )
        .await
        .unwrap();

    assert_eq!(out.send_id, 1);

    let send = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(
        send.string_array("recipients"),
        Some(["+821011112222".to_string(), "+821033334444".to_string()].as_slice())
    );

    // An RCS conversation sets the participants under its own id, raw.
    let item = ValueSet::new().with("syncID", "r").with_all(rcs_conversations(&[(
        7,
        7,
        1,
        &["x@rcs.google.com", "y@example.com"],
    )]));
    push(&wd.h, item_msg(item)).await;

    wd.m.send_message(
        phonelink_engine::features::messaging::SendTarget::Conversation { thread_id: 7 },
        "again",
        Vec::new(),
    )
    .await
    .unwrap();

    let send = chat_requests(&wd.h).pop().unwrap();
    assert_eq!(
        send.string_array("recipients"),
        Some(["x@rcs.google.com".to_string(), "y@example.com".to_string()].as_slice()),
        "the participants not among the RCS recipients were dropped"
    );
}

#[tokio::test(start_paused = true)]
async fn data_survives_a_restart_and_the_later_run_defaults_to_rcs_receive() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    {
        let wd = world_with(Harness::new(), location.clone(), MessagingConfig::default());
        assert_eq!(wd.m.predicates().unwrap().capabilities, 0, "the pairing run");

        push(
            &wd.h,
            item_msg(
                ValueSet::new()
                    .with("syncID", "s")
                    .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]))
                    .with_all(conversations(&[(5, &["+1"])])),
            ),
        )
        .await;
    }

    let wd = world_with(Harness::new(), location, MessagingConfig::default());
    assert_eq!(wd.m.predicates().unwrap().capabilities, 0x2, "a later run: RcsReceive at contract 3.99");
    assert_eq!(sms_ids(&wd, 5), vec![1]);
}

#[tokio::test(start_paused = true)]
async fn rcs_lists_are_read_with_rcs_receive_as_it_was_before_the_settings() {
    let wd = world();
    // Pairing run: RCS receive is off until these settings turn it on, so the RCS lists are not read
    // (a wrongly typed one is not even looked at), yet the answer carries the three RCS keys.
    let md = base_metadata()
        .with("messagingRcsChatIds", vec!["not", "read"])
        .with("settings", ValueSet::new().with("capabilities", 0i32));
    let v = push(&wd.h, metadata_msg(md)).await;

    assert_eq!(result_of(&v), Some(0));
    assert_eq!(v.i64_array("messagingRcsChatIds"), Some([].as_slice()));
    assert_eq!(v.i32("itemCount"), Some(4));

    // Now RCS receive holds before the settings: the same list is read and malformed.
    let md = base_metadata().with("syncID", "2").with("messagingRcsChatIds", vec!["not", "read"]);
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(result_of(&v), Some(1));
}

#[tokio::test(start_paused = true)]
async fn rcs_item_rules_file_bytes_and_rcs_conversations() {
    let wd = world();
    report_caps(&wd, 0).await; // RCS receive

    // fileBytes shorter than a non-deleted row claims (receive MMS on): malformed.
    let files = rcs_files_object(&[(1, 70, "image/png", Some(vec![1, 2, 3]))], &[0])
        .with("fileBytes", Value::UInt8Array(vec![1]));
    rejected(&wd, item_msg(ValueSet::new().with("syncID", "f").with("rcsFileTransfers", files.clone()))).await;

    // With receive MMS off the bytes are not read.
    wd.m.set_pc_settings(PcSettings { receive_mms: false, ..PcSettings::default() }).unwrap();
    let v = push(&wd.h, item_msg(ValueSet::new().with("syncID", "f2").with("rcsFileTransfers", files))).await;
    assert_eq!(result_of(&v), Some(0));

    // rcsConversations: canonicalAddresses required, every present array at least as long as ids.
    let conv = rcs_conversations(&[(70, 5, 1, &["+1"])]);
    let only = conv.object("rcsConversations").unwrap().clone();
    rejected(&wd, item_msg(ValueSet::new().with("syncID", "c1").with("rcsConversations", only.clone()))).await;
    let mut short = only.clone();
    short.insert("fallbackIds", Vec::<i64>::new());
    rejected(&wd, item_msg(conv.clone().with("syncID", "c2").with("rcsConversations", short))).await;
    let mut long_ids = only;
    long_ids.insert("itemCount", 2i32);
    rejected(&wd, item_msg(conv.clone().with("syncID", "c3").with("rcsConversations", long_ids))).await;

    let v = push(&wd.h, item_msg(conv.with("syncID", "c4"))).await;
    assert_eq!(result_of(&v), Some(0));
    assert_eq!(wd.m.conversations().unwrap()[0].rcs_type, Some(1));
}

// ------------------------------------------------------------------------------------------------
// RCS receive, forced resync and the rules only some messages exercise
// ------------------------------------------------------------------------------------------------

/// A metadata message with every required list present and empty.
fn empty_metadata() -> ValueSet {
    ValueSet::new()
        .with("messagingSmsIds", Vec::<i64>::new())
        .with("messagingSmsChecksums", Vec::<i64>::new())
        .with("messagingMmsIds", Vec::<i64>::new())
        .with("messagingMmsChecksums", Vec::<i64>::new())
        .with("messagingConversationIds", Vec::<i64>::new())
        .with("messagingSubscriptionIds", Vec::<i64>::new())
}

async fn connect_with(wd: &World, response: ValueSet) {
    wd.h.messenger.answer_all(response_with(response));
    let r = wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    assert!(matches!(r, phonelink_engine::app::ConnectSendResult::Sent(_)), "{r:?}");
}

#[tokio::test(start_paused = true)]
async fn without_rcs_receive_a_metadata_deletes_every_rcs_row() {
    let wd = world();
    report_caps(&wd, 0).await; // RCS receive on
    let item = ValueSet::new()
        .with("syncID", "r")
        .with_all(rcs_conversations(&[(70, 5, 1, &["+1"])]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+1", "x")]))
        .with("rcsFileTransfers", rcs_files_object(&[(2, 70, "image/png", Some(vec![1]))], &[0]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));
    assert_eq!(wd.m.messages(5).unwrap().len(), 2);

    // The phone reports contract 3.8: RCS receive off.
    connect_with(&wd, ValueSet::new().with("result", 0i32).with("contractVersion", 3.8f64)).await;
    assert!(!wd.m.predicates().unwrap().rcs_receive());

    let v = push(&wd.h, metadata_msg(empty_metadata().with("syncID", "m"))).await;
    assert_eq!(result_of(&v), Some(0));
    assert!(v.get("messagingRcsChatIds").is_none(), "no RCS keys without RCS receive");
    assert!(wd.m.messages(5).unwrap().is_empty(), "RCS chats and file transfers deleted");

    // RCS receive back on: the old rows, listed with their stored checksums, are requested again
    // because they were deleted — the RCS conversation too.
    connect_with(&wd, ValueSet::new().with("result", 0i32).with("contractVersion", 3.99f64)).await;

    let md = empty_metadata()
        .with("syncID", "m2")
        .with("messagingRcsChatIds", vec![1i64])
        .with("messagingRcsChatChecksums", vec![0i64])
        .with("messagingRcsFileTransferIds", vec![2i64])
        .with("messagingRcsFileTransferChecksums", vec![0i64])
        .with("messagingRcsConversationIds", vec![70i64])
        .with("messagingRcsConversationChecksums", vec![490i64]);
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(v.i64_array("messagingRcsChatIds"), Some([1i64].as_slice()));
    assert_eq!(v.i64_array("messagingRcsFileTransferChecksums"), Some([2i64].as_slice()));
    assert_eq!(v.i64_array("messagingRcsConversationIds"), Some([70i64].as_slice()));
}

#[tokio::test(start_paused = true)]
async fn rcs_file_transfer_checksums_are_required_for_non_empty_ids() {
    let wd = world();
    report_caps(&wd, 0).await;

    let md = empty_metadata()
        .with("syncID", "m")
        .with("messagingRcsFileTransferIds", vec![1i64])
        .with("messagingRcsConversationIds", Vec::<i64>::new())
        .with("messagingRcsConversationChecksums", Vec::<i64>::new());
    rejected(&wd, metadata_msg(md)).await;
}

#[tokio::test(start_paused = true)]
async fn a_forced_resync_requests_rcs_messages_without_checksums_but_not_rcs_conversations() {
    let wd = world();
    report_caps(&wd, 0x20).await;

    let md = empty_metadata()
        .with("syncID", "m")
        .with("messagingRcsChatIds", vec![1i64, 2])
        .with("messagingRcsFileTransferIds", vec![3i64])
        .with("messagingRcsConversationIds", Vec::<i64>::new())
        .with("messagingRcsConversationChecksums", Vec::<i64>::new());
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(result_of(&v), Some(0));
    assert_eq!(v.i64_array("messagingRcsChatIds"), Some([1i64, 2].as_slice()));
    assert_eq!(v.i64_array("messagingRcsFileTransferChecksums"), Some([3i64].as_slice()));

    let md = empty_metadata()
        .with("syncID", "m2")
        .with("messagingRcsChatIds", vec![1i64])
        .with("messagingRcsConversationIds", vec![3i64]);
    rejected(&wd, metadata_msg(md)).await;
}

#[tokio::test(start_paused = true)]
async fn mms_rows_that_only_delete_need_no_to_addresses() {
    let wd = world();
    report_caps(&wd, 0x20).await;

    let mms =
        Mms { id: 1, thread: 5, date: NOW_MS, from: "+1".into(), to: String::new(), mailbox: 1, read: true, sub: 1 };
    let obj = mms_object(&[mms], false).with("actions", vec![2i32]);
    let v = push(&wd.h, item_msg(ValueSet::new().with("syncID", "d").with("mmsMessages", obj))).await;
    assert_eq!(result_of(&v), Some(0));
}

#[tokio::test(start_paused = true)]
async fn a_subscription_only_request_stays_open_until_an_item() {
    let wd = world();
    let md = empty_metadata()
        .with("syncID", "m")
        .with("messagingSubscriptionIds", vec![1i64])
        .with("sequenceNumbers", ValueSet::new().with("subscription", 4i64));
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(v.i32("itemCount"), Some(1));
    assert!(wd.m.last_update_ms().unwrap().is_none(), "not completed");

    // Any item then completes it (pending subscriptions are not counted) with the metadata's anchors.
    let v = push(&wd.h, item_msg(ValueSet::new().with("syncID", "m"))).await;
    assert_eq!(result_of(&v), Some(0));
    assert!(wd.m.last_update_ms().unwrap().is_some());

    wd.h.app.send_connect(phonelink_engine::app::ConnectKind::Background, "T").await;
    let seq = payload(&wd.h.messenger.requests_on("/connect/v1")[0]).object("sequenceNumbers").unwrap().clone();
    assert_eq!(seq.i64("subscription"), Some(4));
}

#[tokio::test(start_paused = true)]
async fn a_zero_byte_part_needs_no_content_bytes() {
    let wd = world();
    let part = Part { id: 1, message: 9, content_type: "image/png".into(), text: String::new(), bytes: Some(vec![]) };
    let mut parts = parts_object(&[part]);
    parts.remove("contentBytes");
    let v = push(&wd.h, item_msg(ValueSet::new().with("mmsParts", parts))).await;
    assert_eq!(result_of(&v), Some(0));
}

#[tokio::test(start_paused = true)]
async fn a_sync_id_that_is_not_a_string_does_not_key_the_exchange() {
    let wd = world();
    let md = empty_metadata()
        .with("syncID", 7i32)
        .with("messagingSmsIds", vec![9i64])
        .with("messagingSmsChecksums", vec![9i64]);
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(result_of(&v), Some(0));

    // An item with the same correlation id (cv-1) and no syncID continues it: it does not complete.
    let v = push(&wd.h, item_msg(ValueSet::new())).await;
    assert_eq!(result_of(&v), Some(0));
    assert!(wd.m.last_update_ms().unwrap().is_none(), "still waiting for SMS 9");
}

#[tokio::test(start_paused = true)]
async fn a_forced_resync_without_mms_checksums_marks_unlisted_mms_read() {
    let wd = world();
    report_caps(&wd, 0x20).await;

    let mms = |id| Mms {
        id,
        thread: 5,
        date: NOW_MS,
        from: String::new(),
        to: String::new(),
        mailbox: 1,
        read: false,
        sub: 1,
    };
    let item = ValueSet::new().with("syncID", "s").with("mmsMessages", mms_object(&[mms(1), mms(2)], true));
    push(&wd.h, item_msg(item)).await;

    let md = ValueSet::new()
        .with("syncID", "m")
        .with("messagingSmsChecksums", Vec::<i64>::new())
        .with("messagingMmsIds", vec![1i64, 2])
        .with("messagingUnreadMmsIds", vec![2i64])
        .with("messagingConversationIds", vec![5i64])
        .with("messagingSubscriptionIds", Vec::<i64>::new());
    let v = push(&wd.h, metadata_msg(md)).await;
    assert_eq!(result_of(&v), Some(0));

    let read: Vec<(i64, bool)> = wd.m.messages(5).unwrap().iter().map(|m| (m.id, m.read_on_phone)).collect();
    assert_eq!(read, vec![(1, true), (2, false)]);
}

// ------------------------------------------------------------------------------------------------
// Upsert rules
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn replacing_an_sms_keeps_its_known_thread_and_group_ids() {
    let wd = world();
    let with_group = |sms: &[Sms], groups: Vec<i64>| sms_object(sms).with("groupIds", groups);
    let rows = [Sms::inbox(1, 5, "+1", "a"), Sms::inbox(2, -1, "+1", "b"), Sms::inbox(3, 5, "+1", "c")];
    let item = ValueSet::new()
        .with("syncID", "a")
        .with("smsMessages", with_group(&rows, vec![1, -1, 0]))
        .with_all(conversations(&[(5, &["+1"]), (6, &["+1"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    // Re-delivered: SMS 1 and 3 in thread 6, SMS 2 now in thread 6; new group ids.
    let mut moved = [Sms::inbox(1, 6, "+1", "a"), Sms::inbox(2, 6, "+1", "b"), Sms::inbox(3, 6, "+1", "c")];
    for m in &mut moved {
        m.checksum += 1;
    }
    let item = ValueSet::new().with("syncID", "b").with("smsMessages", with_group(&moved, vec![7, -1, 3]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let ids = |t| wd.m.messages(t).unwrap().iter().map(|m| m.id).collect::<Vec<i64>>();

    // A known thread id is kept; a stored -1 takes the new one. A group id other than 0 is kept (SMS 1
    // stays its own group, so it is shown); a stored 0 is replaced (SMS 3 becomes its own group, shown).
    assert_eq!(ids(5), vec![1, 3]);
    assert_eq!(ids(6), vec![2]);
}

#[tokio::test(start_paused = true)]
async fn the_conversation_preview_is_the_latest_message_the_view_shows() {
    let wd = world();
    // Thread 5: an inbox SMS, a later sent SMS, and a later mass-text copy (group 99, not its own id),
    // which the view does not show. Thread 6: an MMS with a caption and an image.
    let mut first = Sms::inbox(1, 5, "+1", "hello");
    first.date = NOW_MS - 3_000;
    let mut reply = Sms::sent(2, 5, "+1", "hi back");
    reply.date = NOW_MS - 2_000;
    let mut copy = Sms::sent(3, 5, "+1", "mass text");
    copy.date = NOW_MS - 1_000;

    let mms =
        Mms { id: 20, thread: 6, date: NOW_MS, from: "+2".into(), to: String::new(), mailbox: 1, read: false, sub: 1 };
    let parts = [
        Part { id: 200, message: 20, content_type: "text/plain".into(), text: "look".into(), bytes: None },
        Part { id: 201, message: 20, content_type: "image/png".into(), text: String::new(), bytes: Some(vec![1]) },
    ];
    let item = ValueSet::new()
        .with("syncID", "p")
        .with("smsMessages", sms_object(&[first, reply, copy]).with("groupIds", vec![-1i64, -1, 99]))
        .with("mmsParts", parts_object(&parts))
        .with("mmsMessages", mms_object(&[mms], true))
        .with_all(conversations(&[(5, &["+1"]), (6, &["+2"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let conv = wd.m.conversations().unwrap();
    let preview = |t: i64| conv.iter().find(|c| c.thread_id == t).unwrap().preview.clone().unwrap();

    let five = preview(5);
    assert_eq!(five.body, "hi back", "the mass-text copy is not the latest shown message");
    assert!(!five.attachment);
    assert_eq!(five.from, None, "a sent message has no sender");

    let six = preview(6);
    assert_eq!(six.body, "look", "an MMS's text parts, as its body");
    assert!(six.attachment, "its image part");
    assert_eq!(six.from.map(|r| r.address), Some("+2".to_string()));
}

// ------------------------------------------------------------------------------------------------
// Scope, SyncAction, and the rules only some messages exercise (audit)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn messages_and_message_ack_have_no_processor() {
    // The iOS Bluetooth MAP content types have no processor — a metadata message is answered 2, a
    // content message 1 (differs from Phone Link on Windows).
    let wd = world();
    for t in ["messages", "messageAck"] {
        let v = push(&wd.h, phone_content(t, ValueSet::new().with("metadata", true))).await;
        assert_eq!(v.to_json(), json!({"result": {"Int32": 2}}), "{t} metadata");

        let v = push(&wd.h, phone_content(t, ValueSet::new())).await;
        assert_eq!(v.to_json(), json!({"result": {"Int32": 1}}), "{t} content");
    }
}

#[tokio::test(start_paused = true)]
async fn an_action_outside_sync_action_is_malformed_in_message_rows() {
    // Item message row rules: in `smsMessages`, `mmsMessages`, `rcsChats` and `rcsFileTransfers` an
    // `actions` value outside `SyncAction` (-1…2) makes the item malformed; `conversations`,
    // `rcsConversations` and `subscriptions` insert or replace.
    let wd = world();
    report_caps(&wd, 0).await; // RCS receive

    let mms =
        Mms { id: 1, thread: 5, date: NOW_MS, from: "+1".into(), to: String::new(), mailbox: 1, read: true, sub: 1 };
    let cases = vec![
        ("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]).with("actions", vec![3i32])),
        ("mmsMessages", mms_object(&[mms], true).with("actions", vec![-2i32])),
        ("rcsChats", rcs_chats_object(&[(1, 70, "+1", "x")]).with("actions", vec![7i32])),
        ("rcsFileTransfers", rcs_files_object(&[(2, 70, "image/png", None)], &[5])),
    ];

    for (i, (part, object)) in cases.into_iter().enumerate() {
        // An exchange that waits for SMS 9; the malformed item discards it.
        let md = base_metadata()
            .with("syncID", format!("a{i}"))
            .with("messagingSmsIds", vec![9i64])
            .with("messagingSmsChecksums", vec![9i64]);
        assert_eq!(result_of(&push(&wd.h, metadata_msg(md)).await), Some(0), "{part}");
        rejected(&wd, item_msg(ValueSet::new().with("syncID", format!("a{i}")).with(part, object))).await;

        wd.w.clock.advance(std::time::Duration::from_secs(1));
        push(&wd.h, item_msg(ValueSet::new().with("syncID", format!("a{i}")))).await;
        assert_eq!(wd.m.last_update_ms().unwrap(), Some(NOW_MS + 1000 * (i as i64 + 1)), "{part}: exchange discarded");
        assert!(sms_ids(&wd, 5).is_empty(), "{part}: nothing applied");
    }

    // A conversation row inserts or replaces on such a value.
    let mut conv = conversations(&[(6, &["+2"])]);
    let mut rows = conv.object("conversations").unwrap().clone();
    rows.insert("actions", vec![5i32]);
    conv.insert("conversations", rows);
    assert_eq!(result_of(&push(&wd.h, item_msg(conv.with("syncID", "c"))).await), Some(0));
    assert!(wd.m.conversations().unwrap().iter().any(|c| c.thread_id == 6));
}

#[tokio::test(start_paused = true)]
async fn neither_the_phone_permission_nor_the_pc_settings_stop_syncing() {
    // No processor checks the phone messages permission, and the PC consent and message
    // notifications change only the UI and notifications — the PC keeps syncing.
    let wd = world();
    connect_with(
        &wd,
        ValueSet::new().with("result", 0i32).with("permissions", ValueSet::new().with("messaging", false)),
    )
    .await;

    assert!(!wd.m.phone_permission().unwrap());

    let item = ValueSet::new()
        .with("syncID", "a")
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+1", "a")]))
        .with_all(conversations(&[(5, &["+1"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));
    assert_eq!(sms_ids(&wd, 5), vec![1]);

    wd.m.set_pc_settings(PcSettings { pc_consent: false, message_notifications: false, ..PcSettings::default() })
        .unwrap();
    let item = ValueSet::new().with("syncID", "b").with("smsMessages", sms_object(&[Sms::inbox(2, 5, "+1", "b")]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));
    assert_eq!(sms_ids(&wd, 5), vec![1, 2]);
}

#[tokio::test(start_paused = true)]
async fn acks_in_a_metadata_message_delete_pending_sends() {
    let wd = world();
    assert!(wd.m.send_text_to("+15550100", "hi").await.unwrap());
    assert_eq!(wd.m.pending_sends().unwrap().len(), 1);

    let md = empty_metadata().with("syncID", "m").with("acks", ValueSet::new().with("ids", vec![1i64]));
    assert_eq!(result_of(&push(&wd.h, metadata_msg(md)).await), Some(0));
    assert!(wd.m.pending_sends().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_metadata_message_without_participants_clears_them() {
    // The delta computation: when the phone does not report `Participants`, a metadata message clears
    // the participants, so a later report of `Participants` forces a resync again.
    let wd = world();
    report_caps(&wd, 0x20).await;
    let sent = sms_object(&[Sms::sent(3, 6, "+16505550100", "b")]);
    push(&wd.h, item_msg(ValueSet::new().with("syncID", "s").with("smsMessages", sent))).await;

    assert_eq!(requested_sms(&wd.h, vec![3]).await, Vec::<i64>::new(), "a participant is stored: not forced");
    report_caps(&wd, 0).await;
    assert_eq!(requested_sms(&wd.h, vec![3]).await, Vec::<i64>::new(), "no Participants: not forced");
    report_caps(&wd, 0x20).await;
    assert_eq!(requested_sms(&wd.h, vec![3]).await, vec![3], "the participants were cleared: forced again");
}

#[tokio::test(start_paused = true)]
async fn replaced_mms_and_rcs_chats_stay_read_on_the_pc() {
    // Item message upsert rules: a replaced MMS or RCS chat message keeps "read on the PC".
    let wd = world();
    let mms =
        Mms { id: 20, thread: 5, date: NOW_MS, from: "+1".into(), to: String::new(), mailbox: 1, read: false, sub: 1 };
    let messages = ValueSet::new()
        .with("settings", ValueSet::new()) // RCS receive
        .with("mmsMessages", mms_object(&[mms], true))
        .with_all(conversations(&[(5, &["+1"])]));
    let chats = ValueSet::new()
        .with_all(rcs_conversations(&[(70, 5, 1, &["+1"])]))
        .with("rcsChats", rcs_chats_object(&[(1, 70, "+1", "x")]));
    push(&wd.h, item_msg(messages.clone().with("syncID", "a"))).await;
    push(&wd.h, item_msg(chats.clone().with("syncID", "b"))).await;

    let conv5 = |wd: &World| wd.m.conversations().unwrap().into_iter().find(|c| c.thread_id == 5).unwrap();
    assert_eq!((conv5(&wd).pc_unread, conv5(&wd).phone_unread), (2, 2));

    // Read on the PC (the phone does not support mark-as-read: only the PC count is cleared).
    wd.m.on_conversation_selected(Some(5));
    wd.m.on_messages_view_changed(true);
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    assert_eq!(conv5(&wd).pc_unread, 0);

    // Delivered again, still unread on the phone.
    push(&wd.h, item_msg(messages.with("syncID", "c"))).await;
    push(&wd.h, item_msg(chats.with("syncID", "d"))).await;
    assert_eq!((conv5(&wd).pc_unread, conv5(&wd).phone_unread), (0, 2));
}

#[tokio::test(start_paused = true)]
async fn only_the_newest_inserted_mms_of_a_thread_gives_participants_in_recipient_order() {
    // Participants: of the MMS rows an item inserts, only the newest of each thread gives
    // participants, ordered by the conversation's recipient list (addresses not in it first) — the
    // order a Google-RCS send lists as `recipients`.
    let wd = world();
    report_caps(&wd, 0x20).await;
    let mms = |id: i64, date: i64, from: &str, to: &str| Mms {
        id,
        thread: 7,
        date,
        from: from.into(),
        to: to.into(),
        mailbox: 1,
        read: true,
        sub: 1,
    };
    let rows =
        [mms(30, NOW_MS - 1000, "+16505550101", ""), mms(31, NOW_MS, "+16505550102", "+16505550103,+16505550104")];
    let item = ValueSet::new()
        .with("syncID", "i")
        .with("subscriptions", subscriptions_object(&[Sub::new(1, "us")]))
        .with("mmsMessages", mms_object(&rows, true))
        .with_all(conversations(&[(7, &["x@rcs.google.com", "+16505550104", "+16505550103"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    wd.m.send_message(
        phonelink_engine::features::messaging::SendTarget::Conversation { thread_id: 7 },
        "hi",
        Vec::new(),
    )
    .await
    .unwrap();

    let send = chat_requests(&wd.h).into_iter().rfind(|r| r.i32("requestType") == Some(2)).unwrap();
    assert_eq!(
        send.string_array("recipients"),
        Some(["+16505550102", "+16505550104", "+16505550103"].map(String::from).as_slice())
    );
}

#[tokio::test(start_paused = true)]
async fn messages_carry_their_mailbox_group_event_and_contact() {
    // What a bubble needs: the phone mailbox of a row the phone has not sent yet or failed to send,
    // the type of an RCS group event, and the contact an address resolves to (for its photo).
    let wd = world();
    wd.contacts.names.lock().unwrap().insert("+1".into(), "Ada".into());

    let rows = [
        Sms { mailbox: 4, ..Sms::sent(1, 5, "+1", "outbox") },
        Sms { mailbox: 5, ..Sms::sent(2, 5, "+1", "failed") },
        Sms { mailbox: 6, ..Sms::sent(3, 5, "+1", "queued") },
        Sms { mailbox: 3, ..Sms::sent(4, 5, "+1", "draft") },
        Sms::inbox(6, 5, "+1", "hello"),
    ];
    let item = ValueSet::new()
        .with("syncID", "m")
        .with("smsMessages", sms_object(&rows))
        .with_all(conversations(&[(5, &["+1"])]));
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let msgs = wd.m.messages(5).unwrap();
    let mailboxes: Vec<(i64, i32)> = msgs.iter().map(|m| (m.id, m.mailbox)).collect();
    assert_eq!(mailboxes, [(1, 4), (2, 5), (3, 6), (4, 3), (6, 1)]);

    let ada = &msgs[4].from;
    assert_eq!((ada.name.as_deref(), ada.contact_id, ada.photo_contact_id), (Some("Ada"), Some(1), None));

    let conv = wd.m.conversations().unwrap();
    assert_eq!(conv[0].recipients[0].contact_id, Some(1));

    // RCS group events keep their `rcsMessageTypes` value; a message has none.
    report_caps(&wd, 0).await;
    let mut chats = rcs_chats_object(&[(1, 70, "+1", ""), (2, 70, "+1", "hi"), (3, 70, "+1", "renamed")]);
    chats.insert("rcsMessageTypes", vec![2i32, 0, 7]);
    let item = ValueSet::new()
        .with("syncID", "r")
        .with_all(rcs_conversations(&[(70, 55, 2, &["+1", "+2"])]))
        .with("rcsChats", chats);
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let events: Vec<(i64, Option<i32>)> = wd.m.messages(55).unwrap().iter().map(|m| (m.id, m.group_event)).collect();
    assert_eq!(events, [(1, Some(2)), (2, None), (3, Some(7))]);
}

#[tokio::test(start_paused = true)]
async fn an_address_whose_contacts_have_no_name_shows_the_address() {
    // The local lookup by address: the first non-empty display name among the address's contacts,
    // else the address — an empty name is no name, in the views and in the notification alike.
    let wc = world_with_contacts();
    let book = ValueSet::new()
        .with("syncID", "book")
        .with(
            "contacts",
            ValueSet::new()
                .with("itemCount", 1i32)
                .with("contactIds", vec![7i64])
                .with("lastUpdatedTimes", vec![1_700_000_000_000i64])
                .with("displayNames", vec![String::new()]),
        )
        .with(
            "phoneNumbers",
            ValueSet::new()
                .with("itemCount", 1i32)
                .with("phoneNumberIds", vec![70i64])
                .with("contactIds", vec![7i64])
                .with("phoneNumbers", vec!["+15550100".to_string()]),
        );
    assert_eq!(result_of(&push(&wc.h, phone_content("contactsV2", book)).await), Some(0));

    let item = ValueSet::new()
        .with("syncID", "m")
        .with("subscriptions", subscriptions_object(&[Sub::new(1, "us")]))
        .with("smsMessages", sms_object(&[Sms::inbox(1, 5, "+15550100", "hello")]))
        .with_all(conversations(&[(5, &["+15550100"])]));
    assert_eq!(result_of(&push(&wc.h, item_msg(item)).await), Some(0));

    let from = wc.m.messages(5).unwrap().remove(0).from;
    assert_eq!((from.name, from.contact_id), (None, Some(7)), "the contact, without a name");
    let listed = wc.m.conversations().unwrap().remove(0).recipients.remove(0);
    assert_eq!((listed.name, listed.contact_id), (None, Some(7)));
    assert_eq!(wc.w.notifier.posted()[0].title, "+15550100", "the notification is titled with the address");
    assert!(wc.contacts.contacts(&phone()).unwrap().iter().all(|c| c.display_name.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_conversation_over_the_recipient_limit_cannot_be_replied_to() {
    // Rule 6: the participant count is limited to `recipientLimits`; a send over it is refused, so
    // the conversation does not offer the composer either.
    let wd = world();
    let mut sub = Sub::new(1, "us");
    sub.recipient_limit = 2;
    let item = ValueSet::new()
        .with("syncID", "l")
        .with("subscriptions", subscriptions_object(&[sub]))
        .with_all(conversations(&[(5, &["+1", "+2"]), (6, &["+1", "+2", "+3"])]))
        .with(
            "smsMessages",
            sms_object(&[Sms::inbox(1, 5, "+1", "two"), Sms { date: NOW_MS - 1, ..Sms::inbox(2, 6, "+1", "three") }]),
        );
    assert_eq!(result_of(&push(&wd.h, item_msg(item)).await), Some(0));

    let reply: Vec<(i64, bool)> = wd.m.conversations().unwrap().iter().map(|c| (c.thread_id, c.can_reply)).collect();
    assert_eq!(reply, [(5, true), (6, false)]);
}
