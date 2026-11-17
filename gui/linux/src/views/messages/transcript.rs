//! A conversation's transcript: where the day and time and the sender's name show, and each row as the
//! window reads it.

use phonelink_engine::api::messaging::{Message, MessageMailbox, MessageSendState};
use serde_json::{Value, json};

use super::contact_name;
use super::keys::message_key;
use super::texts::{group_event_text, not_delivered, phone_status};
use crate::l10n::translated;

/// One message of a transcript with what shows around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptRow<'a> {
    pub message: &'a Message,
    /// The day and time above it: before the first message and after a gap of over an hour.
    pub separator: bool,
    /// In a group, the sender's name above a received message that starts a run of theirs.
    pub sender: Option<String>,
}

/// The transcript of `messages` (oldest first, as the engine lists them).
pub fn transcript_rows(messages: &[Message], is_group: bool) -> Vec<TranscriptRow<'_>> {
    let mut rows = Vec::with_capacity(messages.len());
    let mut previous: Option<&Message> = None;
    for message in messages {
        let separator = previous.is_none_or(|p| message.timestamp_ms - p.timestamp_ms > 3_600_000);
        let continues_run = !separator
            && previous
                .is_some_and(|p| p.incoming && p.group_event.is_none() && p.contact.address == message.contact.address);
        let shows_sender = is_group && message.incoming && message.group_event.is_none() && !continues_run;
        rows.push(TranscriptRow { message, separator, sender: shows_sender.then(|| contact_name(&message.contact)) });
        previous = Some(message);
    }
    rows
}

/// A transcript row as the window reads it, with its parts already shaped.
pub fn transcript_row_json(row: &TranscriptRow<'_>, parts: Vec<Value>, retrying: bool) -> Value {
    let message = row.message;
    let (send_state, status, status_error) = match message.send_state {
        Some(MessageSendState::Sending) => ("sending", Some(translated("Sending...")), false),
        Some(MessageSendState::Failed { reason }) => ("failed", Some(not_delivered(reason)), true),
        Some(MessageSendState::Sent) => ("sent", None, false),
        None => ("", phone_status(message), message.mailbox == MessageMailbox::Failed),
    };

    json!({
        "key": message_key(message),
        "separator": row.separator,
        "timestampMs": message.timestamp_ms,
        "sender": row.sender,
        "event": group_event_text(message),
        "incoming": message.incoming,
        "subject": message.subject,
        "body": message.body,
        "parts": parts,
        "sendState": if send_state.is_empty() { Value::Null } else { Value::from(send_state) },
        "status": status,
        "statusError": status_error,
        "sendId": matches!(message.send_state, Some(MessageSendState::Failed { .. })).then_some(message.id),
        "retrying": retrying,
    })
}
