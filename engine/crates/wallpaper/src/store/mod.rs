//! The per-phone wallpaper store.
//!
//! It covers Phone Link's copy (one row per `wallpaperType`, the `wallpaper` sequence numbers, the
//! phone's wallpaper permission) and the Cross Device Experience Host's copy (one row per type, `0`
//! home screen, `1` lock screen).

use rusqlite::{OptionalExtension, params};

use phonelink_features::{FeatureDb, StoreError};

pub const MIGRATIONS: &[&str] = &["
    CREATE TABLE phone_link_wallpaper (
        wallpaper_type INTEGER PRIMARY KEY NOT NULL,
        wallpaper_id INTEGER NOT NULL,
        blob BLOB NOT NULL);
    CREATE TABLE cdeh_wallpaper (
        wallpaper_type INTEGER PRIMARY KEY NOT NULL,
        wallpaper_id INTEGER NOT NULL,
        blob BLOB NOT NULL);
    CREATE TABLE state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        current_sequence INTEGER NOT NULL DEFAULT -1,
        expected_sequence INTEGER NOT NULL DEFAULT -1,
        device_permission INTEGER NOT NULL DEFAULT 0);
    INSERT INTO state (id) VALUES (0);
"];

/// The home-screen type, in both copies.
pub const HOME_SCREEN: i64 = 0;
/// The lock-screen type, in both copies.
pub const LOCK_SCREEN: i64 = 1;

/// Which copy a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperCopy {
    PhoneLink,
    CrossDevice,
}

impl WallpaperCopy {
    fn table(self) -> &'static str {
        match self {
            WallpaperCopy::PhoneLink => "phone_link_wallpaper",
            WallpaperCopy::CrossDevice => "cdeh_wallpaper",
        }
    }
}

/// One stored wallpaper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredWallpaper {
    pub id: i64,
    pub bytes: Vec<u8>,
}

/// Phone Link's state of a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhoneLinkState {
    /// The *current* `wallpaper` sequence number, `-1` when none is stored.
    pub current_sequence: i64,
    /// The *expected* one, `-1` until a connect reply set it.
    pub expected_sequence: i64,
    /// `wallpaperDevicePermission`, `false` until the phone reported it.
    pub device_permission: bool,
}

pub(crate) fn wallpaper(
    db: &FeatureDb,
    copy: WallpaperCopy,
    wallpaper_type: i64,
) -> Result<Option<StoredWallpaper>, StoreError> {
    let sql = format!("SELECT wallpaper_id, blob FROM {} WHERE wallpaper_type = ?1", copy.table());
    db.read(|c| {
        c.query_row(&sql, params![wallpaper_type], |r| Ok(StoredWallpaper { id: r.get(0)?, bytes: r.get(1)? }))
            .optional()
    })
}

pub(crate) fn phone_link_state(db: &FeatureDb) -> Result<PhoneLinkState, StoreError> {
    db.read(|c| {
        c.query_row("SELECT current_sequence, expected_sequence, device_permission FROM state WHERE id = 0", [], |r| {
            Ok(PhoneLinkState {
                current_sequence: r.get(0)?,
                expected_sequence: r.get(1)?,
                device_permission: r.get::<_, i64>(2)? != 0,
            })
        })
    })
}

/// A Phone Link item: its type's row and the current sequence number, in one transaction.
pub(crate) fn put_phone_link_item(
    db: &FeatureDb,
    wallpaper_type: i64,
    id: i64,
    bytes: &[u8],
    sequence: i64,
) -> Result<(), StoreError> {
    db.transaction(|tx| {
        tx.execute(
            "INSERT INTO phone_link_wallpaper (wallpaper_type, wallpaper_id, blob) VALUES (?1, ?2, ?3)
             ON CONFLICT (wallpaper_type) DO UPDATE SET wallpaper_id = excluded.wallpaper_id, blob = excluded.blob",
            params![wallpaper_type, id, bytes],
        )?;
        tx.execute("UPDATE state SET current_sequence = ?1 WHERE id = 0", params![sequence])?;
        Ok::<_, StoreError>(())
    })
}

pub(crate) fn set_expected_sequence(db: &FeatureDb, expected: i64) -> Result<(), StoreError> {
    db.read(|c| c.execute("UPDATE state SET expected_sequence = ?1 WHERE id = 0", params![expected]).map(|_| ()))
}

/// Sets the phone's wallpaper permission; whether it changed.
pub(crate) fn set_device_permission(db: &FeatureDb, granted: bool) -> Result<bool, StoreError> {
    db.transaction(|tx| {
        let before: i64 = tx.query_row("SELECT device_permission FROM state WHERE id = 0", [], |r| r.get(0))?;
        tx.execute("UPDATE state SET device_permission = ?1 WHERE id = 0", params![i64::from(granted)])?;
        Ok::<_, StoreError>((before != 0) != granted)
    })
}

/// The ids of the Cross Device copy's home-screen and lock-screen rows.
pub(crate) fn cross_device_ids(db: &FeatureDb) -> Result<(Option<i64>, Option<i64>), StoreError> {
    let id = |t| {
        db.read(|c| {
            c.query_row("SELECT wallpaper_id FROM cdeh_wallpaper WHERE wallpaper_type = ?1", params![t], |r| r.get(0))
                .optional()
        })
    };
    Ok((id(HOME_SCREEN)?, id(LOCK_SCREEN)?))
}

/// What a pull reply does to the Cross Device copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CrossDeviceChange {
    /// Every row is deleted.
    DeleteAll,
    /// These `(type, id, bytes)` rows are replaced.
    Upsert(Vec<(i64, i64, Vec<u8>)>),
}

pub(crate) fn apply_cross_device(db: &FeatureDb, change: &CrossDeviceChange) -> Result<(), StoreError> {
    db.transaction(|tx| {
        match change {
            CrossDeviceChange::DeleteAll => {
                tx.execute("DELETE FROM cdeh_wallpaper", [])?;
            }
            CrossDeviceChange::Upsert(rows) => {
                for (wallpaper_type, id, bytes) in rows {
                    tx.execute(
                        "INSERT INTO cdeh_wallpaper (wallpaper_type, wallpaper_id, blob) VALUES (?1, ?2, ?3)
                         ON CONFLICT (wallpaper_type) DO UPDATE SET
                             wallpaper_id = excluded.wallpaper_id, blob = excluded.blob",
                        params![wallpaper_type, id, bytes],
                    )?;
                }
            }
        }
        Ok::<_, StoreError>(())
    })
}

#[cfg(test)]
mod tests;
