//! The per-phone store of phone files.
//!
//! It covers the phone's state and configuration, the **golden state** (one row per phone item, by
//! its `MediaStore` id) with the sequence it was synced to, the folders the tree adds between the
//! root and an item (synthetic folders), the thumbnail cache, and a change journal for the file
//! manager: every change takes a new **generation**, and removed entries leave a tombstone, so the
//! file manager can ask for what changed since the generation it last saw.

use std::collections::{BTreeSet, HashMap};

use prost::Message;
use rusqlite::{OptionalExtension, Transaction, params};

use super::path::{self, Location};
use phonelink_features::shared::db::{FeatureDb, StoreError};
use phonelink_proto::phone_files::v1::FilesConfigurationPayload;

/// The feature's name: its store and its notification ids (`phone_files/…`).
pub const FEATURE: &str = "phone_files";

pub const MIGRATIONS: &[&str] = &["CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        enabled INTEGER NOT NULL,
        permission_granted INTEGER NOT NULL,
        config BLOB,
        storage_free INTEGER NOT NULL,
        storage_total INTEGER NOT NULL,
        sequence INTEGER NOT NULL,
        initial_sync_done INTEGER NOT NULL,
        last_full_refresh INTEGER,
        last_user_interaction INTEGER,
        recycle_notice_off INTEGER NOT NULL,
        generation INTEGER NOT NULL,
        base_generation INTEGER NOT NULL
    );
    CREATE TABLE items (
        id INTEGER PRIMARY KEY,
        uri TEXT NOT NULL,
        name TEXT NOT NULL,
        location TEXT NOT NULL,
        location_key TEXT NOT NULL,
        file_type INTEGER NOT NULL,
        size INTEGER NOT NULL,
        created INTEGER,
        modified INTEGER,
        checksum INTEGER NOT NULL,
        is_directory INTEGER NOT NULL,
        generation INTEGER NOT NULL
    );
    CREATE INDEX items_location ON items (location_key);
    CREATE INDEX items_generation ON items (generation);
    CREATE TABLE folders (
        location_key TEXT PRIMARY KEY,
        location TEXT NOT NULL,
        generation INTEGER NOT NULL
    );
    CREATE TABLE tombstones (
        identifier TEXT PRIMARY KEY,
        generation INTEGER NOT NULL
    );
    CREATE TABLE thumbnails (
        id INTEGER PRIMARY KEY,
        checksum INTEGER NOT NULL,
        size INTEGER NOT NULL,
        jpeg BLOB NOT NULL
    );"];

/// One phone's state.
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneData {
    /// The phone's latest configuration says it serves phone files.
    pub capable: bool,
    /// The user's switch for this phone.
    pub enabled: bool,
    /// The phone holds the files permission.
    pub permission_granted: bool,
    /// The phone's latest configuration.
    pub config: Option<FilesConfigurationPayload>,
    pub storage_free: i64,
    pub storage_total: i64,
    /// The last applied sequence number; `-1` = none.
    pub sequence: i64,
    pub initial_sync_done: bool,
    /// Unix seconds.
    pub last_full_refresh: Option<i64>,
    pub last_user_interaction: Option<i64>,
    /// The user chose not to be told again where deleted files go.
    pub recycle_notice_off: bool,
}

impl Default for PhoneData {
    fn default() -> Self {
        Self {
            capable: false,
            enabled: false,
            permission_granted: false,
            config: None,
            storage_free: 0,
            storage_total: 0,
            sequence: -1,
            initial_sync_done: false,
            last_full_refresh: None,
            last_user_interaction: None,
            recycle_notice_off: false,
        }
    }
}

/// One item of the golden state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The phone's `MediaStore` id.
    pub id: i64,
    /// `content://media/external/file/<id>`: names the item in requests.
    pub uri: String,
    pub name: String,
    pub location: Location,
    /// `phone_files.v1.FileType`.
    pub file_type: i32,
    pub size: i64,
    /// Unix seconds; `None` when the phone sent none.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    /// The phone's version stamp of the item.
    pub checksum: i64,
    pub is_directory: bool,
}

/// What changed since a generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub rows: Vec<Row>,
    /// Synthetic folders added or changed (their locations).
    pub folders: Vec<Location>,
    /// Identifiers of the entries removed.
    pub removed: Vec<String>,
    pub generation: i64,
}

/// An entry of the tree as its identifier names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Row(Row),
    Folder(Location),
}

/// The identifier prefix of a synthetic folder; the location follows.
pub const FOLDER_PREFIX: &str = "dir:";

pub fn row_identifier(id: i64) -> String {
    id.to_string()
}

pub fn folder_identifier(location: &str) -> String {
    format!("{FOLDER_PREFIX}{location}")
}

/// The store of one phone.
pub struct Store {
    db: std::sync::Arc<FeatureDb>,
}

fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        uri: r.get(1)?,
        name: r.get(2)?,
        location: r.get(3)?,
        file_type: r.get(4)?,
        size: r.get(5)?,
        created: r.get(6)?,
        modified: r.get(7)?,
        checksum: r.get(8)?,
        is_directory: r.get(9)?,
    })
}

const ROW_COLUMNS: &str = "id, uri, name, location, file_type, size, created, modified, checksum, is_directory";

fn next_generation(tx: &Transaction<'_>) -> rusqlite::Result<i64> {
    ensure_state(tx)?;
    tx.execute("UPDATE phone_state SET generation = generation + 1 WHERE id = 0", [])?;
    tx.query_row("SELECT generation FROM phone_state WHERE id = 0", [], |r| r.get(0))
}

fn ensure_state(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let d = PhoneData::default();
    tx.execute(
        "INSERT OR IGNORE INTO phone_state (id, capable, enabled, permission_granted, config, storage_free,
             storage_total, sequence, initial_sync_done, last_full_refresh, last_user_interaction,
             recycle_notice_off, generation, base_generation)
         VALUES (0, ?1, ?2, ?3, NULL, 0, 0, ?4, ?5, NULL, NULL, ?6, 0, 0)",
        params![d.capable, d.enabled, d.permission_granted, d.sequence, d.initial_sync_done, d.recycle_notice_off],
    )?;
    Ok(())
}

fn tombstone(tx: &Transaction<'_>, identifier: &str, generation: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO tombstones (identifier, generation) VALUES (?1, ?2)
         ON CONFLICT (identifier) DO UPDATE SET generation = excluded.generation",
        params![identifier, generation],
    )?;
    Ok(())
}

fn revive(tx: &Transaction<'_>, identifier: &str) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM tombstones WHERE identifier = ?1", params![identifier])?;
    Ok(())
}

fn row_in(tx: &rusqlite::Connection, id: i64) -> rusqlite::Result<Option<Row>> {
    tx.query_row(&format!("SELECT {ROW_COLUMNS} FROM items WHERE id = ?1"), params![id], read_row).optional()
}

fn write_row(tx: &Transaction<'_>, row: &Row, generation: i64) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO items (id, uri, name, location, location_key, file_type, size, created, modified, checksum,
             is_directory, generation)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET uri = excluded.uri, name = excluded.name, location = excluded.location,
             location_key = excluded.location_key, file_type = excluded.file_type, size = excluded.size,
             created = excluded.created, modified = excluded.modified, checksum = excluded.checksum,
             is_directory = excluded.is_directory, generation = excluded.generation",
        params![
            row.id,
            row.uri,
            row.name,
            row.location,
            path::key(&row.location),
            row.file_type,
            row.size,
            row.created,
            row.modified,
            row.checksum,
            row.is_directory,
            generation
        ],
    )?;
    revive(tx, &row_identifier(row.id))
}

/// Re-reports the direct children of `location`: their parent's identifier changed.
fn touch_children(tx: &Transaction<'_>, location: &str, generation: i64) -> rusqlite::Result<()> {
    let prefix = format!("{}\\", path::key(location));
    let mut stmt = tx.prepare("SELECT id, location_key FROM items WHERE substr(location_key, 1, ?1) = ?2")?;
    let ids: Vec<i64> = stmt
        .query_map(params![prefix.chars().count() as i64, prefix], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .filter_map(Result::ok)
        .filter(|(_, k)| !k[prefix.len()..].contains('\\'))
        .map(|(id, _)| id)
        .collect();
    drop(stmt);

    for id in ids {
        tx.execute("UPDATE items SET generation = ?1 WHERE id = ?2", params![generation, id])?;
    }

    tx.execute(
        "UPDATE folders SET generation = ?1 WHERE substr(location_key, 1, ?2) = ?3 AND instr(substr(location_key, ?2 + 1), '\\') = 0",
        params![generation, prefix.chars().count() as i64, prefix],
    )?;
    Ok(())
}

/// Moves the rows inside a folder that moved from `from` to `to`.
fn move_descendants(tx: &Transaction<'_>, from: &str, to: &str, generation: i64) -> rusqlite::Result<()> {
    let prefix = format!("{}\\", path::key(from));
    let mut stmt = tx.prepare(&format!("SELECT {ROW_COLUMNS} FROM items WHERE substr(location_key, 1, ?1) = ?2"))?;
    let rows: Vec<Row> =
        stmt.query_map(params![prefix.chars().count() as i64, prefix], read_row)?.filter_map(Result::ok).collect();
    drop(stmt);

    for mut row in rows {
        let rest: String = row.location.chars().skip(from.chars().count()).collect();
        row.location = format!("{to}{rest}");
        write_row(tx, &row, generation)?;
    }
    Ok(())
}

/// Brings the synthetic folders in line with the rows: every folder between the root and a row that
/// has no row of its own. Their changes take `generation`; a folder that gains or loses its row
/// re-reports its children.
fn reconcile_folders(tx: &Transaction<'_>, generation: i64) -> rusqlite::Result<()> {
    let mut stmt = tx.prepare("SELECT location FROM items")?;
    let rows: Vec<String> = stmt.query_map([], |r| r.get(0))?.filter_map(Result::ok).collect();
    drop(stmt);

    let row_keys: BTreeSet<String> = rows.iter().map(|l| path::key(l)).collect();
    let mut wanted: HashMap<String, String> = HashMap::new();

    for location in &rows {
        let mut folder = path::parent(location);
        while !folder.is_empty() {
            let k = path::key(folder);
            // A row's ancestors come from that row's own walk; a folder already wanted was walked.
            if row_keys.contains(&k) || wanted.insert(k, folder.to_owned()).is_some() {
                break;
            }
            folder = path::parent(folder);
        }
    }

    let mut stmt = tx.prepare("SELECT location_key, location FROM folders")?;
    let existing: HashMap<String, String> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(Result::ok).collect();
    drop(stmt);

    for (k, location) in &existing {
        if !wanted.contains_key(k) {
            tx.execute("DELETE FROM folders WHERE location_key = ?1", params![k])?;
            tombstone(tx, &folder_identifier(location), generation)?;
            touch_children(tx, location, generation)?;
        }
    }

    for (k, location) in &wanted {
        if !existing.contains_key(k) {
            tx.execute(
                "INSERT INTO folders (location_key, location, generation) VALUES (?1, ?2, ?3)",
                params![k, location, generation],
            )?;
            revive(tx, &folder_identifier(location))?;
            touch_children(tx, location, generation)?;
        }
    }
    Ok(())
}

/// Writes `row` (insert or update) with `generation`, moving a folder's content along when its
/// location changed. `true` when anything changed.
fn upsert(tx: &Transaction<'_>, row: &Row, generation: i64) -> rusqlite::Result<bool> {
    let before = row_in(tx, row.id)?;
    if before.as_ref() == Some(row) {
        return Ok(false);
    }

    if let Some(b) = &before {
        if b.is_directory && path::key(&b.location) != path::key(&row.location) {
            move_descendants(tx, &b.location, &row.location, generation)?;
        }
    }

    write_row(tx, row, generation)?;
    if row.is_directory && before.is_none() {
        touch_children(tx, &row.location, generation)?;
    }
    Ok(true)
}

fn remove(tx: &Transaction<'_>, id: i64, generation: i64) -> rusqlite::Result<bool> {
    let Some(before) = row_in(tx, id)? else {
        return Ok(false);
    };

    tx.execute("DELETE FROM items WHERE id = ?1", params![id])?;
    tx.execute("DELETE FROM thumbnails WHERE id = ?1", params![id])?;
    tombstone(tx, &row_identifier(id), generation)?;

    if before.is_directory {
        touch_children(tx, &before.location, generation)?;
    }
    Ok(true)
}

impl Store {
    pub fn new(db: std::sync::Arc<FeatureDb>) -> Self {
        Self { db }
    }

    pub fn data(&self) -> Result<PhoneData, StoreError> {
        let row = self.db.read(|c| {
            c.query_row(
                "SELECT capable, enabled, permission_granted, config, storage_free, storage_total, sequence,
                     initial_sync_done, last_full_refresh, last_user_interaction, recycle_notice_off
                 FROM phone_state WHERE id = 0",
                [],
                |r| {
                    Ok((
                        r.get::<_, bool>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, bool>(2)?,
                        r.get::<_, Option<Vec<u8>>>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, bool>(7)?,
                        r.get::<_, Option<i64>>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                        r.get::<_, bool>(10)?,
                    ))
                },
            )
            .optional()
        })?;

        let Some((capable, enabled, permission_granted, config, free, total, sequence, initial, full, user, notice)) =
            row
        else {
            return Ok(PhoneData::default());
        };

        let config = match config {
            None => None,
            Some(bytes) => Some(
                FilesConfigurationPayload::decode(bytes.as_slice())
                    .map_err(|e| StoreError::Corrupt(format!("stored phone-files configuration: {e}")))?,
            ),
        };

        Ok(PhoneData {
            capable,
            enabled,
            permission_granted,
            config,
            storage_free: free,
            storage_total: total,
            sequence,
            initial_sync_done: initial,
            last_full_refresh: full,
            last_user_interaction: user,
            recycle_notice_off: notice,
        })
    }

    /// Applies `f` to the phone's state; the state after it.
    pub fn update(&self, f: impl FnOnce(&mut PhoneData)) -> Result<(PhoneData, PhoneData), StoreError> {
        let before = self.data()?;
        let mut after = before.clone();
        f(&mut after);

        if after != before {
            self.db.transaction(|tx| -> Result<(), StoreError> {
                ensure_state(tx)?;
                tx.execute(
                    "UPDATE phone_state SET capable = ?1, enabled = ?2, permission_granted = ?3, config = ?4,
                         storage_free = ?5, storage_total = ?6, sequence = ?7, initial_sync_done = ?8,
                         last_full_refresh = ?9, last_user_interaction = ?10, recycle_notice_off = ?11
                     WHERE id = 0",
                    params![
                        after.capable,
                        after.enabled,
                        after.permission_granted,
                        after.config.as_ref().map(Message::encode_to_vec),
                        after.storage_free,
                        after.storage_total,
                        after.sequence,
                        after.initial_sync_done,
                        after.last_full_refresh,
                        after.last_user_interaction,
                        after.recycle_notice_off
                    ],
                )?;
                Ok(())
            })?;
        }
        Ok((before, after))
    }

    /// The current generation and the oldest one changes can be asked from.
    pub fn generations(&self) -> Result<(i64, i64), StoreError> {
        Ok(self
            .db
            .read(|c| {
                c.query_row("SELECT generation, base_generation FROM phone_state WHERE id = 0", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
            })?
            .unwrap_or((0, 0)))
    }

    /// Replaces the golden state with `rows` and sets the sequence: rows not among them are removed.
    /// `true` when anything changed.
    pub fn replace_all(&self, rows: &[Row], sequence: i64) -> Result<bool, StoreError> {
        self.db.transaction(|tx| -> Result<bool, StoreError> {
            let generation = next_generation(tx)?;

            let mut stmt = tx.prepare("SELECT id FROM items")?;
            let existing: Vec<i64> = stmt.query_map([], |r| r.get(0))?.filter_map(Result::ok).collect();
            drop(stmt);

            let keep: BTreeSet<i64> = rows.iter().map(|r| r.id).collect();
            let mut changed = false;
            for id in existing.into_iter().filter(|id| !keep.contains(id)) {
                changed |= remove(tx, id, generation)?;
            }

            for row in rows {
                changed |= upsert(tx, row, generation)?;
            }

            reconcile_folders(tx, generation)?;
            tx.execute("UPDATE phone_state SET sequence = ?1 WHERE id = 0", params![sequence])?;
            if !changed {
                tx.execute("UPDATE phone_state SET generation = generation - 1 WHERE id = 0", [])?;
            }
            Ok(changed)
        })
    }

    /// Applies an incremental sync: `upserts` (insert or update by id), then `deletes` (by id), and sets
    /// the sequence. `true` when anything changed.
    pub fn apply(&self, upserts: &[Row], deletes: &[i64], sequence: Option<i64>) -> Result<bool, StoreError> {
        self.db.transaction(|tx| -> Result<bool, StoreError> {
            let generation = next_generation(tx)?;
            let mut changed = false;

            for row in upserts {
                changed |= upsert(tx, row, generation)?;
            }

            for id in deletes {
                changed |= remove(tx, *id, generation)?;
            }

            if changed {
                reconcile_folders(tx, generation)?;
            } else {
                tx.execute("UPDATE phone_state SET generation = generation - 1 WHERE id = 0", [])?;
            }

            if let Some(sequence) = sequence {
                tx.execute("UPDATE phone_state SET sequence = ?1 WHERE id = 0", params![sequence])?;
            }
            Ok(changed)
        })
    }

    /// Empties the golden state, the folders, the journal and the thumbnails; the sequence goes back
    /// to `-1`. Changes can no longer be asked from an earlier generation.
    pub fn clear(&self) -> Result<(), StoreError> {
        self.db.transaction(|tx| -> Result<(), StoreError> {
            let generation = next_generation(tx)?;
            tx.execute_batch(
                "DELETE FROM items; DELETE FROM folders; DELETE FROM tombstones; DELETE FROM thumbnails;",
            )?;
            tx.execute("UPDATE phone_state SET sequence = -1, base_generation = ?1 WHERE id = 0", params![generation])?;
            Ok(())
        })
    }

    pub fn row(&self, id: i64) -> Result<Option<Row>, StoreError> {
        self.db.read(|c| row_in(c, id))
    }

    /// The row at `location` (compared without regard to case).
    pub fn row_at(&self, location: &str) -> Result<Option<Row>, StoreError> {
        self.db.read(|c| {
            c.query_row(
                &format!("SELECT {ROW_COLUMNS} FROM items WHERE location_key = ?1 ORDER BY id LIMIT 1"),
                params![path::key(location)],
                read_row,
            )
            .optional()
        })
    }

    /// The synthetic folder at `location`.
    pub fn folder_at(&self, location: &str) -> Result<Option<Location>, StoreError> {
        self.db.read(|c| {
            c.query_row("SELECT location FROM folders WHERE location_key = ?1", params![path::key(location)], |r| {
                r.get(0)
            })
            .optional()
        })
    }

    /// The entry an identifier names.
    pub fn entry(&self, identifier: &str) -> Result<Option<Entry>, StoreError> {
        if let Some(location) = identifier.strip_prefix(FOLDER_PREFIX) {
            return Ok(self.folder_at(location)?.map(Entry::Folder));
        }
        match identifier.parse::<i64>() {
            Ok(id) => Ok(self.row(id)?.map(Entry::Row)),
            Err(_) => Ok(None),
        }
    }

    /// The entries directly inside `location` (`""` = the root): rows and synthetic folders.
    pub fn children(&self, location: &str) -> Result<(Vec<Row>, Vec<Location>), StoreError> {
        let (prefix, len) = if location.is_empty() {
            (String::new(), 0)
        } else {
            let p = format!("{}\\", path::key(location));
            let n = p.chars().count() as i64;
            (p, n)
        };

        self.db.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM items
                 WHERE substr(location_key, 1, ?1) = ?2 AND instr(substr(location_key, ?1 + 1), '\\') = 0"
            ))?;
            let rows = stmt.query_map(params![len, prefix], read_row)?.collect::<rusqlite::Result<Vec<_>>>()?;

            let mut stmt = c.prepare(
                "SELECT location FROM folders
                 WHERE substr(location_key, 1, ?1) = ?2 AND instr(substr(location_key, ?1 + 1), '\\') = 0",
            )?;
            let folders =
                stmt.query_map(params![len, prefix], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
            Ok((rows, folders))
        })
    }

    /// Every row at `location` or inside it.
    pub fn subtree(&self, location: &str) -> Result<Vec<Row>, StoreError> {
        let k = path::key(location);
        let prefix = format!("{k}\\");
        self.db.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {ROW_COLUMNS} FROM items WHERE location_key = ?1 OR substr(location_key, 1, ?2) = ?3"
            ))?;
            stmt.query_map(params![k, prefix.chars().count() as i64, prefix], read_row)?.collect()
        })
    }

    /// What changed after `generation`: `None` when that generation is out of the journal's range (the
    /// file manager then reads everything again).
    pub fn changes_since(&self, generation: i64) -> Result<Option<Changes>, StoreError> {
        let (current, base) = self.generations()?;
        if generation < base || generation > current {
            return Ok(None);
        }

        Ok(Some(self.db.read(|c| {
            let mut stmt = c.prepare(&format!("SELECT {ROW_COLUMNS} FROM items WHERE generation > ?1"))?;
            let rows = stmt.query_map(params![generation], read_row)?.collect::<rusqlite::Result<Vec<_>>>()?;

            let mut stmt = c.prepare("SELECT location FROM folders WHERE generation > ?1")?;
            let folders = stmt.query_map(params![generation], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

            let mut stmt = c.prepare("SELECT identifier FROM tombstones WHERE generation > ?1")?;
            let removed = stmt.query_map(params![generation], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;

            Ok(Changes { rows, folders, removed, generation: current })
        })?))
    }

    /// The cached thumbnail of `id`: `(checksum, size, jpeg)`.
    pub fn thumbnail(&self, id: i64) -> Result<Option<(i64, i32, Vec<u8>)>, StoreError> {
        self.db.read(|c| {
            c.query_row("SELECT checksum, size, jpeg FROM thumbnails WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
        })
    }

    pub fn put_thumbnail(&self, id: i64, checksum: i64, size: i32, jpeg: &[u8]) -> Result<(), StoreError> {
        self.db.transaction(|tx| -> Result<(), StoreError> {
            // A thumbnail of an item no longer stored is not kept.
            if row_in(tx, id)?.is_some() {
                tx.execute(
                    "INSERT INTO thumbnails (id, checksum, size, jpeg) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (id) DO UPDATE SET checksum = excluded.checksum, size = excluded.size,
                         jpeg = excluded.jpeg",
                    params![id, checksum, size, jpeg],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
