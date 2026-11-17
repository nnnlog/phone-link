//! The keys the window tells a transcript's messages and their parts apart by.

use phonelink_engine::api::messaging::{Message, MessagePartRef, MessageType};

/// A message's key in a transcript: ids are unique per kind only.
pub(super) fn message_key(message: &Message) -> String {
    let kind = match message.kind {
        MessageType::Sms => "sms",
        MessageType::Mms => "mms",
        MessageType::RcsChat => "rcs",
        MessageType::RcsFileTransfer => "rcsfile",
        MessageType::PendingSend => "send",
    };
    format!("{kind}-{}", message.id)
}

/// A part's key, which the window hands back to download, open or save it.
pub fn part_key(part: MessagePartRef) -> String {
    match part {
        MessagePartRef::MmsPart { id } => format!("mms-{id}"),
        MessagePartRef::RcsFile { id } => format!("rcs-{id}"),
        MessagePartRef::SentPart { send_id, index } => format!("sent-{send_id}-{index}"),
    }
}

/// The part a key names; `None` for a key no part has.
pub fn parse_part_key(key: &str) -> Option<MessagePartRef> {
    let (kind, rest) = key.split_once('-')?;
    match kind {
        "mms" => Some(MessagePartRef::MmsPart { id: rest.parse().ok()? }),
        "rcs" => Some(MessagePartRef::RcsFile { id: rest.parse().ok()? }),
        "sent" => {
            let (send_id, index) = rest.split_once('-')?;
            Some(MessagePartRef::SentPart { send_id: send_id.parse().ok()?, index: index.parse().ok()? })
        }
        _ => None,
    }
}
