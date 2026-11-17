use std::path::PathBuf;

use phonelink_engine::api::contacts::{ContactsAccess, ContactsState};
use phonelink_engine::api::messaging::{
    Conversation, Message, MessageContact, MessageMailbox, MessageType, MessagingSettings, MessagingState,
    NewMessageLimits,
};
use serde_json::Value;

use super::arguments::{Compose, DroppedImage, Flag, ListItem, parse};
use super::draft::{Draft, DraftImage};
use super::draft_images::image_paths;
use super::state::State;
use super::view_json::Wanted;
use crate::messaging_edges::file_types;
use crate::views::messages::{ComposerTarget, Detail, Recipient, UnsentGroup};

fn image(id: u64) -> DraftImage {
    DraftImage { id, name: String::new(), content_type: None, bytes: vec![1], url: String::new() }
}

#[test]
fn drafts_join_text_on_lines_and_keep_at_most_two_images() {
    let first = Draft { text: "typed".to_owned(), images: vec![image(1), image(2)] };
    let then = Draft { text: "kept".to_owned(), images: vec![image(3)] };
    let joined = first.joined(Some(then));
    assert_eq!(joined.text, "typed\nkept");
    assert_eq!(joined.images.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
    let alone = Draft { text: String::new(), images: vec![image(4)] }.joined(Some(Draft::default()));
    assert_eq!(alone.images.len(), 1);
    assert!(!Draft { text: "  \n".to_owned(), images: Vec::new() }.has_content());
}

#[test]
fn the_windows_arguments_are_read_field_by_field() {
    let item = |arg| parse("messages.select", arg, ListItem::read);
    assert_eq!(item(r#"{"kind":"conversation","threadId":5}"#), Some(Some(ListItem::Conversation(5))));
    assert_eq!(item(r#"{"kind":"newMessage"}"#), Some(Some(ListItem::NewMessage)));
    assert_eq!(item(r#"{"kind":"unsent","addresses":["1"]}"#), Some(Some(ListItem::Unsent(vec!["1".to_owned()]))));
    assert_eq!(item("null"), Some(None));
    assert_eq!(item(r#"{"kind":"photo"}"#), None);
    assert_eq!(item(r#"{"kind":"conversation","threadId":"5"}"#), None);
    assert_eq!(item("{"), None);

    let compose = parse("messages.compose", r#"{"addresses":["+8210"]}"#, Compose::read).expect("compose reads");
    assert_eq!((compose.addresses, compose.text), (vec!["+8210".to_owned()], String::new()));
    assert!(parse("messages.compose", r#"{"addresses":"+8210"}"#, Compose::read).is_none());
    let data = parse("messages.addImageData", r#"{"data":"AA==","type":null}"#, DroppedImage::read).expect("reads");
    assert_eq!(data.content_type, None);
    assert!(parse("messages.setMuted", r#"{"threadId":3,"on":true}"#, Flag::read).is_some_and(|f| f.on));
}

#[test]
fn copied_image_files_are_read_from_a_uri_list() {
    let list = b"# copied\r\nfile:///tmp/a%20b.png\r\nfile:///tmp/notes.txt\r\nhttps://example.com/c.png\r\n";
    // The shared MIME database decides which names are images.
    if file_types().database().type_of_file_name("a.png").is_none() {
        eprintln!("skipped: no shared MIME database is installed to tell image names");
        return;
    }
    assert_eq!(image_paths(list), [PathBuf::from("/tmp/a b.png")]);
}

#[test]
fn the_new_message_counts_typed_text_as_content() {
    let mut state = State { detail: Some(Detail::NewMessage), ..State::default() };
    assert!(!state.new_message_has_content());
    state.recipient_query = "01".to_owned();
    assert!(state.new_message_has_content());
    assert!(state.shows_new_message_row());
    state.recipient_query.clear();
    state.detail = None;
    assert!(!state.shows_new_message_row());
    state.drafts.insert(ComposerTarget::NewMessage, Draft { text: "hi".to_owned(), images: Vec::new() });
    assert!(state.shows_new_message_row());
}

#[test]
fn a_new_message_submits_with_a_recipient_or_a_typed_address() {
    let mut state = State { detail: Some(Detail::NewMessage), ..State::default() };
    state.drafts.insert(ComposerTarget::NewMessage, Draft { text: "hello".to_owned(), images: Vec::new() });
    assert!(!state.can_submit());
    state.recipient_query = "010-1234-5678".to_owned();
    assert!(state.can_submit());
    state.recipient_query.clear();
    state.new_recipients.push(Recipient { address: "+8210".to_owned(), name: None });
    assert!(state.can_submit());
    state.drafts.insert(ComposerTarget::NewMessage, Draft { text: " ".to_owned(), images: Vec::new() });
    assert!(!state.can_submit());
}

fn contact(address: &str, name: Option<&str>) -> MessageContact {
    MessageContact {
        address: address.to_owned(),
        name: name.map(str::to_owned),
        contact_id: None,
        photo_contact_id: None,
    }
}

fn conversation(thread_id: i64, recipients: &[&str]) -> Conversation {
    Conversation {
        thread_id,
        recipients: recipients.iter().map(|a| contact(a, None)).collect(),
        is_group: recipients.len() > 1,
        latest_ms: thread_id,
        preview: None,
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

fn sent(kind: MessageType, id: i64, to: &str) -> Message {
    Message {
        kind,
        id,
        timestamp_ms: id,
        incoming: false,
        mailbox: MessageMailbox::Sent,
        contact: contact(to, None),
        body: format!("message {id}"),
        subject: String::new(),
        read: true,
        subscription_id: -1,
        group_event: None,
        parts: Vec::new(),
        send_state: None,
    }
}

fn messaging(pc_consent: bool, phone_permission: bool) -> MessagingState {
    MessagingState {
        phone_permission,
        settings: MessagingSettings {
            receive_mms: true,
            auto_download_mms: true,
            send_mms: true,
            pc_consent,
            message_notifications: true,
        },
        up_to_date: true,
        syncing: false,
        last_updated_ms: None,
        setup_completed_once: true,
        proximity: None,
    }
}

#[test]
fn the_list_puts_hidden_conversations_apart_and_unsent_ones_without_a_conversation_first() {
    let mut hidden = conversation(2, &["010-2222-3333"]);
    hidden.hidden = true;
    let unsent = |to: &str, id| UnsentGroup {
        recipients: vec![contact(to, None)],
        messages: vec![sent(MessageType::PendingSend, id, to)],
    };
    let state = State {
        state: Some(messaging(true, true)),
        conversations: vec![conversation(1, &["010-1111-2222"]), hidden],
        unsent: vec![unsent("+82 10-1111-2222", 7), unsent("010-9999-8888", 8)],
        detail: Some(Detail::Conversation(1)),
        ..State::default()
    };
    let list = state.list_json(&mut Wanted::default());
    assert_eq!(list["status"], "conversations");
    assert_eq!(list["conversations"].as_array().map(Vec::len), Some(1));
    assert_eq!(list["hidden"][0]["threadId"], 2);
    // The first unsent group's recipient has a conversation: its messages show there instead.
    assert_eq!(list["unsent"].as_array().map(Vec::len), Some(1));
    assert_eq!(list["unsent"][0]["addresses"][0], "010-9999-8888");
    assert_eq!(list["selection"], "conversation:1");
    assert_eq!(list["newMessage"], Value::Null);
}

#[test]
fn the_gate_stands_in_for_the_list() {
    let mut state = State { state: Some(messaging(false, true)), ..State::default() };
    assert_eq!(state.list_status(), "turnedOff");
    state.state = Some(messaging(true, false));
    assert_eq!(state.list_status(), "permissionNeeded");
    state.state = Some(MessagingState { syncing: true, ..messaging(true, true) });
    assert_eq!(state.list_status(), "syncing");
    state.conversations.push(conversation(1, &["1"]));
    assert_eq!(state.list_status(), "conversations");
}

#[test]
fn a_conversations_transcript_ends_with_its_sends_in_the_order_they_were_sent() {
    let state = State {
        conversations: vec![conversation(1, &["010-1111-2222"])],
        transcript: vec![
            sent(MessageType::Sms, 10, "010-1111-2222"),
            sent(MessageType::PendingSend, 30, "010-1111-2222"),
            sent(MessageType::Mms, 11, "010-1111-2222"),
        ],
        unsent: vec![UnsentGroup {
            recipients: vec![contact("+82 10-1111-2222", None)],
            messages: vec![sent(MessageType::PendingSend, 20, "+82 10-1111-2222")],
        }],
        detail: Some(Detail::Conversation(1)),
        ..State::default()
    };
    let (messages, group) = state.shown_messages();
    assert_eq!(messages.iter().map(|m| m.id).collect::<Vec<_>>(), [10, 11, 20, 30]);
    assert!(!group);
}

#[test]
fn the_composer_follows_the_conversation() {
    let mut quiet = conversation(1, &["1588-0000"]);
    quiet.can_reply = false;
    let mut state = State { conversations: vec![quiet], detail: Some(Detail::Conversation(1)), ..State::default() };
    let composer = state.composer();
    assert!(!composer.enabled);
    assert_eq!(composer.placeholder, "You can't reply to this conversation");
    assert!(!state.can_add_images());

    state.conversations[0].can_reply = true;
    state.conversations[0].attachments_allowed = false;
    assert!(state.composer().enabled);
    assert!(!state.can_add_images());
    state.conversations[0].attachments_allowed = true;
    assert!(state.can_add_images());
    let image = DraftImage { id: 1, name: String::new(), content_type: None, bytes: vec![1], url: String::new() };
    state
        .drafts
        .insert(ComposerTarget::Conversation(1), Draft { text: String::new(), images: vec![image.clone(), image] });
    assert!(!state.can_add_images());
    assert!(state.can_submit());
    assert_eq!(state.detail_json(&mut Wanted::default())["composer"]["canAddImages"], false);
}

#[test]
fn the_contacts_notice_offers_what_lets_the_new_message_search_them() {
    let contacts =
        |capable, access| ContactsState { capable, pc_consent: true, access, up_to_date: true, last_updated_ms: None };
    let mut state = State { detail: Some(Detail::NewMessage), ..State::default() };
    assert_eq!(state.contacts_notice(), None);
    state.contacts = Some(contacts(true, ContactsAccess::PcConsentRequired));
    assert_eq!(state.contacts_notice(), Some("turnOn"));
    state.contacts = Some(contacts(true, ContactsAccess::DeviceConsentDenied));
    assert_eq!(state.contacts_notice(), Some("allowOnPhone"));
    assert_eq!(state.detail_json(&mut Wanted::default())["contactsNotice"], "allowOnPhone");
    state.contacts = Some(contacts(false, ContactsAccess::DeviceConsentRequired));
    assert_eq!(state.contacts_notice(), None);
    state.contacts = Some(contacts(true, ContactsAccess::Granted));
    assert_eq!(state.contacts_notice(), None);
}

#[test]
fn the_sims_limits_cap_recipients_and_lock_the_composer() {
    let limits = |group_allowed, max_recipients| NewMessageLimits {
        attachments_allowed: true,
        group_allowed,
        max_text_length: Some(10),
        max_recipients,
        active_subscription: -1,
    };
    let mut state = State { detail: Some(Detail::NewMessage), limits: Some(limits(false, None)), ..State::default() };
    assert_eq!(state.recipient_cap(), Some(1));
    assert!(state.can_add_recipient());
    state.new_recipients.push(Recipient { address: "1".to_owned(), name: None });
    assert!(!state.can_add_recipient());
    state.new_recipients.push(Recipient { address: "2".to_owned(), name: None });
    let composer = state.composer();
    assert!(!composer.enabled);
    assert_eq!(composer.placeholder, "This SIM sends to one person at a time");
    state.limits = Some(limits(true, Some(5)));
    assert_eq!(state.recipient_cap(), Some(5));
    assert_eq!(state.composer().max_text_length, Some(10));
}
