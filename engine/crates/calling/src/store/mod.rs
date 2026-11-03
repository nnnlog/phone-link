//! The call-history store: one SQLite database per phone.
//!
//! It holds the call table, the `call_logs` sequence pair, and the per-phone state — "use calls on
//! this Mac", "make and answer calls on this Mac", the one automatic refresh of an empty history, the
//! last completed sync, and the missed-call badge's mark.
//!
//! Times are Unix milliseconds.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::wire::{CallEntry, Items, TYPE_MISSED};

/// The feature name of the store files.
pub const FEATURE: &str = "calling";

/// Schema migrations, applied in order.
pub const MIGRATIONS: &[&str] = &[
    r#"
CREATE TABLE call_history (
    call_id INTEGER PRIMARY KEY,
    phone_number TEXT NOT NULL,
    duration INTEGER NOT NULL,
    call_type INTEGER NOT NULL,
    start_time INTEGER NOT NULL,
    is_read INTEGER NOT NULL,
    phone_account_id TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL
);
CREATE INDEX call_history_by_start ON call_history (start_time);
CREATE TABLE sequence (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
INSERT INTO sequence (id) VALUES (1);
CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    pc_consent INTEGER NOT NULL DEFAULT 0,
    auto_refresh_done INTEGER NOT NULL DEFAULT 0,
    last_updated_ms INTEGER,
    badge_mark_ms INTEGER
);
INSERT INTO state (id) VALUES (1);
"#,
    r#"
ALTER TABLE state ADD COLUMN call_control INTEGER NOT NULL DEFAULT 0;
"#,
];

/// One stored call.
pub type CallRow = CallEntry;

/// The `call_logs` sequence pair and the last completed sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    pub current: i64,
    pub expected: i64,
    /// The last completed call-log transaction, Unix ms.
    pub last_updated_ms: Option<i64>,
}

impl SyncState {
    /// Up to date: `current ≥ expected`.
    pub fn up_to_date(&self) -> bool {
        self.current >= self.expected
    }
}

/// The per-phone switches and marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhoneState {
    /// "Use calls on this Mac" (default off).
    pub pc_consent: bool,
    /// "Make and answer calls on this Mac" (default off).
    pub call_control: bool,
    /// The one automatic refresh of an empty history was sent.
    pub auto_refresh_done: bool,
    /// The newest start time the user has seen; `None` until the calls view first showed a stored
    /// call.
    pub badge_mark_ms: Option<i64>,
}

/// What a metadata message did to the store.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Synchronized {
    pub deleted: usize,
    /// The ids the PC asks for, in the phone's order.
    pub wanted: Vec<i64>,
}

/// Reconciles the table with the phone's full id list.
///
/// Deletes every stored call the list does not name, then returns the listed ids to ask for — every
/// one with `every_listed`, else those with no row or another last-update time.
pub fn synchronize(
    tx: &Transaction<'_>,
    ids: &[i64],
    last_updated: &[i64],
    every_listed: bool,
) -> rusqlite::Result<Synchronized> {
    let stored: HashMap<i64, i64> = {
        let mut stmt = tx.prepare("SELECT call_id, last_updated_time FROM call_history")?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };

    let listed: HashSet<i64> = ids.iter().copied().collect();
    let mut deleted = 0;
    {
        let mut delete = tx.prepare("DELETE FROM call_history WHERE call_id = ?1")?;
        for id in stored.keys().filter(|id| !listed.contains(id)) {
            deleted += delete.execute([id])?;
        }
    }

    let wanted = ids
        .iter()
        .zip(last_updated)
        .filter(|(id, time)| every_listed || stored.get(id) != Some(time))
        .map(|(id, _)| *id)
        .collect();
    Ok(Synchronized { deleted, wanted })
}

/// Applies an item message: its deletes, then its upserts (every column replaced).
pub fn apply_items(tx: &Transaction<'_>, items: &Items) -> rusqlite::Result<()> {
    let mut delete = tx.prepare("DELETE FROM call_history WHERE call_id = ?1")?;
    for id in &items.deletes {
        delete.execute([id])?;
    }

    let mut upsert = tx.prepare(
        "INSERT INTO call_history (call_id, phone_number, duration, call_type, start_time, is_read, \
         phone_account_id, last_updated_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(call_id) DO UPDATE SET phone_number = excluded.phone_number, duration = excluded.duration, \
         call_type = excluded.call_type, start_time = excluded.start_time, is_read = excluded.is_read, \
         phone_account_id = excluded.phone_account_id, last_updated_time = excluded.last_updated_time",
    )?;
    for e in &items.upserts {
        upsert.execute(params![
            e.id,
            e.number,
            e.duration,
            e.call_type,
            e.date,
            e.read,
            e.phone_account_id,
            e.last_updated
        ])?;
    }
    Ok(())
}

/// Writes a transaction's completion: the sync time, and `current` when a sequence number came.
pub fn complete(tx: &Transaction<'_>, now_ms: i64, sequence: Option<i64>) -> rusqlite::Result<()> {
    tx.execute("UPDATE state SET last_updated_ms = ?1 WHERE id = 1", [now_ms])?;
    if let Some(s) = sequence {
        tx.execute("UPDATE sequence SET current = ?1 WHERE id = 1", [s])?;
    }
    Ok(())
}

fn call_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
    Ok(CallRow {
        id: r.get(0)?,
        number: r.get(1)?,
        duration: r.get(2)?,
        call_type: r.get(3)?,
        date: r.get(4)?,
        read: r.get(5)?,
        phone_account_id: r.get(6)?,
        last_updated: r.get(7)?,
    })
}

/// Every stored call, newest start time first (by id among equal times).
pub fn history(db: &FeatureDb) -> Result<Vec<CallRow>, StoreError> {
    db.read(|c| {
        let mut stmt = c.prepare(
            "SELECT call_id, phone_number, duration, call_type, start_time, is_read, phone_account_id, \
             last_updated_time FROM call_history ORDER BY start_time DESC, call_id DESC",
        )?;
        stmt.query_map([], call_row)?.collect()
    })
}

/// The history is empty.
pub fn is_empty(db: &FeatureDb) -> Result<bool, StoreError> {
    db.read(|c| c.query_row("SELECT NOT EXISTS (SELECT 1 FROM call_history)", [], |r| r.get(0)))
}

pub fn sync_state(db: &FeatureDb) -> Result<SyncState, StoreError> {
    db.read(|c| {
        let (current, expected) =
            c.query_row("SELECT current, expected FROM sequence WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let last_updated_ms = c.query_row("SELECT last_updated_ms FROM state WHERE id = 1", [], |r| r.get(0))?;
        Ok(SyncState { current, expected, last_updated_ms })
    })
}

/// Stores the connect reply's `expected`.
pub fn set_expected(db: &FeatureDb, expected: i64) -> Result<(), StoreError> {
    db.read(|c| c.execute("UPDATE sequence SET expected = ?1 WHERE id = 1", [expected]).map(|_| ()))
}

fn read_state(c: &Connection) -> rusqlite::Result<PhoneState> {
    c.query_row("SELECT pc_consent, auto_refresh_done, badge_mark_ms, call_control FROM state WHERE id = 1", [], |r| {
        Ok(PhoneState {
            pc_consent: r.get(0)?,
            call_control: r.get(3)?,
            auto_refresh_done: r.get(1)?,
            badge_mark_ms: r.get(2)?,
        })
    })
}

pub fn state(db: &FeatureDb) -> Result<PhoneState, StoreError> {
    db.read(read_state)
}

/// Sets "use calls on this Mac"; `true` when it changed.
pub fn set_pc_consent(db: &FeatureDb, consent: bool) -> Result<bool, StoreError> {
    db.read(|c| c.execute("UPDATE state SET pc_consent = ?1 WHERE id = 1 AND pc_consent != ?1", [consent]))
        .map(|n| n > 0)
}

/// Sets "make and answer calls on this Mac"; `true` when it changed.
pub fn set_call_control(db: &FeatureDb, enabled: bool) -> Result<bool, StoreError> {
    db.read(|c| c.execute("UPDATE state SET call_control = ?1 WHERE id = 1 AND call_control != ?1", [enabled]))
        .map(|n| n > 0)
}

/// Claims the one automatic refresh of an empty history: `true` when it was not claimed before.
pub fn claim_auto_refresh(db: &FeatureDb) -> Result<bool, StoreError> {
    db.read(|c| c.execute("UPDATE state SET auto_refresh_done = 1 WHERE id = 1 AND auto_refresh_done = 0", []))
        .map(|n| n > 0)
}

/// The missed calls that started after the badge mark (all of them before the first mark).
pub fn unseen_missed(db: &FeatureDb) -> Result<u32, StoreError> {
    db.read(|c| {
        let mark: Option<i64> = c.query_row("SELECT badge_mark_ms FROM state WHERE id = 1", [], |r| r.get(0))?;
        c.query_row(
            "SELECT COUNT(*) FROM call_history WHERE call_type = ?1 AND start_time > ?2",
            params![TYPE_MISSED, mark.unwrap_or(i64::MIN)],
            |r| r.get(0),
        )
    })
}

/// Moves the badge mark to the newest stored start time; `true` when it moved. An empty history
/// leaves it.
pub fn mark_seen(db: &FeatureDb) -> Result<bool, StoreError> {
    db.read(|c| {
        let newest: Option<i64> = c
            .query_row("SELECT MAX(start_time) FROM call_history", [], |r| r.get::<_, Option<i64>>(0))
            .optional()?
            .flatten();
        let Some(newest) = newest else { return Ok(false) };
        c.execute(
            "UPDATE state SET badge_mark_ms = ?1 WHERE id = 1 AND (badge_mark_ms IS NULL OR badge_mark_ms != ?1)",
            [newest],
        )
        .map(|n| n > 0)
    })
}

#[cfg(test)]
mod tests;
