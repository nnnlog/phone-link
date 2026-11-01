//! The per-phone state of the connected camera, persisted (one row per phone store).

use rusqlite::OptionalExtension;

use phonelink_features::shared::db::{FeatureDb, StoreError};

/// The feature's name: its store.
pub const FEATURE: &str = "connected_camera";

pub const MIGRATIONS: &[&str] = &["CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        camera_permission INTEGER NOT NULL,
        notification_permission INTEGER NOT NULL,
        camera_enabled INTEGER NOT NULL,
        initial_refresh_pending INTEGER NOT NULL,
        battery_level INTEGER NOT NULL,
        is_charging INTEGER NOT NULL
    );"];

/// One phone's persisted state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneData {
    /// The phone's last publication said it can stream its camera.
    pub capable: bool,
    /// The phone holds the camera permission.
    pub camera_permission: bool,
    /// The phone holds the notification permission.
    pub notification_permission: bool,
    /// The user's "use as a camera" switch.
    pub camera_enabled: bool,
    /// The switch has not been turned on by default yet.
    pub initial_refresh_pending: bool,
    /// The phone's battery percentage from its last battery message; `-1` before any.
    pub battery_level: i32,
    pub is_charging: bool,
}

impl Default for PhoneData {
    fn default() -> Self {
        Self {
            capable: false,
            camera_permission: false,
            notification_permission: false,
            camera_enabled: false,
            initial_refresh_pending: true,
            battery_level: -1,
            is_charging: false,
        }
    }
}

type Row = (bool, bool, bool, bool, bool, i32, bool);

pub(crate) fn load(db: &FeatureDb) -> Result<PhoneData, StoreError> {
    let row: Option<Row> = db.read(|c| {
        c.query_row(
            "SELECT capable, camera_permission, notification_permission, camera_enabled,
                    initial_refresh_pending, battery_level, is_charging
             FROM phone_state WHERE id = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .optional()
    })?;
    Ok(match row {
        None => PhoneData::default(),
        Some((
            capable,
            camera_permission,
            notification_permission,
            camera_enabled,
            initial_refresh_pending,
            battery_level,
            is_charging,
        )) => PhoneData {
            capable,
            camera_permission,
            notification_permission,
            camera_enabled,
            initial_refresh_pending,
            battery_level,
            is_charging,
        },
    })
}

pub(crate) fn save(db: &FeatureDb, data: &PhoneData) -> Result<(), StoreError> {
    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO phone_state (id, capable, camera_permission, notification_permission, camera_enabled,
                 initial_refresh_pending, battery_level, is_charging)
             VALUES (0, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (id) DO UPDATE SET capable = excluded.capable,
                 camera_permission = excluded.camera_permission,
                 notification_permission = excluded.notification_permission,
                 camera_enabled = excluded.camera_enabled,
                 initial_refresh_pending = excluded.initial_refresh_pending,
                 battery_level = excluded.battery_level,
                 is_charging = excluded.is_charging",
            rusqlite::params![
                data.capable,
                data.camera_permission,
                data.notification_permission,
                data.camera_enabled,
                data.initial_refresh_pending,
                data.battery_level,
                data.is_charging
            ],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}
