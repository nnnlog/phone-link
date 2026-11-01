//! The persisted audio-controls setting: per phone (default on), which the user changes in Settings.
//! Nothing of it is sent to the phone.

use rusqlite::OptionalExtension;

use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{PhoneStores, StoreError, StoreLocation};

/// A phone's audio-controls setting until the user changes it.
pub const SETTING_DEFAULT: bool = true;

pub const PHONE_MIGRATIONS: &[&str] = &["CREATE TABLE phone_setting (
        name TEXT NOT NULL PRIMARY KEY,
        value INTEGER NOT NULL
    );"];

const AUDIO_CONTROLS: &str = "audio_controls";

/// A DCG client id compares case-insensitively: one store per phone whatever case a source uses.
fn store_phone(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The per-phone databases.
pub struct AudioStore {
    phones: PhoneStores,
}

impl AudioStore {
    pub fn open(location: &StoreLocation) -> Self {
        Self { phones: PhoneStores::new(location.clone(), super::FEATURE, PHONE_MIGRATIONS) }
    }

    /// The audio-controls setting of `phone`.
    pub fn setting(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        let stored: Option<bool> = db.read(|c| {
            c.query_row("SELECT value FROM phone_setting WHERE name = ?1", [AUDIO_CONTROLS], |r| r.get(0)).optional()
        })?;
        Ok(stored.unwrap_or(SETTING_DEFAULT))
    }

    /// Writes the setting; returns whether its effective value changed (one transaction).
    pub fn set_setting(&self, phone: &DcgClient, on: bool) -> Result<bool, StoreError> {
        let db = self.phones.get(&store_phone(phone))?;
        db.transaction(|tx| {
            let before: Option<bool> = tx
                .query_row("SELECT value FROM phone_setting WHERE name = ?1", [AUDIO_CONTROLS], |r| r.get(0))
                .optional()?;
            tx.execute(
                "INSERT INTO phone_setting (name, value) VALUES (?1, ?2)
                 ON CONFLICT (name) DO UPDATE SET value = excluded.value",
                rusqlite::params![AUDIO_CONTROLS, on],
            )?;
            Ok::<_, StoreError>(before.unwrap_or(SETTING_DEFAULT) != on)
        })
    }
}

#[cfg(test)]
mod tests;
