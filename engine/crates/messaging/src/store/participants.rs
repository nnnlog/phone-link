//! The conversations' participants.

use rusqlite::{Connection, params};
use std::collections::HashSet;

pub fn participants_empty(c: &Connection) -> rusqlite::Result<bool> {
    let any: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM participant)", [], |r| r.get(0))?;
    Ok(!any)
}

pub fn clear_participants(c: &Connection) -> rusqlite::Result<()> {
    c.execute("DELETE FROM participant", []).map(|_| ())
}

/// Adds a participant, or updates the sent form of the one with the same normalised address.
pub fn upsert_participant(c: &Connection, thread_id: i64, address: &str, normalized: &str) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO participant (thread_id, address, normalized) VALUES (?1, ?2, ?3)
         ON CONFLICT (thread_id, normalized) DO UPDATE SET address = excluded.address",
        params![thread_id, address, normalized],
    )
    .map(|_| ())
}

/// An RCS conversation's participants: drops those of `thread_id` whose normalised address is not one
/// of `recipients`, then adds each missing recipient raw as both its sent and normalised form.
pub fn set_rcs_participants(c: &Connection, thread_id: i64, recipients: &[String]) -> rusqlite::Result<()> {
    let keep: HashSet<&str> = recipients.iter().map(String::as_str).collect();
    let stored: Vec<String> = {
        let mut stmt = c.prepare("SELECT normalized FROM participant WHERE thread_id = ?1")?;
        stmt.query_map([thread_id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };

    let mut delete = c.prepare("DELETE FROM participant WHERE thread_id = ?1 AND normalized = ?2")?;
    for n in stored.iter().filter(|n| !keep.contains(n.as_str())) {
        delete.execute(params![thread_id, n])?;
    }

    let mut insert =
        c.prepare("INSERT OR IGNORE INTO participant (thread_id, address, normalized) VALUES (?1, ?2, ?2)")?;
    for r in recipients {
        insert.execute(params![thread_id, r])?;
    }
    Ok(())
}

/// A thread's participants `(sent address, normalised address)` in the order they were added.
pub fn participants(c: &Connection, thread_id: i64) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = c.prepare("SELECT address, normalized FROM participant WHERE thread_id = ?1 ORDER BY seq")?;
    stmt.query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}
