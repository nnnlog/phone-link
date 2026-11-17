//! The conversation list's search.

use phonelink_engine::api::messaging::{Conversation, MessageContact};

use super::{UnsentGroup, conversation_preview};

fn contact_matches(contact: &MessageContact, query: &str) -> bool {
    contact.address.to_lowercase().contains(query)
        || contact.name.as_ref().is_some_and(|name| name.to_lowercase().contains(query))
}

/// The list's search: a recipient's name or address, or the preview, contains the query.
pub fn conversation_matches(conversation: &Conversation, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || conversation.recipients.iter().any(|r| contact_matches(r, &query))
        || conversation_preview(conversation).to_lowercase().contains(&query)
}

/// What was sent to new recipients matches the search by a recipient.
pub fn unsent_matches(group: &UnsentGroup, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty() || group.recipients.iter().any(|r| contact_matches(r, &query))
}
