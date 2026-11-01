//! The photos store.
//!
//! It covers one SQLite database per phone holding the media cache, the `media_info` sequence pair
//! and the per-phone settings — the local state the protocol depends on, and its cache maintenance
//! (the app's own persistent per-phone store).
//!
//! Times are Unix milliseconds. Thumbnails and full-resolution bytes are stored only when non-empty,
//! so `IS NOT NULL` is "the item has them". Every change a message makes is one transaction.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Transaction, params};

use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::wire::{MediaItem, Metadata};

/// The feature name of the store files.
pub const FEATURE: &str = "photos";

/// Schema migrations, applied in order.
pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE media (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    last_updated_time INTEGER NOT NULL,
    taken_time INTEGER NOT NULL,
    orientation TEXT NOT NULL,
    last_seen_time INTEGER,
    mime_type TEXT NOT NULL,
    height INTEGER NOT NULL,
    width INTEGER NOT NULL,
    size INTEGER NOT NULL,
    uri TEXT NOT NULL,
    checksum INTEGER NOT NULL DEFAULT 0,
    thumbnail BLOB,
    media BLOB
);
CREATE INDEX media_display ON media (taken_time DESC, last_updated_time DESC, id DESC);
CREATE TABLE photos_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    current_sequence INTEGER NOT NULL DEFAULT -1,
    expected_sequence INTEGER NOT NULL DEFAULT -1,
    last_sync_time INTEGER,
    photos_capabilities INTEGER NOT NULL DEFAULT 0,
    consent INTEGER NOT NULL DEFAULT 1,
    deletion_allowed INTEGER NOT NULL DEFAULT 1,
    new_photo_notifications INTEGER NOT NULL DEFAULT 1,
    full_access_reminder_dismissed INTEGER NOT NULL DEFAULT 0
);
INSERT INTO photos_state (id) VALUES (1);
"#];

/// The display order: `taken DESC, last-modified DESC` (and the id, so ties are stable).
const DISPLAY_ORDER: &str = "ORDER BY taken_time DESC, last_updated_time DESC, id DESC";

/// Insert, or overwrite the fields, checksum and thumbnail only when the last-modified time, the
/// taken time or the URI changed, or the cached item has no thumbnail; `last_seen_time` is written
/// only on insert.
const UPSERT_SQL: &str = "INSERT INTO media (id, name, last_updated_time, taken_time, orientation, last_seen_time, \
     mime_type, height, width, size, uri, checksum, thumbnail) \
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13) \
     ON CONFLICT(id) DO UPDATE SET name = excluded.name, last_updated_time = excluded.last_updated_time, \
     taken_time = excluded.taken_time, orientation = excluded.orientation, mime_type = excluded.mime_type, \
     height = excluded.height, width = excluded.width, size = excluded.size, uri = excluded.uri, \
     checksum = excluded.checksum, thumbnail = excluded.thumbnail \
     WHERE last_updated_time != excluded.last_updated_time OR taken_time != excluded.taken_time \
     OR uri != excluded.uri OR thumbnail IS NULL";

/// One cached item without its blobs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoRow {
    pub id: i64,
    pub name: String,
    pub modified_ms: i64,
    pub taken_ms: i64,
    pub orientation: String,
    pub last_seen_ms: Option<i64>,
    pub mime_type: String,
    pub height: i32,
    pub width: i32,
    pub size: i64,
    pub uri: String,
    pub checksum: i64,
    pub has_thumbnail: bool,
    pub has_full: bool,
}

/// The per-phone state row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhotosState {
    pub current_sequence: i64,
    pub expected_sequence: i64,
    pub last_sync_ms: Option<i64>,
    pub capabilities: u32,
    pub consent: bool,
    pub deletion_allowed: bool,
    pub new_photo_notifications: bool,
    pub full_access_reminder_dismissed: bool,
}

impl PhotosState {
    /// "Up to date" = `current ≥ expected`.
    pub fn up_to_date(&self) -> bool {
        self.current_sequence >= self.expected_sequence
    }
}

/// A PC-side setting of the state row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Consent,
    DeletionAllowed,
    NewPhotoNotifications,
    FullAccessReminderDismissed,
}

impl Setting {
    fn column(self) -> &'static str {
        match self {
            Setting::Consent => "consent",
            Setting::DeletionAllowed => "deletion_allowed",
            Setting::NewPhotoNotifications => "new_photo_notifications",
            Setting::FullAccessReminderDismissed => "full_access_reminder_dismissed",
        }
    }
}

/// What a metadata message did to the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// The pending ids in the phone's order (`outgoing`).
    pub outgoing: Vec<i64>,
    /// Cached items deleted because the phone no longer lists them.
    pub deleted: Vec<i64>,
    /// Whether the capabilities changed.
    pub capabilities_changed: bool,
}

/// How an item batch's completion is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Completion {
    pub now_ms: i64,
    /// `current` := this when the sync carried a sequence number.
    pub sequence_number: Option<i64>,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PhotoRow> {
    Ok(PhotoRow {
        id: r.get(0)?,
        name: r.get(1)?,
        modified_ms: r.get(2)?,
        taken_ms: r.get(3)?,
        orientation: r.get(4)?,
        last_seen_ms: r.get(5)?,
        mime_type: r.get(6)?,
        height: r.get(7)?,
        width: r.get(8)?,
        size: r.get(9)?,
        uri: r.get(10)?,
        checksum: r.get(11)?,
        has_thumbnail: r.get(12)?,
        has_full: r.get(13)?,
    })
}

const ROW_COLUMNS: &str = "id, name, last_updated_time, taken_time, orientation, last_seen_time, mime_type, height, \
     width, size, uri, checksum, thumbnail IS NOT NULL, media IS NOT NULL";

/// Every cached item in display order.
pub fn gallery(db: &FeatureDb) -> Result<Vec<PhotoRow>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare_cached(&format!("SELECT {ROW_COLUMNS} FROM media {DISPLAY_ORDER}"))?;
        st.query_map([], row)?.collect()
    })
}

/// One cached item.
pub fn item(db: &FeatureDb, id: i64) -> Result<Option<PhotoRow>, StoreError> {
    db.read(|c| c.query_row(&format!("SELECT {ROW_COLUMNS} FROM media WHERE id = ?1"), [id], row).optional())
}

pub fn thumbnail(db: &FeatureDb, id: i64) -> Result<Option<Vec<u8>>, StoreError> {
    db.read(|c| {
        c.query_row("SELECT thumbnail FROM media WHERE id = ?1", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
            .optional()
            .map(Option::flatten)
    })
}

pub fn full_bytes(db: &FeatureDb, id: i64) -> Result<Option<Vec<u8>>, StoreError> {
    db.read(|c| {
        c.query_row("SELECT media FROM media WHERE id = ?1", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
            .optional()
            .map(Option::flatten)
    })
}

/// The cached ids that lack a thumbnail (the thumbnail dispatcher's candidates).
pub fn ids_without_thumbnail(db: &FeatureDb) -> Result<HashSet<i64>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare_cached("SELECT id FROM media WHERE thumbnail IS NULL")?;
        st.query_map([], |r| r.get(0))?.collect()
    })
}

/// The cached ids that lack full-resolution bytes (the image dispatcher's candidates).
pub fn ids_without_full(db: &FeatureDb) -> Result<HashSet<i64>, StoreError> {
    db.read(|c| {
        let mut st = c.prepare_cached("SELECT id FROM media WHERE media IS NULL")?;
        st.query_map([], |r| r.get(0))?.collect()
    })
}

pub fn state(db: &FeatureDb) -> Result<PhotosState, StoreError> {
    db.read(read_state)
}

fn read_state(c: &rusqlite::Connection) -> rusqlite::Result<PhotosState> {
    c.query_row(
        "SELECT current_sequence, expected_sequence, last_sync_time, photos_capabilities, consent, deletion_allowed, \
         new_photo_notifications, full_access_reminder_dismissed FROM photos_state WHERE id = 1",
        [],
        |r| {
            Ok(PhotosState {
                current_sequence: r.get(0)?,
                expected_sequence: r.get(1)?,
                last_sync_ms: r.get(2)?,
                capabilities: r.get::<_, i64>(3)? as u32,
                consent: r.get(4)?,
                deletion_allowed: r.get(5)?,
                new_photo_notifications: r.get(6)?,
                full_access_reminder_dismissed: r.get(7)?,
            })
        },
    )
}

pub fn set_setting(db: &FeatureDb, setting: Setting, value: bool) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute(&format!("UPDATE photos_state SET {} = ?1 WHERE id = 1", setting.column()), [value])?;
        Ok::<_, StoreError>(())
    })
}

/// Stores `expected` (connect reply).
pub fn set_expected(db: &FeatureDb, expected: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute("UPDATE photos_state SET expected_sequence = ?1 WHERE id = 1", [expected])?;
        Ok::<_, StoreError>(())
    })
}

/// Stores the photo capabilities; `true` when they changed.
pub fn set_capabilities(db: &FeatureDb, capabilities: u32) -> Result<bool, StoreError> {
    db.transaction(|tx| Ok::<_, StoreError>(write_capabilities(tx, capabilities)?))
}

fn write_capabilities(tx: &Transaction<'_>, capabilities: u32) -> rusqlite::Result<bool> {
    let n = tx.execute(
        "UPDATE photos_state SET photos_capabilities = ?1 WHERE id = 1 AND photos_capabilities != ?1",
        [i64::from(capabilities)],
    )?;
    Ok(n > 0)
}

/// Records the sync time (completion, or the state becoming up to date).
pub fn record_sync_time(db: &FeatureDb, now_ms: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute("UPDATE photos_state SET last_sync_time = ?1 WHERE id = 1", [now_ms])?;
        Ok::<_, StoreError>(())
    })
}

fn complete(tx: &Transaction<'_>, completion: Completion) -> rusqlite::Result<()> {
    tx.execute("UPDATE photos_state SET last_sync_time = ?1 WHERE id = 1", [completion.now_ms])?;
    if let Some(seq) = completion.sequence_number {
        tx.execute("UPDATE photos_state SET current_sequence = ?1 WHERE id = 1", [seq])?;
    }
    Ok(())
}

/// A metadata message, in one transaction, with its `mediaSettings` capabilities.
///
/// Delete every cached item the list omits (an empty list clears the cache), then compute the
/// pending ids — with checksums: not cached or a different checksum; without: every id except a
/// cached item that has a thumbnail and the same last-modified time.
///
/// When nothing is pending the sync completes here.
pub fn reconcile(
    db: &FeatureDb,
    m: &Metadata,
    capabilities: Option<u32>,
    now_ms: i64,
) -> Result<Reconciled, StoreError> {
    db.transaction(|tx| {
        let capabilities_changed = match capabilities {
            Some(c) => write_capabilities(tx, c)?,
            None => false,
        };

        let listed: HashSet<i64> = m.ids.iter().copied().collect();
        let cached: Vec<i64> = {
            let mut st = tx.prepare_cached("SELECT id FROM media ORDER BY id")?;
            st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };

        let deleted: Vec<i64> = cached.into_iter().filter(|id| !listed.contains(id)).collect();
        {
            let mut del = tx.prepare_cached("DELETE FROM media WHERE id = ?1")?;
            for id in &deleted {
                del.execute([id])?;
            }
        }

        let mut outgoing = Vec::new();
        {
            let mut q = tx
                .prepare_cached("SELECT checksum, last_updated_time, thumbnail IS NOT NULL FROM media WHERE id = ?1")?;
            for (i, id) in m.ids.iter().enumerate() {
                let cached: Option<(i64, i64, bool)> =
                    q.query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
                let pending = match (&m.checksums, cached) {
                    (_, None) => true,
                    (Some(sums), Some((checksum, _, _))) => checksum != sums[i],
                    (None, Some((_, modified, has_thumbnail))) => !(has_thumbnail && modified == m.times[i]),
                };
                if pending {
                    outgoing.push(*id);
                }
            }
        }

        if outgoing.is_empty() {
            complete(tx, Completion { now_ms, sequence_number: m.sequence_number })?;
        }
        Ok::<_, StoreError>(Reconciled { outgoing, deleted, capabilities_changed })
    })
}

/// Upserts `items`; returns the ids whose cache entry this wrote — inserted, overwritten, or its full
/// bytes dropped (an upsert whose overwrite condition is false writes nothing).
fn upsert(tx: &Transaction<'_>, items: &[MediaItem], now_ms: i64) -> rusqlite::Result<Vec<i64>> {
    let mut drop_full = tx.prepare_cached("UPDATE media SET media = NULL WHERE id = ?1 AND checksum != ?2")?;
    let mut up = tx.prepare_cached(UPSERT_SQL)?;
    let mut written = Vec::new();

    for it in items {
        let mut wrote = false;
        if it.thumbnail.is_some() {
            wrote |= drop_full.execute(params![it.id, it.checksum])? > 0;
        }

        wrote |= up.execute(params![
            it.id,
            it.name,
            it.modified_ms,
            it.taken_ms,
            it.orientation,
            now_ms,
            it.mime_type,
            it.height,
            it.width,
            it.size,
            it.uri,
            it.checksum,
            it.thumbnail,
        ])? > 0;

        if wrote {
            written.push(it.id);
        }
    }
    Ok(written)
}

/// An item message, in one transaction.
///
/// Delete the removed ids, then for every other item drop the cached full bytes when it carries a
/// thumbnail and its cached checksum differs, and upsert it (a new item gets last-seen time `now`).
///
/// `completion` records the sync's completion with it. Returns
/// the ids of the items whose cache entry the message wrote (inserted, overwritten, or full bytes
/// dropped).
pub fn apply_items(
    db: &FeatureDb,
    items: &[MediaItem],
    removed: &[i64],
    now_ms: i64,
    completion: Option<Completion>,
) -> Result<Vec<i64>, StoreError> {
    db.transaction(|tx| {
        {
            let mut del = tx.prepare_cached("DELETE FROM media WHERE id = ?1")?;
            for id in removed {
                del.execute([id])?;
            }
        }

        let written = upsert(tx, items, now_ms)?;
        if let Some(c) = completion {
            complete(tx, c)?;
        }
        Ok::<_, StoreError>(written)
    })
}

/// The thumbnail-reply items: upserted as for an item message.
pub fn upsert_items(db: &FeatureDb, items: &[MediaItem], now_ms: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        upsert(tx, items, now_ms)?;
        Ok::<_, StoreError>(())
    })
}

/// Stores full-resolution bytes for the cached items among `images`; returns the ids stored.
pub fn set_full_bytes(db: &FeatureDb, images: &[(i64, Vec<u8>)]) -> Result<Vec<i64>, StoreError> {
    db.transaction(|tx| {
        let mut st = tx.prepare_cached("UPDATE media SET media = ?1 WHERE id = ?2")?;
        let mut stored = Vec::new();
        for (id, bytes) in images {
            if st.execute(params![bytes, id])? > 0 {
                stored.push(*id);
            }
        }
        Ok::<_, StoreError>(stored)
    })
}

/// Viewing an item in the full-screen viewer stamps its last-seen time.
pub fn set_last_seen(db: &FeatureDb, id: i64, now_ms: i64) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute("UPDATE media SET last_seen_time = ?1 WHERE id = ?2", [now_ms, id])?;
        Ok::<_, StoreError>(())
    })
}

/// Cache maintenance.
///
/// Stamp `last_seen = now` on items without one, then drop the full bytes of every item outside the
/// `keep_recent` most recent (by display order) whose last-seen time is older than `older_than_ms`.
///
/// Returns the ids dehydrated.
pub fn maintain(db: &FeatureDb, now_ms: i64, keep_recent: u32, older_than_ms: i64) -> Result<Vec<i64>, StoreError> {
    db.transaction(|tx| {
        tx.execute(
            "UPDATE media SET last_seen_time = ?1 WHERE last_seen_time IS NULL OR last_seen_time = 0",
            [now_ms],
        )?;

        let cutoff = now_ms.saturating_sub(older_than_ms);
        let ids: Vec<i64> = {
            let mut st = tx.prepare_cached(&format!(
                "SELECT id FROM (SELECT id, last_seen_time, media FROM media {DISPLAY_ORDER} LIMIT -1 OFFSET ?1) \
                 WHERE last_seen_time < ?2 AND media IS NOT NULL"
            ))?;
            st.query_map(params![i64::from(keep_recent), cutoff], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
        };

        {
            let mut st = tx.prepare_cached("UPDATE media SET media = NULL WHERE id = ?1")?;
            for id in &ids {
                st.execute([id])?;
            }
        }
        Ok::<_, StoreError>(ids)
    })
}

#[cfg(test)]
mod tests;
