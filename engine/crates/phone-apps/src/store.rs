//! The phone-apps store: one SQLite database per phone holding the app table, the recent table, the
//! `phoneApps` / `recentApps` sequence pairs and the app list's sync time.
//!
//! The app table's `app_id` is its primary key and `package_name` its unique key, as the phone's ids
//! are: an upsert is keyed by package name, so an item whose id another package holds fails the
//! message's transaction. Times are Unix milliseconds except favorite ranks, which are UTC ticks
//! (100 ns since 0001-01-01), `-1` for "not a favorite".

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::wire::{AppItem, IdList};

/// The feature name of the store files.
pub const FEATURE: &str = "phone_apps";

/// Schema migrations, applied in order.
pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE apps (
    app_id INTEGER PRIMARY KEY,
    package_name TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL,
    checksum INTEGER,
    icon BLOB,
    favorite_rank INTEGER NOT NULL DEFAULT -1
);
CREATE TABLE recent (
    recent_id INTEGER PRIMARY KEY,
    package_name TEXT NOT NULL,
    name TEXT NOT NULL,
    checksum INTEGER,
    rank INTEGER NOT NULL,
    intent_action TEXT NOT NULL,
    task_id INTEGER NOT NULL,
    web_uri TEXT,
    desktop_uri TEXT
);
CREATE TABLE sequence (
    content_type TEXT PRIMARY KEY,
    current INTEGER NOT NULL DEFAULT -1,
    expected INTEGER NOT NULL DEFAULT -1
);
INSERT INTO sequence (content_type) VALUES ('phoneApps'), ('recentApps');
CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_sync_time INTEGER
);
INSERT INTO state (id) VALUES (1);
"#];

/// The two sequence pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    PhoneApps,
    RecentApps,
}

impl Content {
    fn name(self) -> &'static str {
        match self {
            Content::PhoneApps => "phoneApps",
            Content::RecentApps => "recentApps",
        }
    }
}

/// One `current` / `expected` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SequencePair {
    pub current: i64,
    pub expected: i64,
}

/// What the store says about the sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    pub phone_apps: SequencePair,
    pub recent_apps: SequencePair,
    /// The app list's last completed sync, Unix ms.
    pub last_sync_ms: Option<i64>,
}

impl SyncState {
    /// Up to date: `current ≥ expected` for both pairs.
    pub fn up_to_date(&self) -> bool {
        self.phone_apps.current >= self.phone_apps.expected && self.recent_apps.current >= self.recent_apps.expected
    }
}

/// One stored app without its icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRow {
    pub app_id: i64,
    pub package_name: String,
    pub name: String,
    pub version: String,
    pub last_updated: i64,
    pub checksum: Option<i64>,
    pub has_icon: bool,
    pub favorite_rank: i64,
}

/// One stored recent entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentRow {
    pub recent_id: i64,
    pub package_name: String,
    pub name: String,
    pub checksum: Option<i64>,
    pub rank: i32,
    pub intent_action: String,
    pub task_id: i64,
    pub web_uri: Option<String>,
    pub desktop_uri: Option<String>,
}

const APP_COLUMNS: &str =
    "app_id, package_name, name, version, last_updated_time, checksum, icon IS NOT NULL, favorite_rank";

fn app_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AppRow> {
    Ok(AppRow {
        app_id: r.get(0)?,
        package_name: r.get(1)?,
        name: r.get(2)?,
        version: r.get(3)?,
        last_updated: r.get(4)?,
        checksum: r.get(5)?,
        has_icon: r.get(6)?,
        favorite_rank: r.get(7)?,
    })
}

const RECENT_COLUMNS: &str = "recent.recent_id, recent.package_name, recent.name, recent.checksum, recent.rank, \
     recent.intent_action, recent.task_id, recent.web_uri, recent.desktop_uri";

fn recent_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RecentRow> {
    Ok(RecentRow {
        recent_id: r.get(0)?,
        package_name: r.get(1)?,
        name: r.get(2)?,
        checksum: r.get(3)?,
        rank: r.get(4)?,
        intent_action: r.get(5)?,
        task_id: r.get(6)?,
        web_uri: r.get(7)?,
        desktop_uri: r.get(8)?,
    })
}

// ------------------------------------------------------------------------------------------------
// Reads
// ------------------------------------------------------------------------------------------------

pub fn sync_state(db: &FeatureDb) -> Result<SyncState, StoreError> {
    db.read(|c| {
        let pair = |content: Content| {
            c.query_row("SELECT current, expected FROM sequence WHERE content_type = ?1", [content.name()], |r| {
                Ok(SequencePair { current: r.get(0)?, expected: r.get(1)? })
            })
        };

        Ok(SyncState {
            phone_apps: pair(Content::PhoneApps)?,
            recent_apps: pair(Content::RecentApps)?,
            last_sync_ms: c.query_row("SELECT last_sync_time FROM state WHERE id = 1", [], |r| r.get(0))?,
        })
    })
}

/// Every stored app, in package-name order.
pub fn apps(db: &FeatureDb) -> Result<Vec<AppRow>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare(&format!("SELECT {APP_COLUMNS} FROM apps ORDER BY package_name"))?;
        st.query_map([], app_row)?.collect()
    })
}

fn app_by_package_in(c: &Connection, package: &str) -> rusqlite::Result<Option<AppRow>> {
    c.query_row(&format!("SELECT {APP_COLUMNS} FROM apps WHERE package_name = ?1"), [package], app_row).optional()
}

/// The stored app of `package` (exact match).
pub fn app_by_package(db: &FeatureDb, package: &str) -> Result<Option<AppRow>, StoreError> {
    db.read(|c| app_by_package_in(c, package))
}

/// The icon bytes of `package`'s app.
pub fn icon(db: &FeatureDb, package: &str) -> Result<Option<Vec<u8>>, StoreError> {
    db.read(|c| {
        c.query_row("SELECT icon FROM apps WHERE package_name = ?1", [package], |r| r.get::<_, Option<Vec<u8>>>(0))
            .optional()
            .map(Option::flatten)
    })
}

/// The recent entries whose package has a stored app (exact match), by rank ascending.
pub fn recent_with_apps(db: &FeatureDb) -> Result<Vec<RecentRow>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare(&format!(
            "SELECT {RECENT_COLUMNS} FROM recent JOIN apps ON recent.package_name = apps.package_name \
             ORDER BY recent.rank ASC, recent.recent_id ASC"
        ))?;
        st.query_map([], recent_row)?.collect()
    })
}

#[cfg(feature = "test-support")]
/// Every stored recent entry, by id.
pub fn recent(db: &FeatureDb) -> Result<Vec<RecentRow>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare(&format!("SELECT {RECENT_COLUMNS} FROM recent ORDER BY recent_id"))?;
        st.query_map([], recent_row)?.collect()
    })
}

// ------------------------------------------------------------------------------------------------
// Sync writes (inside the message's transaction)
// ------------------------------------------------------------------------------------------------

/// The stored favorite ranks by package (`favorite_rank != -1`).
pub fn favorite_ranks(tx: &Transaction<'_>) -> rusqlite::Result<HashMap<String, i64>> {
    let mut st = tx.prepare("SELECT package_name, favorite_rank FROM apps WHERE favorite_rank != -1")?;
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
}

/// What a reconciliation did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Synchronized {
    /// The ids the PC needs, in the phone's order, once per entry.
    pub requested: Vec<i64>,
    /// How many stored rows the phone no longer lists.
    pub deleted: usize,
}

/// Deletes every row of `table` whose id is not in `list`, and returns the entries of `list` that
/// have no row, no stored checksum or another one.
fn synchronize(tx: &Transaction<'_>, table: &str, id_column: &str, list: &IdList) -> rusqlite::Result<Synchronized> {
    let stored: HashMap<i64, Option<i64>> = {
        let mut st = tx.prepare(&format!("SELECT {id_column}, checksum FROM {table}"))?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };

    let listed: HashSet<i64> = list.ids.iter().copied().collect();
    let mut deleted = 0;
    for id in stored.keys().filter(|id| !listed.contains(id)) {
        deleted += tx.execute(&format!("DELETE FROM {table} WHERE {id_column} = ?1"), [id])?;
    }

    let requested = list
        .ids
        .iter()
        .zip(&list.checksums)
        .filter(|(id, checksum)| !matches!(stored.get(id), Some(Some(c)) if c == *checksum))
        .map(|(id, _)| *id)
        .collect();
    Ok(Synchronized { requested, deleted })
}

pub fn synchronize_apps(tx: &Transaction<'_>, list: &IdList) -> rusqlite::Result<Synchronized> {
    synchronize(tx, "apps", "app_id", list)
}

pub fn synchronize_recent(tx: &Transaction<'_>, list: &IdList) -> rusqlite::Result<Synchronized> {
    synchronize(tx, "recent", "recent_id", list)
}

/// Upserts `item` by package name, with favorite rank `rank`; a missing icon clears the stored one.
///
/// The row is overwritten even when only its icon appeared or went away, which Phone Link on Windows
/// leaves as it was.
pub fn upsert_app(tx: &Transaction<'_>, item: &AppItem, rank: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO apps (app_id, package_name, name, version, last_updated_time, checksum, icon, favorite_rank) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(package_name) DO UPDATE SET app_id = excluded.app_id, name = excluded.name, \
         version = excluded.version, last_updated_time = excluded.last_updated_time, \
         checksum = excluded.checksum, icon = excluded.icon, favorite_rank = excluded.favorite_rank",
        params![item.id, item.package_name, item.name, item.version, item.last_updated, item.checksum, item.icon, rank],
    )?;
    Ok(())
}

/// The favorite rank an upserted package keeps: the snapshot's, else its row's, else `-1`.
pub fn kept_rank(tx: &Transaction<'_>, snapshot: &HashMap<String, i64>, package: &str) -> rusqlite::Result<i64> {
    if let Some(rank) = snapshot.get(package) {
        return Ok(*rank);
    }
    Ok(app_by_package_in(tx, package)?.map_or(-1, |r| r.favorite_rank))
}

/// Deletes the apps of `ids`; the number deleted.
pub fn delete_apps(tx: &Transaction<'_>, ids: &[i64]) -> rusqlite::Result<usize> {
    let mut n = 0;
    for id in ids {
        n += tx.execute("DELETE FROM apps WHERE app_id = ?1", [id])?;
    }
    Ok(n)
}

/// `package`'s app, read inside the message's transaction.
pub fn app_in(tx: &Transaction<'_>, package: &str) -> rusqlite::Result<Option<AppRow>> {
    app_by_package_in(tx, package)
}

/// Upserts one recent entry by id.
pub fn upsert_recent(tx: &Transaction<'_>, row: &RecentRow) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO recent (recent_id, package_name, name, checksum, rank, intent_action, task_id, web_uri, desktop_uri) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
         ON CONFLICT(recent_id) DO UPDATE SET package_name = excluded.package_name, name = excluded.name, \
         checksum = excluded.checksum, rank = excluded.rank, intent_action = excluded.intent_action, \
         task_id = excluded.task_id, web_uri = excluded.web_uri, desktop_uri = excluded.desktop_uri",
        params![
            row.recent_id,
            row.package_name,
            row.name,
            row.checksum,
            row.rank,
            row.intent_action,
            row.task_id,
            row.web_uri,
            row.desktop_uri
        ],
    )?;
    Ok(())
}

/// Deletes every recent entry; the number deleted.
pub fn clear_recent(tx: &Transaction<'_>) -> rusqlite::Result<usize> {
    tx.execute("DELETE FROM recent", [])
}

pub fn set_current(tx: &Transaction<'_>, content: Content, current: i64) -> rusqlite::Result<()> {
    tx.execute("UPDATE sequence SET current = ?1 WHERE content_type = ?2", params![current, content.name()])?;
    Ok(())
}

pub fn record_sync_time(tx: &Transaction<'_>, now_ms: i64) -> rusqlite::Result<()> {
    tx.execute("UPDATE state SET last_sync_time = ?1 WHERE id = 1", [now_ms])?;
    Ok(())
}

// ------------------------------------------------------------------------------------------------
// Other writes
// ------------------------------------------------------------------------------------------------

pub fn set_expected(db: &FeatureDb, content: Content, expected: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute("UPDATE sequence SET expected = ?1 WHERE content_type = ?2", params![expected, content.name()])?;
        Ok(())
    })
}

/// Sets `package`'s favorite rank; `false` when no app has that package.
pub fn set_favorite_rank(db: &FeatureDb, package: &str, rank: i64) -> Result<bool, StoreError> {
    db.transaction(|tx| {
        Ok(tx.execute("UPDATE apps SET favorite_rank = ?1 WHERE package_name = ?2", params![rank, package])? > 0)
    })
}

/// Sets the ranks `start + i` of `packages[i]` in one transaction; `Err(package)` (nothing written)
/// for the first package no app has.
pub fn set_favorite_ranks(db: &FeatureDb, packages: &[String], start: i64) -> Result<Result<(), String>, StoreError> {
    enum Failed {
        Store(StoreError),
        Unknown(String),
    }

    impl From<rusqlite::Error> for Failed {
        fn from(e: rusqlite::Error) -> Self {
            Failed::Store(e.into())
        }
    }

    let written = db.transaction(|tx| {
        for (i, package) in packages.iter().enumerate() {
            let rank = start.saturating_add(i as i64);
            if tx.execute("UPDATE apps SET favorite_rank = ?1 WHERE package_name = ?2", params![rank, package])? == 0 {
                return Err(Failed::Unknown(package.clone()));
            }
        }
        Ok(())
    });

    match written {
        Ok(()) => Ok(Ok(())),
        // Rolled back: nothing written.
        Err(Failed::Unknown(package)) => Ok(Err(package)),
        Err(Failed::Store(e)) => Err(e),
    }
}
