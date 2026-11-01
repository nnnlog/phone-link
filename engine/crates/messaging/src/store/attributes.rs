//! Per-conversation flags the PC keeps.

use rusqlite::{Connection, OptionalExtension, params};

/// Local per-conversation state: muted, hidden, the user's SIM choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ConversationAttr {
    pub muted: bool,
    pub hidden: bool,
    pub selected_subscription: Option<i64>,
}

pub fn conversation_attr(c: &Connection, thread_id: i64) -> rusqlite::Result<ConversationAttr> {
    Ok(c.query_row(
        "SELECT muted, hidden, selected_subscription FROM conversation_attr WHERE thread_id = ?1",
        [thread_id],
        |r| Ok(ConversationAttr { muted: r.get(0)?, hidden: r.get(1)?, selected_subscription: r.get(2)? }),
    )
    .optional()?
    .unwrap_or_default())
}

pub fn set_conversation_attr(c: &Connection, thread_id: i64, a: ConversationAttr) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO conversation_attr (thread_id, muted, hidden, selected_subscription) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (thread_id) DO UPDATE SET muted = excluded.muted, hidden = excluded.hidden,
            selected_subscription = excluded.selected_subscription",
        params![thread_id, a.muted, a.hidden, a.selected_subscription],
    )
    .map(|_| ())
}
