//! The delta between the phone's ids and checksums and the stored ones.

use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

/// The tables that take part in the delta of a metadata message, with their id column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Table {
    Sms,
    Mms,
    RcsChat,
    RcsFileTransfer,
    Conversation,
    RcsConversation,
    Subscription,
}

impl Table {
    pub(super) fn sql(self) -> (&'static str, &'static str) {
        match self {
            Table::Sms => ("sms", "id"),
            Table::Mms => ("mms", "id"),
            Table::RcsChat => ("rcs_chat", "id"),
            Table::RcsFileTransfer => ("rcs_file_transfer", "id"),
            Table::Conversation => ("conversation", "thread_id"),
            Table::RcsConversation => ("rcs_conversation", "id"),
            Table::Subscription => ("subscription", "id"),
        }
    }
}

/// The delta computation.
///
/// Deletes every local row whose id is not in `ids` (an empty list deletes all) and returns, in
/// incoming order, the ids that have no local row, whose local checksum differs from the incoming
/// one, or whose local checksum is unknown.
///
/// `checksums = None`
/// compares against `0` (the types whose checksums may be absent). `checksums` must cover `ids`
/// (validated before).
pub fn delta(c: &Connection, table: Table, ids: &[i64], checksums: Option<&[i64]>) -> rusqlite::Result<Vec<i64>> {
    let (name, id_col) = table.sql();
    let mut local: HashMap<i64, Option<i64>> = HashMap::new();
    {
        let mut stmt = c.prepare(&format!("SELECT {id_col}, checksum FROM {name}"))?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?)))?;
        for row in rows {
            let (id, checksum) = row?;
            local.insert(id, checksum);
        }
    }

    let incoming: HashSet<i64> = ids.iter().copied().collect();
    let gone: Vec<i64> = local.keys().copied().filter(|id| !incoming.contains(id)).collect();
    delete_rows(c, table, &gone)?;

    let mut requested = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let incoming_checksum = checksums.map_or(0, |cs| cs[i]);
        match local.get(id) {
            Some(Some(stored)) if *stored == incoming_checksum => {}
            _ => requested.push(*id),
        }
    }

    Ok(requested)
}

/// Deletes rows by id (MMS rows take their parts with them).
pub fn delete_rows(c: &Connection, table: Table, ids: &[i64]) -> rusqlite::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }

    let (name, id_col) = table.sql();
    let mut stmt = c.prepare(&format!("DELETE FROM {name} WHERE {id_col} = ?1"))?;
    let mut parts = c.prepare("DELETE FROM mms_part WHERE message_id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
        if table == Table::Mms {
            parts.execute([id])?;
        }
    }
    Ok(())
}

/// Forced resync without checksums: every local row of the type that is not in `unread` is marked
/// read on the phone.
pub fn mark_phone_read_except(c: &Connection, table: Table, unread: &[i64]) -> rusqlite::Result<()> {
    let (name, id_col) = table.sql();
    let keep: HashSet<i64> = unread.iter().copied().collect();
    let ids: Vec<i64> = {
        let mut stmt = c.prepare(&format!("SELECT {id_col} FROM {name} WHERE phone_read = 0"))?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };

    let mut update = c.prepare(&format!("UPDATE {name} SET phone_read = 1 WHERE {id_col} = ?1"))?;
    for id in ids.into_iter().filter(|id| !keep.contains(id)) {
        update.execute([id])?;
    }
    Ok(())
}
