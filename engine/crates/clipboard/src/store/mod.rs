//! The clipboard's persisted settings — its inputs.
//!
//! It covers the global switch (an app setting, default on; it also decides whether the PC
//! advertises `CLIPBOARD`) and, per phone, `clipboardEnabled` (default off) and
//! `clipboardFeatureStatusUpToDate` (default true).
//!
//! The last announced hash lives
//! in memory only and is not here.

use rusqlite::OptionalExtension;

use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError, StoreLocation};

/// The global switch until the user changes it.
pub const GLOBAL_SWITCH_DEFAULT: bool = true;
/// A phone's `clipboardEnabled` until the user changes it.
pub const TOGGLE_DEFAULT: bool = false;
/// A phone's `clipboardFeatureStatusUpToDate` until the feature-state sync writes it.
pub const UP_TO_DATE_DEFAULT: bool = true;

/// The PC capability string the global switch advertises (enrollment / `SetDeviceInfo`
/// `metadata.capabilities`), and the phone capability every gate tests (exact, case-sensitive).
pub const CAPABILITY: &str = "CLIPBOARD";

pub const FEATURE: &str = "clipboard";

pub const GLOBAL_MIGRATIONS: &[&str] = &["CREATE TABLE setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );"];

pub const PHONE_MIGRATIONS: &[&str] = &["CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );"];

const GLOBAL_SWITCH: &str = "global_switch";
const CLIPBOARD_ENABLED: &str = "clipboard_enabled";
const FEATURE_STATUS_UP_TO_DATE: &str = "feature_status_up_to_date";

/// The persisted global switch, read without installing the feature.
///
/// The engine decides the advertised capabilities with it before provisioning; a
/// [`StoreLocation::Memory`] store has nothing persisted, so it reads the default.
pub fn global_switch_persisted(location: &StoreLocation) -> Result<bool, StoreError> {
    let db = FeatureDb::open_global(location, FEATURE, GLOBAL_MIGRATIONS)?;
    read_flag(&db, "setting", GLOBAL_SWITCH, GLOBAL_SWITCH_DEFAULT)
}

/// `base` with `CLIPBOARD` present iff `enabled`: any `CLIPBOARD` already in `base` is removed, and
/// when enabled it is put first (its place in Cross Device's capability list).
///
/// Hand the result to `AuthService::set_capabilities`.
pub fn with_clipboard_capability(base: &[String], enabled: bool) -> Vec<String> {
    let mut capabilities: Vec<String> = Vec::with_capacity(base.len() + 1);
    if enabled {
        capabilities.push(CAPABILITY.to_owned());
    }
    capabilities.extend(base.iter().filter(|c| c.as_str() != CAPABILITY).cloned());
    capabilities
}

fn read_flag(db: &FeatureDb, table: &str, name: &str, default: bool) -> Result<bool, StoreError> {
    let sql = format!("SELECT value FROM {table} WHERE name = ?1");
    let stored: Option<bool> = db.read(|c| c.query_row(&sql, [name], |r| r.get(0)).optional())?;
    Ok(stored.unwrap_or(default))
}

/// Writes `value`; returns whether the effective value changed (one transaction).
fn write_flag(db: &FeatureDb, table: &str, name: &str, default: bool, value: bool) -> Result<bool, StoreError> {
    let select = format!("SELECT value FROM {table} WHERE name = ?1");
    let upsert = format!(
        "INSERT INTO {table} (name, value) VALUES (?1, ?2) ON CONFLICT (name) DO UPDATE SET value = excluded.value"
    );

    db.transaction(|tx| {
        let before: Option<bool> = tx.query_row(&select, [name], |r| r.get(0)).optional()?;
        tx.execute(&upsert, rusqlite::params![name, value])?;
        Ok::<_, StoreError>(before.unwrap_or(default) != value)
    })
}

/// A DCG client id compares case-insensitively: one store per phone whatever case a source uses.
fn store_phone(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The global settings database and the per-phone databases.
pub struct ClipboardStore {
    global: FeatureDb,
    phones: PhoneStores,
}

impl ClipboardStore {
    pub fn open(location: &StoreLocation) -> Result<Self, StoreError> {
        Ok(Self {
            global: FeatureDb::open_global(location, FEATURE, GLOBAL_MIGRATIONS)?,
            phones: PhoneStores::new(location.clone(), FEATURE, PHONE_MIGRATIONS),
        })
    }

    pub fn global_switch(&self) -> Result<bool, StoreError> {
        read_flag(&self.global, "setting", GLOBAL_SWITCH, GLOBAL_SWITCH_DEFAULT)
    }

    /// Returns whether the switch changed.
    pub fn set_global_switch(&self, enabled: bool) -> Result<bool, StoreError> {
        write_flag(&self.global, "setting", GLOBAL_SWITCH, GLOBAL_SWITCH_DEFAULT, enabled)
    }

    /// `clipboardEnabled` of `phone`.
    pub fn toggle(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        read_flag(&db, "phone_setting", CLIPBOARD_ENABLED, TOGGLE_DEFAULT)
    }

    /// Returns whether the toggle changed.
    pub fn set_toggle(&self, phone: &DcgClient, enabled: bool) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        write_flag(&db, "phone_setting", CLIPBOARD_ENABLED, TOGGLE_DEFAULT, enabled)
    }

    /// `clipboardFeatureStatusUpToDate` of `phone`.
    pub fn up_to_date(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        read_flag(&db, "phone_setting", FEATURE_STATUS_UP_TO_DATE, UP_TO_DATE_DEFAULT)
    }

    /// Returns whether the flag changed.
    pub fn set_up_to_date(&self, phone: &DcgClient, up_to_date: bool) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        write_flag(&db, "phone_setting", FEATURE_STATUS_UP_TO_DATE, UP_TO_DATE_DEFAULT, up_to_date)
    }
}

#[cfg(test)]
mod tests;
