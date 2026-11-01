//! The conversation list as the PC shows it.

use super::from_json;
use crate::consts::{mailbox, rcs_message_type};
use phonelink_core::time::FILETIME_ZERO_UNIX_MS;
use rusqlite::{Connection, OptionalExtension};

/// A stored SMS/MMS conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredConversation {
    pub thread_id: i64,
    pub recipients: Vec<String>,
    pub has_rcs: bool,
}

pub fn conversation(c: &Connection, thread_id: i64) -> rusqlite::Result<Option<StoredConversation>> {
    c.query_row("SELECT thread_id, recipients, has_rcs FROM conversation WHERE thread_id = ?1", [thread_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?))
    })
    .optional()?
    .map(|(thread_id, recipients, has_rcs)| {
        Ok(StoredConversation { thread_id, recipients: from_json(&recipients)?, has_rcs })
    })
    .transpose()
}

pub fn all_conversations(c: &Connection) -> rusqlite::Result<Vec<StoredConversation>> {
    let mut stmt = c.prepare("SELECT thread_id, recipients, has_rcs FROM conversation ORDER BY thread_id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?;

    let mut out = Vec::new();
    for row in rows {
        let (thread_id, recipients, has_rcs) = row?;
        out.push(StoredConversation { thread_id, recipients: from_json(&recipients)?, has_rcs });
    }
    Ok(out)
}

/// A stored RCS conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRcsConversation {
    pub id: i64,
    pub fallback_id: i64,
    pub conversation_type: i32,
    pub recipients: Vec<String>,
}

pub(super) fn rcs_conversation_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRcsConversation> {
    Ok(StoredRcsConversation {
        id: r.get(0)?,
        fallback_id: r.get(1)?,
        conversation_type: r.get(2)?,
        recipients: from_json(&r.get::<_, String>(3)?)?,
    })
}

pub fn rcs_conversation(c: &Connection, id: i64) -> rusqlite::Result<Option<StoredRcsConversation>> {
    c.query_row(
        "SELECT id, fallback_id, conversation_type, recipients FROM rcs_conversation WHERE id = ?1",
        [id],
        rcs_conversation_from,
    )
    .optional()
}

pub fn all_rcs_conversations(c: &Connection) -> rusqlite::Result<Vec<StoredRcsConversation>> {
    let mut stmt =
        c.prepare("SELECT id, fallback_id, conversation_type, recipients FROM rcs_conversation ORDER BY id")?;
    stmt.query_map([], rcs_conversation_from)?.collect()
}

/// The RCS conversations merged into the conversation shown under `thread_id`.
pub fn rcs_conversations_for(c: &Connection, thread_id: i64) -> rusqlite::Result<Vec<StoredRcsConversation>> {
    let mut stmt = c.prepare(
        "SELECT id, fallback_id, conversation_type, recipients FROM rcs_conversation WHERE fallback_id = ?1 ORDER BY id",
    )?;
    stmt.query_map([thread_id], rcs_conversation_from)?.collect()
}

/// The thread id the PC shows an RCS conversation under: its fallback id.
pub fn unified_thread_of_rcs(c: &Connection, rcs_conversation_id: i64) -> rusqlite::Result<Option<i64>> {
    c.query_row("SELECT fallback_id FROM rcs_conversation WHERE id = ?1", [rcs_conversation_id], |r| r.get(0))
        .optional()
}

/// Unread counts and the latest timestamp of the conversation shown under `thread_id` (the SMS/MMS
/// thread and the RCS conversations merged into it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversationCounts {
    /// Inbox rows unread on the phone and not yet read on the PC.
    pub pc_unread: i64,
    /// Rows whose `readFlags` is `false`.
    pub phone_unread: i64,
    /// Unix ms of the latest inbox or sent message (1601-01-01 when none).
    pub latest_ms: i64,
    pub message_count: i64,
}

/// RCS group events (`rcsMessageTypes` 1–8) are not counted as messages.
pub(super) const RCS_NOT_EVENT: &str = "rcs_message_type NOT BETWEEN 1 AND 8";

pub fn conversation_counts(c: &Connection, thread_id: i64) -> rusqlite::Result<ConversationCounts> {
    let _ = (rcs_message_type::FIRST_GROUP_EVENT, rcs_message_type::LAST_SHOWN_GROUP_EVENT);
    let sql = format!(
        "WITH rows (timestamp_ms, mailbox, phone_read, pc_read) AS (
            SELECT timestamp_ms, mailbox, phone_read, pc_read FROM sms WHERE thread_id = ?1
            UNION ALL SELECT timestamp_ms, mailbox, phone_read, pc_read FROM mms WHERE thread_id = ?1
            UNION ALL SELECT timestamp_ms, mailbox, phone_read, pc_read FROM rcs_chat
                WHERE {RCS_NOT_EVENT} AND thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1)
            UNION ALL SELECT timestamp_ms, mailbox, phone_read, pc_read FROM rcs_file_transfer
                WHERE thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1))
         SELECT
            COALESCE(SUM(mailbox = {inbox} AND phone_read = 0 AND pc_read = 0), 0),
            COALESCE(SUM(phone_read = 0), 0),
            MAX(CASE WHEN mailbox IN ({inbox}, {sent}) THEN timestamp_ms END),
            COUNT(*)
         FROM rows",
        inbox = mailbox::INBOX,
        sent = mailbox::SENT,
    );

    c.query_row(&sql, [thread_id], |r| {
        Ok(ConversationCounts {
            pc_unread: r.get(0)?,
            phone_unread: r.get(1)?,
            latest_ms: r.get::<_, Option<i64>>(2)?.unwrap_or(FILETIME_ZERO_UNIX_MS),
            message_count: r.get(3)?,
        })
    })
}

/// The table a conversation's latest message comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatestTable {
    Sms,
    Mms,
    RcsChat,
    RcsFile,
}

/// The latest inbox or sent message of a conversation, as [`latest_message`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestMessage {
    pub table: LatestTable,
    pub id: i64,
    pub mailbox: i32,
    pub from_address: String,
    /// `sms.body` / `rcs_chat.body`; empty for an MMS (its text is in its parts) and an RCS file.
    pub body: String,
}

/// The latest inbox or sent message of the conversation shown under `thread_id` — the one whose
/// time [`conversation_counts`] reports as `latest_ms` — among the rows the conversation view
/// shows.
///
/// Mass-text copies (`group_id` neither −1 nor the row's own id) and RCS group events left out.
///
/// Ties
/// on the time go to the higher id, as the view orders them.
pub fn latest_message(c: &Connection, thread_id: i64) -> rusqlite::Result<Option<LatestMessage>> {
    let sql = format!(
        "WITH rows (timestamp_ms, kind, id, mailbox, from_address, body) AS (
            SELECT timestamp_ms, 0, id, mailbox, from_address, body FROM sms
                WHERE thread_id = ?1 AND (group_id = {none} OR group_id = id)
            UNION ALL SELECT timestamp_ms, 1, id, mailbox, from_address, '' FROM mms WHERE thread_id = ?1
            UNION ALL SELECT timestamp_ms, 2, id, mailbox, from_address, body FROM rcs_chat
                WHERE {RCS_NOT_EVENT} AND thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1)
            UNION ALL SELECT timestamp_ms, 3, id, mailbox, from_address, '' FROM rcs_file_transfer
                WHERE thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1))
         SELECT kind, id, mailbox, from_address, body FROM rows
         WHERE mailbox IN ({inbox}, {sent})
         ORDER BY timestamp_ms DESC, id DESC
         LIMIT 1",
        none = super::super::consts::NONE_ID,
        inbox = mailbox::INBOX,
        sent = mailbox::SENT,
    );

    let mut stmt = c.prepare(&sql)?;
    let mut rows = stmt.query([thread_id])?;
    let Some(r) = rows.next()? else { return Ok(None) };

    let kind: i64 = r.get(0)?;
    let table = match kind {
        0 => LatestTable::Sms,
        1 => LatestTable::Mms,
        2 => LatestTable::RcsChat,
        3 => LatestTable::RcsFile,
        other => return Err(rusqlite::Error::IntegralValueOutOfRange(0, other)),
    };

    Ok(Some(LatestMessage { table, id: r.get(1)?, mailbox: r.get(2)?, from_address: r.get(3)?, body: r.get(4)? }))
}

/// Marks every message of the conversation shown under `thread_id` read on the PC.
pub fn mark_read_on_pc(c: &Connection, thread_id: i64) -> rusqlite::Result<()> {
    c.execute("UPDATE sms SET pc_read = 1 WHERE thread_id = ?1", [thread_id])?;
    c.execute("UPDATE mms SET pc_read = 1 WHERE thread_id = ?1", [thread_id])?;

    for table in ["rcs_chat", "rcs_file_transfer"] {
        c.execute(
            &format!(
                "UPDATE {table} SET pc_read = 1 WHERE thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1)"
            ),
            [thread_id],
        )?;
    }
    Ok(())
}

/// The latest message's subscription in the conversation shown under `thread_id`.
pub fn last_message_subscription(c: &Connection, thread_id: i64) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT subscription_id FROM (
            SELECT timestamp_ms, id, subscription_id FROM sms WHERE thread_id = ?1
            UNION ALL SELECT timestamp_ms, id, subscription_id FROM mms WHERE thread_id = ?1
            UNION ALL SELECT timestamp_ms, id, subscription_id FROM rcs_chat
                WHERE thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1)
            UNION ALL SELECT timestamp_ms, id, subscription_id FROM rcs_file_transfer
                WHERE thread_id IN (SELECT id FROM rcs_conversation WHERE fallback_id = ?1))
         ORDER BY timestamp_ms DESC, id DESC LIMIT 1",
        [thread_id],
        |r| r.get(0),
    )
    .optional()
}
