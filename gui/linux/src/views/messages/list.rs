//! The visible conversations in the shape the main window's sidebar and section read.

use phonelink_engine::api::messaging::Conversation;
use serde_json::{Value, json};

use super::{conversation_preview, conversation_title};

/// The visible conversations as the main window reads them (its unread badge, the open conversation).
pub fn conversations_json(conversations: &[Conversation]) -> String {
    let items: Vec<Value> = conversations
        .iter()
        .filter(|c| !c.hidden)
        .map(|c| {
            let single = c.recipients.first().filter(|_| !c.is_group);
            json!({
                "threadId": c.thread_id,
                "name": conversation_title(c),
                "address": single.map(|r| r.address.clone()).unwrap_or_default(),
                "isGroup": c.is_group,
                "preview": conversation_preview(c),
                "latestMs": c.latest_ms,
                "unread": c.unread,
                "canReply": c.can_reply,
            })
        })
        .collect();
    Value::Array(items).to_string()
}
