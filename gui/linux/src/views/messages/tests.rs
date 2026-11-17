use phonelink_engine::api::contacts::{ContactSuggestion, PhoneNumberKind};
use phonelink_engine::api::messaging::{
    Conversation, ConversationPreviewLine, Message, MessageContact, MessageGroupEvent, MessageMailbox, MessagePart,
    MessagePartRef, MessagePartState, MessageSendState, MessageType, NewConversationMessage, SendFailure,
    SimSubscription,
};

use super::texts::{group_event_text, not_delivered, phone_status};
use super::*;

fn contact(address: &str, name: Option<&str>) -> MessageContact {
    MessageContact {
        address: address.to_owned(),
        name: name.map(str::to_owned),
        contact_id: None,
        photo_contact_id: None,
    }
}

fn message(id: i64, timestamp_ms: i64, incoming: bool, from: &str) -> Message {
    Message {
        kind: MessageType::Sms,
        id,
        timestamp_ms,
        incoming,
        mailbox: if incoming { MessageMailbox::Inbox } else { MessageMailbox::Sent },
        contact: contact(from, None),
        body: format!("message {id}"),
        subject: String::new(),
        read: true,
        subscription_id: -1,
        group_event: None,
        parts: Vec::new(),
        send_state: None,
    }
}

fn conversation(recipients: Vec<MessageContact>, preview: Option<ConversationPreviewLine>) -> Conversation {
    Conversation {
        thread_id: 7,
        is_group: recipients.len() > 1,
        recipients,
        latest_ms: 0,
        preview,
        unread: 0,
        muted: false,
        hidden: false,
        can_reply: true,
        attachments_allowed: true,
        active_subscription: -1,
        max_text_length: None,
        max_recipients: None,
    }
}

#[test]
fn contact_name_falls_back_to_the_address() {
    assert_eq!(contact_name(&contact("+8210", Some("Kim"))), "Kim");
    assert_eq!(contact_name(&contact("+8210", Some(" "))), "+8210");
    assert_eq!(contact_name(&contact("+8210", None)), "+8210");
}

#[test]
fn plausible_addresses_are_numbers_or_email_addresses() {
    assert!(address::is_plausible("010-1234-5678"));
    assert!(address::is_plausible(" +82 (10) 1234 5678 "));
    assert!(address::is_plausible("112"));
    assert!(!address::is_plausible("12"));
    assert!(!address::is_plausible("Kim"));
    assert!(address::is_plausible("mina@example.com"));
    assert!(!address::is_plausible("mina@example"));
    assert!(!address::is_plausible("@example.com"));
    assert!(!address::is_plausible("mi na@example.com"));
}

#[test]
fn numbers_match_across_national_and_international_forms() {
    assert!(address::same("010-2345-6789", "+82 10-2345-6789"));
    assert!(address::same("+821023456789", "01023456789"));
    assert!(!address::same("1588-0000", "+82 1588-0000-1"));
    assert!(!address::same("123456", "0123456"));
    assert!(address::same("123456", "123456"));
    assert!(address::same("Mina@Example.com", "mina@example.com"));
    assert!(!address::same("mina@example.com", "minb@example.com"));
}

#[test]
fn contacts_are_searched_by_a_numbers_last_digits() {
    assert_eq!(address::contact_search("+82 10-2345-6789").as_deref(), Some("23456789"));
    assert_eq!(address::contact_search("112").as_deref(), Some("112"));
    assert_eq!(address::contact_search("mina@example.com"), None);
    assert_eq!(address::contact_search("Mina"), None);
}

#[test]
fn recipient_lists_match_in_any_order_one_to_one() {
    let a = vec!["010-1111-2222".to_owned(), "010-3333-4444".to_owned()];
    let b = vec!["+82 10-3333-4444".to_owned(), "+82 10-1111-2222".to_owned()];
    assert!(address::same_recipients(&a, &b));
    assert!(!address::same_recipients(&a, &b[..1]));
    let twice = vec!["010-1111-2222".to_owned(), "010-1111-2222".to_owned()];
    assert!(!address::same_recipients(&a, &twice));
}

#[test]
fn unsent_messages_group_by_recipients_newest_group_first() {
    let sent = |id, at, to: &[&str]| NewConversationMessage {
        recipients: to.iter().map(|a| contact(a, None)).collect(),
        message: message(id, at, false, to[0]),
    };
    let groups = unsent_groups(&[
        sent(1, 100, &["010-1111-2222"]),
        sent(2, 200, &["010-3333-4444"]),
        sent(3, 300, &["+82 10-1111-2222"]),
    ]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].messages.iter().map(|m| m.id).collect::<Vec<_>>(), [1, 3]);
    assert_eq!(groups[0].addresses(), ["010-1111-2222"]);
    assert_eq!(groups[1].messages[0].id, 2);
}

#[test]
fn text_is_clamped_by_utf16_units_at_character_boundaries() {
    assert_eq!(clamp_utf16("hello", Some(3)), "hel");
    assert_eq!(clamp_utf16("a😀b", Some(2)), "a");
    assert_eq!(clamp_utf16("a😀b", Some(3)), "a😀");
    assert_eq!(clamp_utf16("한글", None), "한글");
}

#[test]
fn titles_previews_and_subtitles() {
    let one = conversation(vec![contact("+8210", Some("Mina"))], None);
    assert_eq!(conversation_title(&one), "Mina");
    assert_eq!(conversation_subtitle(&one).as_deref(), Some("+8210"));
    let unnamed = conversation(vec![contact("+8210", None)], None);
    assert_eq!(conversation_subtitle(&unnamed), None);
    assert_eq!(conversation_title(&conversation(Vec::new(), None)), "Unknown");

    let preview = |body: &str, attachment, from: Option<MessageContact>| ConversationPreviewLine {
        body: body.to_owned(),
        attachment,
        from,
    };
    let group = conversation(
        vec![contact("1", Some("A")), contact("2", None)],
        Some(preview("hi\nthere", false, Some(contact("1", Some("A"))))),
    );
    assert_eq!(conversation_title(&group), "A, 2");
    assert_eq!(conversation_preview(&group), "A: hi there");
    let picture = conversation(vec![contact("1", None)], Some(preview("", true, None)));
    assert_eq!(conversation_preview(&picture), "Attachment");
}

#[test]
fn new_message_previews_name_recipients_else_the_text() {
    let to = [Recipient { address: "+8210".to_owned(), name: Some("Mina".to_owned()) }];
    assert_eq!(new_message_preview(&to, "hi"), "To: Mina");
    assert_eq!(new_message_preview(&[], "line one\nline two"), "line one line two");
}

#[test]
fn transcript_rows_mark_separators_and_group_senders() {
    let hour = 3_600_000;
    let mut event = message(4, 3 * hour, true, "a");
    event.group_event = Some(MessageGroupEvent::Joined);
    event.body = String::new();
    let messages = vec![
        message(1, 0, true, "a"),
        message(2, 1000, true, "a"),
        message(3, 2000, true, "b"),
        event,
        message(5, 3 * hour + 1000, true, "a"),
        message(6, 5 * hour, false, "a"),
    ];
    let rows = transcript_rows(&messages, true);
    let separators: Vec<bool> = rows.iter().map(|r| r.separator).collect();
    assert_eq!(separators, [true, false, false, true, false, true]);
    let senders: Vec<Option<&str>> = rows.iter().map(|r| r.sender.as_deref()).collect();
    assert_eq!(senders, [Some("a"), None, Some("b"), None, Some("a"), None]);
    assert_eq!(group_event_text(rows[3].message).as_deref(), Some("a joined the conversation"));
    assert!(transcript_rows(&messages, false).iter().all(|r| r.sender.is_none()));
}

#[test]
fn send_states_and_phone_mailboxes_read_as_statuses() {
    let mut failed = message(9, 0, false, "a");
    failed.kind = MessageType::PendingSend;
    failed.send_state = Some(MessageSendState::Failed { reason: SendFailure::NoService });
    let row = TranscriptRow { message: &failed, separator: false, sender: None };
    let json = transcript_row_json(&row, Vec::new(), false);
    assert_eq!(json["status"], "Not Delivered · No service");
    assert_eq!(json["statusError"], true);
    assert_eq!(json["sendId"], 9);
    assert_eq!(json["key"], "send-9");

    let mut draft = message(10, 0, false, "a");
    draft.mailbox = MessageMailbox::Draft;
    assert_eq!(phone_status(&draft).as_deref(), Some("Draft"));
    assert_eq!(not_delivered(SendFailure::Other { code: 99 }), "Not Delivered");
}

#[test]
fn part_keys_round_trip() {
    for part in [
        MessagePartRef::MmsPart { id: 12 },
        MessagePartRef::RcsFile { id: -3 },
        MessagePartRef::SentPart { send_id: 5, index: 1 },
    ] {
        assert_eq!(parse_part_key(&part_key(part)), Some(part));
    }
    assert_eq!(parse_part_key("mms-x"), None);
    assert_eq!(parse_part_key("photo-1"), None);
    assert_eq!(parse_part_key("sent-5"), None);
}

#[test]
fn part_file_names_drop_folders_and_gain_an_extension() {
    assert_eq!(part_file_name("IMG_1.jpg", Some(".jpeg")), "IMG_1.jpg");
    assert_eq!(part_file_name("../etc/passwd", None), "passwd");
    assert_eq!(part_file_name("voice", Some(".amr")), "voice.amr");
    assert_eq!(part_file_name("", Some(".vcard")), "Attachment.vcard");
    assert_eq!(part_file_name("..", None), "Attachment");
    assert_eq!(part_file_name(".hidden", Some(".txt")), ".hidden.txt");
}

#[test]
fn search_matches_names_addresses_and_previews() {
    let line = ConversationPreviewLine { body: "See you at Gangnam".to_owned(), attachment: false, from: None };
    let c = conversation(vec![contact("+82 10-1234", Some("Kim Mina"))], Some(line));
    assert!(conversation_matches(&c, ""));
    assert!(conversation_matches(&c, "mina"));
    assert!(conversation_matches(&c, "10-12"));
    assert!(conversation_matches(&c, " GANGNAM "));
    assert!(!conversation_matches(&c, "seoul"));
}

#[test]
fn sims_and_suggestions_read_as_lines() {
    let sim =
        SimSubscription { id: 1, sim_slot_index: 1, name: String::new(), number: String::new(), is_default_sms: false };
    assert_eq!(sim_name(&sim), "SIM 2");
    let suggestion = ContactSuggestion {
        contact_id: 1,
        name: "Mina".to_owned(),
        number: "010-1234-5678".to_owned(),
        kind: PhoneNumberKind::Custom,
        label: "Studio".to_owned(),
        has_thumbnail: false,
    };
    assert_eq!(number_line(&suggestion), "Studio · 010-1234-5678");
    assert_eq!(number_line(&ContactSuggestion { kind: PhoneNumberKind::Unknown, ..suggestion }), "010-1234-5678");
    assert_eq!(cap_notice(1), "This SIM sends to one person at a time");
    assert_eq!(cap_notice(10), "This SIM sends to at most 10 people");
}

#[test]
fn unsent_previews_follow_the_newest_send() {
    let mut sending = message(1, 0, false, "a");
    sending.send_state = Some(MessageSendState::Sending);
    let mut group = UnsentGroup { recipients: vec![contact("a", Some("Mina"))], messages: vec![sending] };
    assert_eq!(unsent_title(&group), "To: Mina");
    assert_eq!(unsent_preview(&group), "Sending...");
    let mut sent = message(2, 1, false, "a");
    sent.body = String::new();
    sent.parts = vec![MessagePart {
        part: MessagePartRef::SentPart { send_id: 2, index: 0 },
        content_type: "image/png".to_owned(),
        name: String::new(),
        size: 10,
        state: MessagePartState::Image,
        has_bytes: true,
    }];
    sent.send_state = Some(MessageSendState::Sent);
    group.messages.push(sent);
    assert_eq!(unsent_preview(&group), "Attachment");
}
