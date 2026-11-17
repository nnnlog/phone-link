//! Messages sent to new recipients before the phone created their conversation, grouped by their
//! recipients.

use phonelink_engine::api::messaging::{Message, MessageContact, NewConversationMessage};

use super::address;

/// Messages sent to the same people before the phone created a conversation for them: one list row,
/// until their conversation exists — then they show in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsentGroup {
    /// The recipients of its first message, with their contacts.
    pub recipients: Vec<MessageContact>,
    /// Oldest first.
    pub messages: Vec<Message>,
}

impl UnsentGroup {
    pub fn addresses(&self) -> Vec<String> {
        self.recipients.iter().map(|r| r.address.clone()).collect()
    }

    pub fn latest(&self) -> Option<&Message> {
        self.messages.last()
    }
}

/// The messages sent to new recipients, one group per set of recipients, newest group first; each
/// group's messages oldest first, as the engine lists them.
pub fn unsent_groups(sent: &[NewConversationMessage]) -> Vec<UnsentGroup> {
    let mut groups: Vec<UnsentGroup> = Vec::new();
    for message in sent {
        let addresses: Vec<String> = message.recipients.iter().map(|r| r.address.clone()).collect();
        match groups.iter_mut().find(|group| address::same_recipients(&group.addresses(), &addresses)) {
            Some(group) => group.messages.push(message.message.clone()),
            None => groups
                .push(UnsentGroup { recipients: message.recipients.clone(), messages: vec![message.message.clone()] }),
        }
    }

    groups.sort_by_key(|group| std::cmp::Reverse(group.latest().map_or(0, |m| m.timestamp_ms)));
    groups
}
