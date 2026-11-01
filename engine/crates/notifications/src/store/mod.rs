//! The feature's per-phone persistent store (the app's own store).
//!
//! It covers the notification rows, the action queue, the settings rows and the feature's per-phone
//! values (PC permission, `NotificationsCapabilities`, "last updated", mutes, …).
//!
//! A batch's purges, upserts, removals and queue drops are one transaction ([`apply_batch`]).

use rusqlite::{OptionalExtension, Transaction, params};

use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::item::NotificationItem;
use super::settings::SettingRow;

/// The schema, applied in order (`user_version` counts them).
pub const MIGRATIONS: &[&str] = &["
    CREATE TABLE notifications (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        notification_id TEXT NOT NULL UNIQUE,
        package_name TEXT NOT NULL,
        json TEXT NOT NULL,
        post_time INTEGER NOT NULL,
        state INTEGER NOT NULL DEFAULT 0,
        anonymous_id TEXT NOT NULL
    );
    CREATE INDEX notifications_post_time ON notifications (post_time);
    CREATE TABLE phone_requests (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        request_id TEXT NOT NULL,
        external_id1 TEXT NOT NULL,
        external_id2 TEXT NOT NULL,
        request TEXT NOT NULL,
        time_added_ms INTEGER NOT NULL,
        finished INTEGER NOT NULL DEFAULT 0,
        time_finished_ms INTEGER,
        status TEXT,
        expired INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX phone_requests_external_id1 ON phone_requests (external_id1);
    CREATE TABLE settings (
        setting_group_id TEXT NOT NULL,
        setting_key TEXT NOT NULL,
        setting_type INTEGER NOT NULL,
        setting_value TEXT NOT NULL,
        setting_mute_expiration_timestamp INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (setting_group_id, setting_key)
    );
    CREATE TABLE device_data (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );
"];

/// The pinned state of a stored notification.
pub mod pinned {
    pub const NONE: i32 = 0;
    pub const PINNED: i32 = 1;
    pub const UNPINNED: i32 = 2;
}

/// Names of the per-phone values in `device_data`.
pub mod value {
    /// The user's "show my phone's notifications" switch (default on).
    pub const PC_PERMISSION: &str = "pc_permission";
    /// `NotificationsCapabilities` from the last connect response (default `0`).
    pub const CAPABILITIES: &str = "notifications_capabilities";
    /// "Notifications last updated" (Unix ms).
    pub const LAST_UPDATED_MS: &str = "last_updated_ms";
    /// Mute-all expiry (UTC ticks; `0` = not muted).
    pub const MUTE_ALL_UNTIL_TICKS: &str = "mute_all_until_ticks";
    /// Banner throttling enabled for this phone (default on).
    pub const THROTTLING_ENABLED: &str = "throttling_enabled";
    /// The quiet-by-default preference (flags) and whether the user set it.
    pub const DEFAULT_PREFERENCE: &str = "default_preference";
    pub const DEFAULT_PREFERENCE_SET: &str = "default_preference_set";
    /// A notification-permission request answered *`DeviceNotSupported`*.
    pub const LOW_RAM_DEVICE: &str = "low_ram_device";
    /// A notification-permission request answered *Success* or *Accepted*.
    pub const SETUP_COMPLETED: &str = "setup_completed";
}

/// One stored notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRow {
    pub id: i64,
    pub key: String,
    pub package_name: String,
    /// The item JSON text as received.
    pub json: String,
    pub post_time: i64,
    /// [`pinned`] state.
    pub state: i32,
    /// The local random id, kept across updates of the key.
    pub anonymous_id: String,
}

const ROW_COLUMNS: &str = "id, notification_id, package_name, json, post_time, state, anonymous_id";

fn row_of(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredRow> {
    Ok(StoredRow {
        id: r.get(0)?,
        key: r.get(1)?,
        package_name: r.get(2)?,
        json: r.get(3)?,
        post_time: r.get(4)?,
        state: r.get(5)?,
        anonymous_id: r.get(6)?,
    })
}

/// Every stored row, newest `postTime` first (insertion order among equal times).
pub fn all_rows(db: &FeatureDb) -> Result<Vec<StoredRow>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare(&format!("SELECT {ROW_COLUMNS} FROM notifications ORDER BY post_time DESC, id ASC"))?;
        st.query_map([], row_of)?.collect()
    })
}

/// The row of `key`.
pub fn row_by_key(db: &FeatureDb, key: &str) -> Result<Option<StoredRow>, StoreError> {
    db.read(|c| row_in(c, key))
}

fn row_in(c: &rusqlite::Connection, key: &str) -> rusqlite::Result<Option<StoredRow>> {
    c.query_row(&format!("SELECT {ROW_COLUMNS} FROM notifications WHERE notification_id = ?1"), [key], row_of)
        .optional()
}

/// The stored keys and their `postTime`s, newest first (full sync and the connect keys).
pub fn keys_and_post_times(db: &FeatureDb) -> Result<(Vec<String>, Vec<i64>), StoreError> {
    let rows = all_rows(db)?;
    Ok(rows.into_iter().map(|r| (r.key, r.post_time)).unzip())
}

/// Deletes the rows of `keys` (one transaction) and returns the rows that were stored.
pub fn delete_rows(db: &FeatureDb, keys: &[String]) -> Result<Vec<StoredRow>, StoreError> {
    db.transaction(|tx| {
        let mut deleted = Vec::new();
        for k in keys {
            if let Some(row) = row_in(tx, k)? {
                tx.execute("DELETE FROM notifications WHERE id = ?1", [row.id])?;
                deleted.push(row);
            }
        }
        Ok(deleted)
    })
}

/// Sets the pinned state of `key`; `false` when the key is not stored.
pub fn set_state(db: &FeatureDb, key: &str, state: i32) -> Result<bool, StoreError> {
    db.transaction(|tx| {
        Ok(tx.execute("UPDATE notifications SET state = ?1 WHERE notification_id = ?2", params![state, key])? > 0)
    })
}

/// The ASCII-case-insensitive substring test of a session change (`%`/`_` literal).
pub fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

/// What one batch changes, in application order.
#[derive(Debug, Default)]
pub struct BatchPlan<'a> {
    /// The non-empty session ids of operation 7, in array order.
    pub session_changes: Vec<&'a str>,
    /// The well-formed items of operations 1 and 3, in array order, with their operation.
    pub upserts: Vec<(&'a NotificationItem, &'a str, i32)>,
    /// The keys of operation 2, in array order.
    pub removals: Vec<&'a str>,
}

/// One upsert as applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upserted {
    pub key: String,
    pub operation: i32,
    /// No row had the key when the upserts began (after the batch's session changes).
    pub is_new: bool,
}

/// What a batch did to the store.
#[derive(Debug, Default)]
pub struct BatchEffects {
    /// Rows deleted by session changes.
    pub purged: Vec<StoredRow>,
    pub upserted: Vec<Upserted>,
    /// Rows deleted by removals.
    pub removed: Vec<StoredRow>,
}

/// Applies `plan` in one transaction.
///
/// Session changes (rows and queued requests whose key does not contain the session id), then
/// upserts (an existing key keeps its pinned state and local id), then removals with their queue
/// drops.
///
/// Nothing is kept when any step fails.
pub fn apply_batch(db: &FeatureDb, plan: &BatchPlan<'_>) -> Result<BatchEffects, StoreError> {
    db.transaction(|tx| {
        let mut effects = BatchEffects::default();
        for s in &plan.session_changes {
            effects.purged.extend(purge_outside_session(tx, s)?);
        }

        let mut stored_before: Vec<(String, bool)> = Vec::new();
        for (item, _, _) in &plan.upserts {
            let present: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM notifications WHERE notification_id = ?1)",
                [&item.key],
                |r| r.get(0),
            )?;
            stored_before.push((item.key.clone(), present));
        }

        for ((item, json, op), (_, present)) in plan.upserts.iter().zip(&stored_before) {
            let updated = tx.execute(
                "UPDATE notifications SET json = ?1, post_time = ?2 WHERE notification_id = ?3",
                params![json, item.post_time, item.key],
            )?;
            if updated == 0 {
                tx.execute(
                    "INSERT INTO notifications (notification_id, package_name, json, post_time, state, anonymous_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        item.key,
                        item.package_name,
                        json,
                        item.post_time,
                        pinned::NONE,
                        uuid::Uuid::new_v4().to_string()
                    ],
                )?;
            }
            effects.upserted.push(Upserted { key: item.key.clone(), operation: *op, is_new: !present });
        }

        for k in &plan.removals {
            if let Some(row) = row_in(tx, k)? {
                tx.execute("DELETE FROM notifications WHERE id = ?1", [row.id])?;
                effects.removed.push(row);
            }
            tx.execute("DELETE FROM phone_requests WHERE external_id1 = ?1", [k])?;
        }

        Ok(effects)
    })
}

fn purge_outside_session(tx: &Transaction<'_>, session: &str) -> Result<Vec<StoredRow>, StoreError> {
    let rows: Vec<StoredRow> = {
        let mut st = tx.prepare(&format!("SELECT {ROW_COLUMNS} FROM notifications ORDER BY id"))?;
        st.query_map([], row_of)?.collect::<rusqlite::Result<_>>()?
    };

    let mut purged = Vec::new();
    for row in rows.into_iter().filter(|r| !contains_ascii_ci(&r.key, session)) {
        tx.execute("DELETE FROM notifications WHERE id = ?1", [row.id])?;
        purged.push(row);
    }

    let queued: Vec<(i64, String)> = {
        let mut st = tx.prepare("SELECT id, external_id1 FROM phone_requests")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };

    for (id, key) in queued {
        if !contains_ascii_ci(&key, session) {
            tx.execute("DELETE FROM phone_requests WHERE id = ?1", [id])?;
        }
    }
    Ok(purged)
}

// ------------------------------------------------------------------------------------------------
// Action queue
// ------------------------------------------------------------------------------------------------

/// One queued action or reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: i64,
    /// The `correlationVector` of the request as first built (local identity only).
    pub request_id: String,
    /// The notification key (external id 1).
    pub key: String,
    /// `"inline_reply"` or the decimal `actionIndex` (external id 2).
    pub external_id2: String,
    /// The request record (JSON, local).
    pub request: String,
    pub time_added_ms: i64,
}

/// The final status of an entry: finished with an outcome, or dropped unsent after its expiry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueFinal {
    pub status: String,
    pub expired: bool,
}

pub fn enqueue(
    db: &FeatureDb,
    request_id: &str,
    key: &str,
    external_id2: &str,
    request: &str,
    time_added_ms: i64,
) -> Result<i64, StoreError> {
    db.transaction(|tx| {
        tx.execute(
            "INSERT INTO phone_requests (request_id, external_id1, external_id2, request, time_added_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![request_id, key, external_id2, request, time_added_ms],
        )?;
        Ok(tx.last_insert_rowid())
    })
}

/// The oldest entry that was neither sent nor dropped.
pub fn next_unsent(db: &FeatureDb) -> Result<Option<QueueEntry>, StoreError> {
    db.read(|c| {
        c.query_row(
            "SELECT id, request_id, external_id1, external_id2, request, time_added_ms
             FROM phone_requests WHERE finished = 0 ORDER BY id LIMIT 1",
            [],
            |r| {
                Ok(QueueEntry {
                    id: r.get(0)?,
                    request_id: r.get(1)?,
                    key: r.get(2)?,
                    external_id2: r.get(3)?,
                    request: r.get(4)?,
                    time_added_ms: r.get(5)?,
                })
            },
        )
        .optional()
    })
}

/// Records an entry's final status; `false` when the entry was deleted meanwhile.
pub fn finish(db: &FeatureDb, id: i64, status: &QueueFinal, now_ms: i64) -> Result<bool, StoreError> {
    db.transaction(|tx| {
        Ok(tx.execute(
            "UPDATE phone_requests SET finished = 1, status = ?1, expired = ?2, time_finished_ms = ?3 WHERE id = ?4",
            params![status.status, status.expired, now_ms, id],
        )? > 0)
    })
}

/// Deletes the queued requests of `keys`.
pub fn drop_queued(db: &FeatureDb, keys: &[String]) -> Result<(), StoreError> {
    db.transaction(|tx| {
        for k in keys {
            tx.execute("DELETE FROM phone_requests WHERE external_id1 = ?1", [k])?;
        }
        Ok(())
    })
}

#[cfg(feature = "test-support")]
/// The final status of the entry `id`, when it has one.
pub fn queue_final(db: &FeatureDb, id: i64) -> Result<Option<QueueFinal>, StoreError> {
    db.read(|c| {
        c.query_row("SELECT status, expired FROM phone_requests WHERE id = ?1 AND finished = 1", [id], |r| {
            Ok(QueueFinal { status: r.get(0)?, expired: r.get(1)? })
        })
        .optional()
    })
}

// ------------------------------------------------------------------------------------------------
// Settings rows
// ------------------------------------------------------------------------------------------------

pub fn setting(db: &FeatureDb, group_id: &str, key: &str) -> Result<Option<SettingRow>, StoreError> {
    db.read(|c| {
        c.query_row(
            "SELECT setting_group_id, setting_key, setting_type, setting_value, setting_mute_expiration_timestamp
             FROM settings WHERE setting_group_id = ?1 AND setting_key = ?2",
            [group_id, key],
            |r| {
                Ok(SettingRow {
                    group_id: r.get(0)?,
                    key: r.get(1)?,
                    setting_type: r.get(2)?,
                    value: r.get(3)?,
                    mute_expiration_ticks: r.get(4)?,
                })
            },
        )
        .optional()
    })
}

/// Writes `rows`, each replacing the row with its `(group_id, key)`.
pub fn put_settings(db: &FeatureDb, rows: &[SettingRow]) -> Result<(), StoreError> {
    db.transaction(|tx| {
        for r in rows {
            tx.execute(
                "INSERT OR REPLACE INTO settings
                 (setting_group_id, setting_key, setting_type, setting_value, setting_mute_expiration_timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![r.group_id, r.key, r.setting_type, r.value, r.mute_expiration_ticks],
            )?;
        }
        Ok(())
    })
}

pub fn delete_settings(db: &FeatureDb, group_id: &str, keys: &[String]) -> Result<(), StoreError> {
    db.transaction(|tx| {
        for k in keys {
            tx.execute("DELETE FROM settings WHERE setting_group_id = ?1 AND setting_key = ?2", params![group_id, k])?;
        }
        Ok(())
    })
}

// ------------------------------------------------------------------------------------------------
// Per-phone values
// ------------------------------------------------------------------------------------------------

pub fn stored_value(db: &FeatureDb, name: &str) -> Result<Option<i64>, StoreError> {
    db.read(|c| c.query_row("SELECT value FROM device_data WHERE name = ?1", [name], |r| r.get(0)).optional())
}

pub fn set_value(db: &FeatureDb, name: &str, v: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute(
            "INSERT INTO device_data (name, value) VALUES (?1, ?2)
             ON CONFLICT (name) DO UPDATE SET value = excluded.value",
            params![name, v],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}

#[cfg(test)]
mod tests;
