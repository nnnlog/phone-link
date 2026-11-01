//! The per-phone settings of file transfer, persisted (one row per phone store).

use rusqlite::OptionalExtension;

use phonelink_features::shared::db::{FeatureDb, StoreError};

/// The feature's name: its store and its notification ids (`file_transfer/…`).
pub const FEATURE: &str = "file_transfer";

pub const MIGRATIONS: &[&str] = &["CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        share_enabled INTEGER NOT NULL,
        received_files_folder TEXT,
        show_folder_setting INTEGER NOT NULL
    );"];

/// One phone's settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneData {
    /// The per-phone *Share* toggle (default on); gates sending only.
    pub share_enabled: bool,
    /// Where received files go; `None` = the default folder.
    pub received_files_folder: Option<String>,
    /// The received-files-folder setting is shown (set once a receive request was accepted).
    pub show_folder_setting: bool,
}

impl Default for PhoneData {
    fn default() -> Self {
        Self { share_enabled: true, received_files_folder: None, show_folder_setting: false }
    }
}

pub(crate) fn load(db: &FeatureDb) -> Result<PhoneData, StoreError> {
    let row: Option<(bool, Option<String>, bool)> = db.read(|c| {
        c.query_row(
            "SELECT share_enabled, received_files_folder, show_folder_setting FROM phone_state WHERE id = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
    })?;

    Ok(match row {
        None => PhoneData::default(),
        Some((share_enabled, received_files_folder, show_folder_setting)) => {
            PhoneData { share_enabled, received_files_folder, show_folder_setting }
        }
    })
}

pub(crate) fn save(db: &FeatureDb, data: &PhoneData) -> Result<(), StoreError> {
    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO phone_state (id, share_enabled, received_files_folder, show_folder_setting)
             VALUES (0, ?1, ?2, ?3)
             ON CONFLICT (id) DO UPDATE SET share_enabled = excluded.share_enabled,
                 received_files_folder = excluded.received_files_folder,
                 show_folder_setting = excluded.show_folder_setting",
            rusqlite::params![data.share_enabled, data.received_files_folder, data.show_folder_setting],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}
