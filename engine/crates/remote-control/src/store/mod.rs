//! Remote control's persisted setting: per phone, `remoteControlEnabled` (default on), which the
//! user changes in Settings. Nothing of it is sent to the phone.

use rusqlite::OptionalExtension;

use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{PhoneStores, StoreError, StoreLocation};

/// A phone's `remoteControlEnabled` until the user changes it.
pub const TOGGLE_DEFAULT: bool = true;

pub const FEATURE: &str = "remote_control";

pub const PHONE_MIGRATIONS: &[&str] = &["CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );"];

const REMOTE_CONTROL_ENABLED: &str = "remote_control_enabled";

/// A DCG client id compares case-insensitively: one store per phone whatever case a source uses.
fn store_phone(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The per-phone databases.
pub struct RemoteControlStore {
    phones: PhoneStores,
}

impl RemoteControlStore {
    pub fn open(location: &StoreLocation) -> Self {
        Self { phones: PhoneStores::new(location.clone(), FEATURE, PHONE_MIGRATIONS) }
    }

    /// `remoteControlEnabled` of `phone`.
    pub fn toggle(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        let stored: Option<bool> = db.read(|c| {
            c.query_row("SELECT value FROM phone_setting WHERE name = ?1", [REMOTE_CONTROL_ENABLED], |r| r.get(0))
                .optional()
        })?;
        Ok(stored.unwrap_or(TOGGLE_DEFAULT))
    }

    /// Writes the toggle; returns whether its effective value changed (one transaction).
    pub fn set_toggle(&self, phone: &DcgClient, enabled: bool) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        db.transaction(|tx| {
            let before: Option<bool> = tx
                .query_row("SELECT value FROM phone_setting WHERE name = ?1", [REMOTE_CONTROL_ENABLED], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO phone_setting (name, value) VALUES (?1, ?2)
                 ON CONFLICT (name) DO UPDATE SET value = excluded.value",
                rusqlite::params![REMOTE_CONTROL_ENABLED, enabled],
            )?;
            Ok::<_, StoreError>(before.unwrap_or(TOGGLE_DEFAULT) != enabled)
        })
    }
}

#[cfg(test)]
mod tests;
