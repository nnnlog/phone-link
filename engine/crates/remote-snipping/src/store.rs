//! The per-phone state of new-photo notifications, persisted (one row per phone store).

use std::time::SystemTime;

use rusqlite::OptionalExtension;

use phonelink_features::shared::db::{FeatureDb, StoreError};

/// The feature's name: its store and its notification ids (`remote_snipping/…`).
pub const FEATURE: &str = "remote_snipping";

pub const MIGRATIONS: &[&str] = &["CREATE TABLE phone_state (
        id INTEGER PRIMARY KEY CHECK (id = 0),
        capable INTEGER NOT NULL,
        enabled INTEGER NOT NULL,
        has_permissions INTEGER NOT NULL,
        last_popup_ms INTEGER,
        capability_answer INTEGER
    );"];

/// What the phone last answered to the capability request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityAnswer {
    /// Its reply listed new-media notifications.
    Capable,
    /// Its reply did not list them, or it has no handler for the request.
    NotCapable,
}

/// One phone's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhoneData {
    /// The last parsed capability reply listed new-media notifications (shows the settings toggle).
    pub capable: bool,
    /// The user's setting (default on).
    pub enabled: bool,
    /// The phone reported photo access (selects the settings link).
    pub has_permissions: bool,
    /// When a notification last popped up (not silent).
    pub last_popup: Option<SystemTime>,
    /// `None` until the phone answered a capability request.
    pub capability_answer: Option<CapabilityAnswer>,
}

impl Default for PhoneData {
    fn default() -> Self {
        Self { capable: false, enabled: true, has_permissions: false, last_popup: None, capability_answer: None }
    }
}

pub(crate) fn load(db: &FeatureDb) -> Result<PhoneData, StoreError> {
    type Row = (bool, bool, bool, Option<i64>, Option<i64>);
    let row: Option<Row> = db.read(|c| {
        c.query_row(
            "SELECT capable, enabled, has_permissions, last_popup_ms, capability_answer FROM phone_state WHERE id = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
    })?;

    let Some((capable, enabled, has_permissions, last_popup, answer)) = row else {
        return Ok(PhoneData::default());
    };

    let capability_answer = match answer {
        None => None,
        Some(1) => Some(CapabilityAnswer::Capable),
        Some(0) => Some(CapabilityAnswer::NotCapable),
        Some(other) => return Err(StoreError::Corrupt(format!("capability_answer {other}"))),
    };

    Ok(PhoneData {
        capable,
        enabled,
        has_permissions,
        last_popup: last_popup.map(phonelink_core::time::system_time),
        capability_answer,
    })
}

pub(crate) fn save(db: &FeatureDb, data: PhoneData) -> Result<(), StoreError> {
    let answer = data.capability_answer.map(|a| match a {
        CapabilityAnswer::Capable => 1i64,
        CapabilityAnswer::NotCapable => 0,
    });

    db.transaction(|tx| -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO phone_state (id, capable, enabled, has_permissions, last_popup_ms, capability_answer)
             VALUES (0, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET capable = excluded.capable, enabled = excluded.enabled,
                 has_permissions = excluded.has_permissions, last_popup_ms = excluded.last_popup_ms,
                 capability_answer = excluded.capability_answer",
            rusqlite::params![
                data.capable,
                data.enabled,
                data.has_permissions,
                data.last_popup.map(phonelink_core::time::unix_ms),
                answer
            ],
        )
        .map(|_| ())
        .map_err(StoreError::from)
    })
}
