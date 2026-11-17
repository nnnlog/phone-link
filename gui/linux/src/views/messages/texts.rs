//! The Messages section's texts: names and titles, the list's previews, group events, how a send went,
//! and the lines of SIMs and contact suggestions.

use phonelink_engine::api::contacts::{ContactSuggestion, PhoneNumberKind};
use phonelink_engine::api::messaging::{
    Conversation, Message, MessageContact, MessageGroupEvent, MessageMailbox, MessageSendState, SendFailure,
    SimSubscription,
};
use phonelink_messaging::ProximityState;

use super::{Recipient, UnsentGroup};
use crate::l10n::{fill_plural, filled, translated};

/// A person: their contact name, else their address.
pub fn contact_name(contact: &MessageContact) -> String {
    contact.name.clone().filter(|name| !name.trim().is_empty()).unwrap_or_else(|| contact.address.clone())
}

/// A conversation's title: its recipients' names (else addresses) joined with ", ".
pub fn conversation_title(conversation: &Conversation) -> String {
    if conversation.recipients.is_empty() {
        return translated("Unknown");
    }
    conversation.recipients.iter().map(contact_name).collect::<Vec<_>>().join(", ")
}

/// The line under a conversation's title: the number of a one-person conversation named by a contact.
pub fn conversation_subtitle(conversation: &Conversation) -> Option<String> {
    let [only] = conversation.recipients.as_slice() else { return None };
    only.name.as_ref().filter(|name| !name.trim().is_empty()).map(|_| only.address.clone())
}

/// Text on one line; "Attachment" when there is none but an attachment.
fn line(text: &str, attachment: bool) -> String {
    if text.is_empty() && attachment { translated("Attachment") } else { text.replace('\n', " ") }
}

/// The list's preview of a conversation's latest message: its text on one line, "Attachment" for one
/// with attachments only, and in a group the sender's name before a received one.
pub fn conversation_preview(conversation: &Conversation) -> String {
    let Some(latest) = &conversation.preview else { return String::new() };
    let text = line(&latest.body, latest.attachment);
    match &latest.from {
        Some(sender) if conversation.is_group => format!("{}: {text}", contact_name(sender)),
        _ => text,
    }
}

/// The title of messages sent to new recipients: "To: " and their names.
pub fn unsent_title(group: &UnsentGroup) -> String {
    let names: Vec<String> = group.recipients.iter().map(contact_name).collect();
    filled("To: %@", &[names.join(", ")])
}

/// The list's line under it: how the newest one's send went, or its text once the phone took it.
pub fn unsent_preview(group: &UnsentGroup) -> String {
    let Some(latest) = group.latest() else { return String::new() };
    match latest.send_state {
        Some(MessageSendState::Failed { reason }) => not_delivered(reason),
        Some(MessageSendState::Sending) => translated("Sending..."),
        Some(MessageSendState::Sent) | None => line(&latest.body, !latest.parts.is_empty()),
    }
}

/// The list's line under the new message being written: its recipients, else its text.
pub fn new_message_preview(recipients: &[Recipient], text: &str) -> String {
    if recipients.is_empty() {
        return line(text, false);
    }
    let names: Vec<&str> = recipients.iter().map(Recipient::title).collect();
    filled("To: %@", &[names.join(", ")])
}

/// A group event as the transcript shows it: what the phone wrote about it, else what happened.
pub(super) fn group_event_text(message: &Message) -> Option<String> {
    let event = message.group_event?;
    if !message.body.is_empty() {
        return Some(message.body.clone());
    }

    let who = if message.contact.address.is_empty() { translated("Someone") } else { contact_name(&message.contact) };
    Some(match event {
        MessageGroupEvent::JoinedSelf => translated("You joined the conversation"),
        MessageGroupEvent::Joined => filled("%@ joined the conversation", &[who]),
        MessageGroupEvent::LeftSelf => translated("You left the conversation"),
        MessageGroupEvent::Left => filled("%@ left the conversation", &[who]),
        MessageGroupEvent::AllParticipantsLeft => translated("Everyone left the conversation"),
        MessageGroupEvent::LeaderChanged => translated("The group leader changed"),
        MessageGroupEvent::RenamedByLeader => translated("The group was renamed"),
        MessageGroupEvent::RemovedByLeader => filled("%@ was removed from the conversation", &[who]),
    })
}

/// Under a message the phone holds and has not delivered: that it is still being sent, that it
/// failed, or that it is a draft.
///
/// `None` for a received or sent message, and for one sent from this computer (its own state shows
/// instead).
pub(super) fn phone_status(message: &Message) -> Option<String> {
    if message.send_state.is_some() {
        return None;
    }
    match message.mailbox {
        MessageMailbox::Outbox | MessageMailbox::Queued => Some(translated("Sending...")),
        MessageMailbox::Failed => Some(translated("Not Delivered")),
        MessageMailbox::Draft => Some(translated("Draft")),
        MessageMailbox::Inbox | MessageMailbox::Sent | MessageMailbox::Unknown => None,
    }
}

/// Under a message the phone could not send: "Not Delivered", with the reason when the phone gave one.
pub(super) fn not_delivered(reason: SendFailure) -> String {
    match failure_text(reason) {
        Some(why) => filled("Not Delivered · %@", &[why]),
        None => translated("Not Delivered"),
    }
}

/// Why the phone could not send; `None` when it did not say.
fn failure_text(reason: SendFailure) -> Option<String> {
    let text = match reason {
        SendFailure::AirplaneMode => "Airplane mode is on",
        SendFailure::NoSim => "No SIM card",
        SendFailure::MessageSizeExceeded | SendFailure::MmsTransactionSizeExceeded => "The message is too large",
        SendFailure::MmsNotSupported => "MMS isn't supported",
        SendFailure::MmsDisabled => "MMS is turned off",
        SendFailure::MmsGroupMessageDisabled => "Group messages are turned off",
        SendFailure::MmsTooManyRecipients => "Too many recipients",
        SendFailure::MmsSubjectSizeExceeded => "The subject is too long",
        SendFailure::MmsAudioNotAllowed => "Audio can't be sent",
        SendFailure::ActionRequired => "Check your phone",
        SendFailure::Empty => "The message is empty",
        SendFailure::RcsSendUnavailable => "Chat isn't available",
        SendFailure::PhoneTimeout => "Your phone didn't respond",
        SendFailure::RadioOff => "Mobile network is off",
        SendFailure::NoService => "No service",
        SendFailure::NoRecipients => "No recipients",
        SendFailure::BluetoothNotConnected => "Your phone isn't connected over Bluetooth",
        SendFailure::Unknown | SendFailure::Other { .. } => return None,
    };
    Some(translated(text))
}

/// Why a phone that sends only over Bluetooth cannot send now; `None` while it is connected or does
/// not need Bluetooth.
pub fn bluetooth_notice(proximity: Option<ProximityState>) -> Option<String> {
    let text = match proximity? {
        ProximityState::Connected => return None,
        ProximityState::BluetoothOff => "Bluetooth is off. Turn it on to send messages from this computer.",
        ProximityState::PhoneNotPaired => {
            "Your phone isn't paired with this computer. Pair it in your system's Bluetooth settings to send messages."
        }
        ProximityState::PairedPhoneUnavailable => "Your phone isn't in Bluetooth range of this computer.",
        ProximityState::NotConnected => "Your phone isn't connected to this computer over Bluetooth.",
    };
    Some(translated(text))
}

/// A SIM's name, else its slot.
pub fn sim_name(sim: &SimSubscription) -> String {
    if sim.name.is_empty() { filled("SIM %lld", &[(sim.sim_slot_index + 1).to_string()]) } else { sim.name.clone() }
}

/// A suggestion's second line: the number's kind and the number.
pub fn number_line(suggestion: &ContactSuggestion) -> String {
    let kind = match suggestion.kind {
        PhoneNumberKind::Mobile => Some(translated("Mobile")),
        PhoneNumberKind::Home => Some(translated("Home")),
        PhoneNumberKind::Work => Some(translated("Work")),
        PhoneNumberKind::WorkMobile => Some(translated("Work Mobile")),
        PhoneNumberKind::Main => Some(translated("Main")),
        PhoneNumberKind::School => Some(translated("School")),
        PhoneNumberKind::Other => Some(translated("Other")),
        PhoneNumberKind::Custom => Some(suggestion.label.clone()).filter(|label| !label.is_empty()),
        PhoneNumberKind::Unknown => None,
    };
    match kind {
        Some(kind) => format!("{kind} · {}", suggestion.number),
        None => suggestion.number.clone(),
    }
}

/// Why the composer takes no more recipients: the SIM's limit.
pub fn cap_notice(cap: usize) -> String {
    if cap == 1 {
        translated("This SIM sends to one person at a time")
    } else {
        fill_plural("This SIM sends to at most %lld people", cap, &[cap.to_string()])
    }
}

/// Why a message to more people than the SIM's limit is not sent, as a sentence.
pub fn cap_sentence(cap: usize) -> String {
    if cap == 1 {
        translated("This SIM sends to one person at a time.")
    } else {
        filled("This SIM sends to at most %lld people.", &[cap.to_string()])
    }
}
