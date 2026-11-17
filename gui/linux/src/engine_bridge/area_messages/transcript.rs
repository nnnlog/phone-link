//! The shown detail's transcript.
//!
//! A conversation's messages come first, then its sends; the parts being downloaded are looked up too.

use phonelink_engine::api::messaging::{Message, MessagePart, MessagePartRef, MessagePartState, MessageType};

use super::state::State;
use crate::views::messages::{Detail, address};

impl State {
    pub(super) fn downloading(&self, part: MessagePartRef) -> bool {
        self.requested_downloads.contains(&part) || self.awaited_downloads.contains_key(&part)
    }

    /// The transcript of the shown detail, and whether it is a group's.
    ///
    /// A conversation's messages come first, then its sends — those of its composer and those to its
    /// recipients from a new message — in the order they were sent.
    pub(super) fn shown_messages(&self) -> (Vec<Message>, bool) {
        match &self.detail {
            None | Some(Detail::NewMessage) => (Vec::new(), self.new_recipients.len() > 1),
            Some(Detail::Unsent(people)) => {
                let addresses: Vec<String> = people.iter().map(|p| p.address.clone()).collect();
                let messages = self.unsent_group(&addresses).map(|g| g.messages.clone()).unwrap_or_default();
                (messages, people.len() > 1)
            }
            Some(Detail::Conversation(thread_id)) => {
                let conversation = self.conversation(*thread_id);
                let merged: Vec<Message> = conversation
                    .map(|c| {
                        let recipients: Vec<String> = c.recipients.iter().map(|r| r.address.clone()).collect();
                        self.unsent
                            .iter()
                            .filter(|g| address::same_recipients(&g.addresses(), &recipients))
                            .flat_map(|g| g.messages.iter().cloned())
                            .collect()
                    })
                    .unwrap_or_default();
                let (sends, synced): (Vec<Message>, Vec<Message>) =
                    self.transcript.iter().cloned().partition(|m| m.kind == MessageType::PendingSend);
                let mut sends: Vec<Message> = sends.into_iter().chain(merged).collect();
                sends.sort_by_key(|m| m.id);
                (synced.into_iter().chain(sends).collect(), conversation.is_some_and(|c| c.is_group))
            }
        }
    }

    /// The transcript shows the part with its bytes (or no longer as downloadable).
    pub(super) fn arrived(&self, part: MessagePartRef) -> bool {
        self.shown_messages()
            .0
            .iter()
            .flat_map(|m| m.parts.iter())
            .find(|p| p.part == part)
            .is_some_and(|p| p.has_bytes || p.state != MessagePartState::Downloadable)
    }

    pub(super) fn find_part(&self, part: MessagePartRef) -> Option<MessagePart> {
        self.shown_messages().0.iter().flat_map(|m| m.parts.iter()).find(|p| p.part == part).cloned()
    }
}
