//! A conversation's messages as the PC shows them.

#[cfg(feature = "test-support")]
use super::Table;
use super::opt_from_json;
use crate::parse::{MmsPartRow, MmsRow, RcsChatRow, RcsFileTransferRow, SmsRow};
use rusqlite::{Connection, OptionalExtension};

/// A stored SMS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSms {
    pub row: SmsRow,
    pub pc_read: bool,
}

pub fn sms_in_thread(c: &Connection, thread_id: i64) -> rusqlite::Result<Vec<StoredSms>> {
    let mut stmt = c.prepare(
        "SELECT id, checksum, thread_id, timestamp_ms, from_address, to_address, body, subject, mailbox, phone_read,
                subscription_id, group_id, pc_read FROM sms WHERE thread_id = ?1 ORDER BY timestamp_ms, id",
    )?;
    stmt.query_map([thread_id], |r| {
        Ok(StoredSms {
            row: SmsRow {
                id: r.get(0)?,
                checksum: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                thread_id: r.get(2)?,
                timestamp_ms: r.get(3)?,
                from_address: r.get(4)?,
                to_address: r.get(5)?,
                body: r.get(6)?,
                subject: r.get(7)?,
                mailbox: r.get(8)?,
                read: r.get(9)?,
                subscription_id: r.get(10)?,
                group_id: r.get(11)?,
            },
            pc_read: r.get(12)?,
        })
    })?
    .collect()
}

/// A stored MMS with its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredMms {
    pub row: MmsRow,
    pub pc_read: bool,
    /// Its parts in part order, `application/smil` parts excluded.
    pub parts: Vec<MmsPartRow>,
}

/// The MMS row of a [`MMS_COLUMNS`] query.
pub(super) fn mms_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MmsRow> {
    Ok(MmsRow {
        id: r.get(0)?,
        checksum: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
        thread_id: r.get(2)?,
        timestamp_ms: r.get(3)?,
        subject: r.get(4)?,
        subject_charset: r.get(5)?,
        mailbox: r.get(6)?,
        read: r.get(7)?,
        subscription_id: r.get(8)?,
        from_address: r.get(9)?,
        to_addresses: opt_from_json(r.get(10)?)?,
    })
}

pub(super) fn mms_from(c: &Connection, r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredMms> {
    let row = mms_row(r)?;
    let parts = mms_parts(c, row.id)?;
    Ok(StoredMms { row, pc_read: r.get(11)?, parts })
}

pub(super) const MMS_COLUMNS: &str = "id, checksum, thread_id, timestamp_ms, subject, subject_charset, mailbox, phone_read, \
                           subscription_id, from_address, to_addresses, pc_read";

#[cfg(feature = "test-support")]
pub fn mms_in_thread(c: &Connection, thread_id: i64) -> rusqlite::Result<Vec<StoredMms>> {
    let mut stmt =
        c.prepare(&format!("SELECT {MMS_COLUMNS} FROM mms WHERE thread_id = ?1 ORDER BY timestamp_ms, id"))?;
    let mut rows = stmt.query([thread_id])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        out.push(mms_from(c, r)?);
    }
    Ok(out)
}

pub fn mms_by_id(c: &Connection, id: i64) -> rusqlite::Result<Option<StoredMms>> {
    let mut stmt = c.prepare(&format!("SELECT {MMS_COLUMNS} FROM mms WHERE id = ?1"))?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(r) => Ok(Some(mms_from(c, r)?)),
        None => Ok(None),
    }
}

/// An MMS's parts in part order, `application/smil` parts excluded.
pub fn mms_parts(c: &Connection, message_id: i64) -> rusqlite::Result<Vec<MmsPartRow>> {
    let mut stmt = c.prepare_cached(
        "SELECT part_id, message_id, sequence, content_id, content_type, name, text, charset, size, bytes
         FROM mms_part WHERE message_id = ?1 AND content_type != 'application/smil' ORDER BY sequence, part_id",
    )?;
    stmt.query_map([message_id], part_from)?.collect()
}

pub(super) fn part_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<MmsPartRow> {
    Ok(MmsPartRow {
        part_id: r.get(0)?,
        message_id: r.get(1)?,
        sequence: r.get(2)?,
        content_id: r.get(3)?,
        content_type: r.get(4)?,
        name: r.get(5)?,
        text: r.get(6)?,
        charset: r.get(7)?,
        size: r.get(8)?,
        bytes: r.get(9)?,
    })
}

/// An MMS part as the views show it: its bytes stay in the store ([`mms_part_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownPart {
    pub part_id: i64,
    pub content_type: String,
    pub name: String,
    pub text: String,
    pub size: i64,
    pub has_bytes: bool,
}

/// An MMS as the views show it, its parts without their bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownMms {
    pub row: MmsRow,
    pub pc_read: bool,
    /// Its parts in part order, `application/smil` parts excluded.
    pub parts: Vec<ShownPart>,
}

/// An MMS's parts without their bytes, in part order, `application/smil` parts excluded.
pub fn shown_parts(c: &Connection, message_id: i64) -> rusqlite::Result<Vec<ShownPart>> {
    let mut stmt = c.prepare_cached(
        "SELECT part_id, content_type, name, text, size, bytes IS NOT NULL
         FROM mms_part WHERE message_id = ?1 AND content_type != 'application/smil' ORDER BY sequence, part_id",
    )?;
    stmt.query_map([message_id], |r| {
        Ok(ShownPart {
            part_id: r.get(0)?,
            content_type: r.get(1)?,
            name: r.get(2)?,
            text: r.get(3)?,
            size: r.get(4)?,
            has_bytes: r.get(5)?,
        })
    })?
    .collect()
}

/// The MMS of a thread with their parts without bytes, oldest first.
pub fn shown_mms_in_thread(c: &Connection, thread_id: i64) -> rusqlite::Result<Vec<ShownMms>> {
    let mut stmt =
        c.prepare(&format!("SELECT {MMS_COLUMNS} FROM mms WHERE thread_id = ?1 ORDER BY timestamp_ms, id"))?;
    let mut rows = stmt.query([thread_id])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        out.push(ShownMms { row: mms_row(r)?, pc_read: r.get(11)?, parts: shown_parts(c, id)? });
    }
    Ok(out)
}

/// The bytes of an MMS part: `Ok(None)` when the part is not stored, `Ok(Some(None))` when it is
/// stored without bytes.
pub fn mms_part_bytes(c: &Connection, part_id: i64) -> rusqlite::Result<Option<Option<Vec<u8>>>> {
    c.query_row("SELECT bytes FROM mms_part WHERE part_id = ?1", [part_id], |r| r.get(0)).optional()
}

pub fn mms_part(c: &Connection, part_id: i64) -> rusqlite::Result<Option<MmsPartRow>> {
    c.query_row(
        "SELECT part_id, message_id, sequence, content_id, content_type, name, text, charset, size, bytes
         FROM mms_part WHERE part_id = ?1",
        [part_id],
        part_from,
    )
    .optional()
}

/// A stored RCS chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRcsChat {
    pub row: RcsChatRow,
    pub pc_read: bool,
}

/// A stored RCS file transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRcsFileTransfer {
    pub row: RcsFileTransferRow,
    pub pc_read: bool,
}

pub(super) fn rcs_common(r: &rusqlite::Row<'_>) -> rusqlite::Result<super::super::parse::RcsRow> {
    Ok(super::super::parse::RcsRow {
        id: r.get(0)?,
        checksum: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
        thread_id: r.get(2)?,
        timestamp_ms: r.get(3)?,
        from_address: r.get(4)?,
        content_type: r.get(5)?,
        session_id: r.get(6)?,
        mailbox: r.get(7)?,
        read: r.get(8)?,
        rcs_message_type: r.get(9)?,
        subscription_id: r.get(10)?,
        recipients: opt_from_json(r.get(11)?)?,
        user_action_required: r.get(12)?,
    })
}

pub(super) const RCS_COMMON_COLUMNS: &str = "id, checksum, thread_id, timestamp_ms, from_address, content_type, session_id, \
                                  mailbox, phone_read, rcs_message_type, subscription_id, recipients, \
                                  user_action_required";

pub fn rcs_chats_in(c: &Connection, rcs_conversation_id: i64) -> rusqlite::Result<Vec<StoredRcsChat>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {RCS_COMMON_COLUMNS}, body, pc_read FROM rcs_chat WHERE thread_id = ?1 ORDER BY timestamp_ms, id"
    ))?;
    stmt.query_map([rcs_conversation_id], |r| {
        Ok(StoredRcsChat { row: RcsChatRow { common: rcs_common(r)?, body: r.get(13)? }, pc_read: r.get(14)? })
    })?
    .collect()
}

pub(super) fn file_transfer_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRcsFileTransfer> {
    Ok(StoredRcsFileTransfer {
        row: RcsFileTransferRow {
            common: rcs_common(r)?,
            file_name: r.get(13)?,
            file_size: r.get(14)?,
            bytes: r.get(15)?,
        },
        pc_read: r.get(16)?,
    })
}

#[cfg(feature = "test-support")]
pub fn rcs_file_transfers_in(c: &Connection, rcs_conversation_id: i64) -> rusqlite::Result<Vec<StoredRcsFileTransfer>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {RCS_COMMON_COLUMNS}, file_name, file_size, bytes, pc_read FROM rcs_file_transfer
         WHERE thread_id = ?1 ORDER BY timestamp_ms, id"
    ))?;
    stmt.query_map([rcs_conversation_id], file_transfer_from)?.collect()
}

/// An RCS file transfer as the views show it: its bytes stay in the store ([`rcs_file_bytes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownRcsFile {
    pub common: super::super::parse::RcsRow,
    pub file_name: String,
    pub file_size: i32,
    pub has_bytes: bool,
    pub pc_read: bool,
}

/// The file transfers of an RCS conversation without their bytes, oldest first.
pub fn shown_rcs_files_in(c: &Connection, rcs_conversation_id: i64) -> rusqlite::Result<Vec<ShownRcsFile>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {RCS_COMMON_COLUMNS}, file_name, file_size, bytes IS NOT NULL, pc_read FROM rcs_file_transfer
         WHERE thread_id = ?1 ORDER BY timestamp_ms, id"
    ))?;
    stmt.query_map([rcs_conversation_id], |r| {
        Ok(ShownRcsFile {
            common: rcs_common(r)?,
            file_name: r.get(13)?,
            file_size: r.get(14)?,
            has_bytes: r.get(15)?,
            pc_read: r.get(16)?,
        })
    })?
    .collect()
}

/// The bytes of an RCS file transfer: `Ok(None)` when it is not stored, `Ok(Some(None))` when it is
/// stored without bytes.
pub fn rcs_file_bytes(c: &Connection, id: i64) -> rusqlite::Result<Option<Option<Vec<u8>>>> {
    c.query_row("SELECT bytes FROM rcs_file_transfer WHERE id = ?1", [id], |r| r.get(0)).optional()
}

pub fn rcs_file_transfer(c: &Connection, id: i64) -> rusqlite::Result<Option<StoredRcsFileTransfer>> {
    c.query_row(
        &format!(
            "SELECT {RCS_COMMON_COLUMNS}, file_name, file_size, bytes, pc_read FROM rcs_file_transfer WHERE id = ?1"
        ),
        [id],
        file_transfer_from,
    )
    .optional()
}

#[cfg(feature = "test-support")]
/// Every id and checksum of `table` (tests and diagnostics).
pub fn ids_and_checksums(c: &Connection, table: Table) -> rusqlite::Result<Vec<(i64, Option<i64>)>> {
    let (name, id_col) = table.sql();
    let mut stmt = c.prepare(&format!("SELECT {id_col}, checksum FROM {name} ORDER BY {id_col}"))?;
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}
