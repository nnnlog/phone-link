//! The sends the PC has not finished: their text, recipients, parts and state.

use super::{from_json, to_json, unified_thread_of_rcs};
use crate::consts::SendMessageFailureReason;
use rusqlite::{Connection, OptionalExtension, params};

/// Where a send came from (composition rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendOrigin {
    /// The conversation view; its thread is the one the UI shows the conversation under.
    Conversation,
    /// The inline reply of an SMS or MMS notification; its thread is the message's thread.
    NotificationReply,
    /// The inline reply of an RCS notification; its thread is the message's thread, which is an RCS
    /// conversation's id (the reply's `threadId`, as Phone Link on Windows sends it). The UI shows the
    /// send under that RCS conversation's fallback id ([`shown_thread`]).
    RcsNotificationReply,
    SendMessageApi,
}

impl SendOrigin {
    fn code(self) -> i64 {
        match self {
            SendOrigin::Conversation => 0,
            SendOrigin::NotificationReply => 1,
            SendOrigin::SendMessageApi => 2,
            SendOrigin::RcsNotificationReply => 3,
        }
    }

    fn from_code(v: i64) -> rusqlite::Result<Self> {
        match v {
            0 => Ok(SendOrigin::Conversation),
            1 => Ok(SendOrigin::NotificationReply),
            2 => Ok(SendOrigin::SendMessageApi),
            3 => Ok(SendOrigin::RcsNotificationReply),
            other => Err(rusqlite::Error::IntegralValueOutOfRange(5, other)),
        }
    }
}

/// The thread id the UI shows a pending send under.
///
/// Its own thread (`-1` for a new conversation), except an RCS notification reply, which shows
/// under its RCS conversation's fallback id — `None` while that RCS conversation is not stored.
pub fn shown_thread(c: &Connection, thread_id: i64, origin: SendOrigin) -> rusqlite::Result<Option<i64>> {
    match origin {
        SendOrigin::RcsNotificationReply => unified_thread_of_rcs(c, thread_id),
        SendOrigin::Conversation | SendOrigin::NotificationReply | SendOrigin::SendMessageApi => Ok(Some(thread_id)),
    }
}

/// The state of a pending send as the UI shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendStatus {
    Sending,
    /// The phone accepted it; the pending send stays until acked.
    Sent,
    /// Failed with the phone's `sendMessageFailureReason` (`Unknown` when absent).
    Failed(i32),
}

impl SendStatus {
    fn encode(self) -> (i64, Option<i32>) {
        match self {
            SendStatus::Sending => (0, None),
            SendStatus::Sent => (1, None),
            SendStatus::Failed(r) => (2, Some(r)),
        }
    }

    fn decode(status: i64, reason: Option<i32>) -> rusqlite::Result<Self> {
        match (status, reason) {
            (0, _) => Ok(SendStatus::Sending),
            (1, _) => Ok(SendStatus::Sent),
            (2, r) => Ok(SendStatus::Failed(r.unwrap_or(11))),
            (other, _) => Err(rusqlite::Error::IntegralValueOutOfRange(5, other)),
        }
    }
}

/// One attachment of a send, as sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentPart {
    pub name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A pending send with what it sends (kept so a retry resends the same message).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSend {
    /// The persistent counter value (`id`, ≥ 1, never reused).
    pub id: i64,
    /// Lower-case hyphenated GUID (`tempId`).
    pub temp_id: String,
    /// `-1` for a new conversation.
    pub thread_id: i64,
    pub created_ms: i64,
    pub origin: SendOrigin,
    pub status: SendStatus,
    pub body: String,
    pub recipients: Vec<String>,
    pub subscription_id: i64,
    pub parts: Vec<SentPart>,
}

/// Stores a new pending send and returns its `id` (rule 1).
#[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
pub fn insert_pending_send(
    c: &Connection,
    temp_id: &str,
    thread_id: i64,
    created_ms: i64,
    origin: SendOrigin,
    body: &str,
    recipients: &[String],
    subscription_id: i64,
    parts: &[SentPart],
) -> rusqlite::Result<i64> {
    let (status, reason) = SendStatus::Sending.encode();
    c.execute(
        "INSERT INTO pending_send (temp_id, thread_id, created_ms, origin, status, failure_reason, body, recipients,
                                   subscription_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            temp_id,
            thread_id,
            created_ms,
            origin.code(),
            status,
            reason,
            body,
            to_json(recipients),
            subscription_id
        ],
    )?;

    let id = c.last_insert_rowid();
    let mut stmt = c.prepare(
        "INSERT INTO pending_send_part (send_id, idx, name, content_type, bytes) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (i, p) in parts.iter().enumerate() {
        stmt.execute(params![id, i as i64, p.name, p.content_type, p.bytes])?;
    }
    Ok(id)
}

pub fn set_send_status(c: &Connection, id: i64, status: SendStatus) -> rusqlite::Result<bool> {
    let (s, r) = status.encode();
    Ok(c.execute("UPDATE pending_send SET status = ?2, failure_reason = ?3 WHERE id = ?1", params![id, s, r])? == 1)
}

/// Marks every pending send still marked sending as failed with reason `Unknown` (11) and returns
/// their ids.
///
/// Called when the store is first opened in an app run: such a send's request ended with
/// an earlier run before its outcome was stored (differs from Phone Link on Windows).
pub fn fail_interrupted_sends(c: &Connection) -> rusqlite::Result<Vec<i64>> {
    let (sending, _) = SendStatus::Sending.encode();
    let ids: Vec<i64> = {
        let mut stmt = c.prepare("SELECT id FROM pending_send WHERE status = ?1 ORDER BY id")?;
        stmt.query_map([sending], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };

    let (failed, reason) = SendStatus::Failed(SendMessageFailureReason::Unknown.as_i32()).encode();
    c.execute(
        "UPDATE pending_send SET status = ?1, failure_reason = ?2 WHERE status = ?3",
        params![failed, reason, sending],
    )?;
    Ok(ids)
}

/// `acks`: deletes the pending sends with these ids; returns the ids that existed.
pub fn delete_pending_sends(c: &Connection, ids: &[i64]) -> rusqlite::Result<Vec<i64>> {
    let mut stmt = c.prepare("DELETE FROM pending_send WHERE id = ?1")?;
    let mut parts = c.prepare("DELETE FROM pending_send_part WHERE send_id = ?1")?;
    let mut deleted = Vec::new();
    for id in ids {
        parts.execute([id])?;
        if stmt.execute([id])? == 1 {
            deleted.push(*id);
        }
    }
    Ok(deleted)
}

pub(super) fn pending_from_row(c: &Connection, r: &rusqlite::Row<'_>) -> rusqlite::Result<PendingSend> {
    let id: i64 = r.get(0)?;
    let mut stmt =
        c.prepare_cached("SELECT name, content_type, bytes FROM pending_send_part WHERE send_id = ?1 ORDER BY idx")?;
    let parts = stmt
        .query_map([id], |p| Ok(SentPart { name: p.get(0)?, content_type: p.get(1)?, bytes: p.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(PendingSend {
        id,
        temp_id: r.get(1)?,
        thread_id: r.get(2)?,
        created_ms: r.get(3)?,
        origin: SendOrigin::from_code(r.get(4)?)?,
        status: SendStatus::decode(r.get(5)?, r.get(6)?)?,
        body: r.get(7)?,
        recipients: from_json(&r.get::<_, String>(8)?)?,
        subscription_id: r.get(9)?,
        parts,
    })
}

pub(super) const PENDING_COLUMNS: &str =
    "id, temp_id, thread_id, created_ms, origin, status, failure_reason, body, recipients, subscription_id";

pub fn pending_send(c: &Connection, id: i64) -> rusqlite::Result<Option<PendingSend>> {
    let mut stmt = c.prepare(&format!("SELECT {PENDING_COLUMNS} FROM pending_send WHERE id = ?1"))?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(r) => Ok(Some(pending_from_row(c, r)?)),
        None => Ok(None),
    }
}

/// Every pending send, oldest first.
pub fn pending_sends(c: &Connection) -> rusqlite::Result<Vec<PendingSend>> {
    let mut stmt = c.prepare(&format!("SELECT {PENDING_COLUMNS} FROM pending_send ORDER BY id"))?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(pending_from_row(c, r)?);
    }
    Ok(out)
}

/// One attachment of a pending send as the views list it: its bytes stay in the store
/// ([`sent_part_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSentPart {
    pub name: String,
    pub content_type: String,
    /// Its byte length.
    pub size: i64,
}

/// A pending send as the views list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSend {
    pub id: i64,
    /// `-1` for a new conversation; an RCS conversation's id for an RCS notification reply.
    pub thread_id: i64,
    pub origin: SendOrigin,
    pub created_ms: i64,
    pub status: SendStatus,
    pub body: String,
    pub recipients: Vec<String>,
    pub subscription_id: i64,
    pub parts: Vec<ListedSentPart>,
}

/// Every pending send without its attachments' bytes, oldest first.
pub fn listed_sends(c: &Connection) -> rusqlite::Result<Vec<ListedSend>> {
    let mut stmt = c.prepare(
        "SELECT id, thread_id, created_ms, status, failure_reason, body, recipients, subscription_id, origin
         FROM pending_send ORDER BY id",
    )?;
    let mut parts =
        c.prepare("SELECT name, content_type, length(bytes) FROM pending_send_part WHERE send_id = ?1 ORDER BY idx")?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        let listed = parts
            .query_map([id], |p| Ok(ListedSentPart { name: p.get(0)?, content_type: p.get(1)?, size: p.get(2)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        out.push(ListedSend {
            id,
            thread_id: r.get(1)?,
            origin: SendOrigin::from_code(r.get(8)?)?,
            created_ms: r.get(2)?,
            status: SendStatus::decode(r.get(3)?, r.get(4)?)?,
            body: r.get(5)?,
            recipients: from_json(&r.get::<_, String>(6)?)?,
            subscription_id: r.get(7)?,
            parts: listed,
        });
    }
    Ok(out)
}

/// The bytes of attachment `index` of pending send `send_id`; `None` when there is no such part.
pub fn sent_part_bytes(c: &Connection, send_id: i64, index: u32) -> rusqlite::Result<Option<Vec<u8>>> {
    c.query_row(
        "SELECT bytes FROM pending_send_part WHERE send_id = ?1 AND idx = ?2",
        params![send_id, i64::from(index)],
        |r| r.get(0),
    )
    .optional()
}
