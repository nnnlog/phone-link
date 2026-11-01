//! The per-phone "enabled" switch of the Cross Device Experience Host features.
//!
//! It covers the local per-device switch the CDEH request wrapper refuses a disabled phone by
//! (`UserDisabled`, in the `/DeviceResourceManager` sequence) and that gates the clipboard ("Phone
//! enabled").
//!
//! Persisted;
//! default **on**; the user turns it off per phone.

use std::sync::Arc;

use rusqlite::OptionalExtension;
use tokio::sync::broadcast;

use super::db::{FeatureDb, StoreError, StoreLocation};
use phonelink_core::types::DcgClient;

pub const MIGRATIONS: &[&str] = &["CREATE TABLE phone_switch (
        phone TEXT NOT NULL PRIMARY KEY,
        enabled INTEGER NOT NULL
    );"];

/// A phone's switch changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneSwitchChanged {
    pub phone: DcgClient,
    pub enabled: bool,
}

/// The switches of every phone of the account.
pub struct PhoneSwitches {
    db: FeatureDb,
    events: broadcast::Sender<PhoneSwitchChanged>,
}

/// The key a phone's switch is stored under (`<dcgClientId>.<Environment>`, id lower-cased: DCG ids
/// compare case-insensitively).
fn key(phone: &DcgClient) -> String {
    format!("{}.{}", phone.id.to_ascii_lowercase(), phone.env)
}

impl PhoneSwitches {
    pub fn open(location: &StoreLocation) -> Result<Arc<Self>, StoreError> {
        let db = FeatureDb::open_global(location, "phone_switch", MIGRATIONS)?;
        Ok(Arc::new(Self { db, events: broadcast::channel(16).0 }))
    }

    /// `phone`'s switch; `true` until the user turns it off.
    pub fn is_enabled(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        let stored: Option<bool> = self.db.read(|c| {
            c.query_row("SELECT enabled FROM phone_switch WHERE phone = ?1", [key(phone)], |r| r.get(0)).optional()
        })?;
        Ok(stored.unwrap_or(true))
    }

    /// Sets `phone`'s switch; a change is reported to the subscribers.
    pub fn set_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), StoreError> {
        let before = self.is_enabled(phone)?;

        self.db.transaction(|tx| {
            tx.execute(
                "INSERT INTO phone_switch (phone, enabled) VALUES (?1, ?2)
                 ON CONFLICT (phone) DO UPDATE SET enabled = excluded.enabled",
                rusqlite::params![key(phone), enabled],
            )
            .map(|_| ())
        })?;

        if before != enabled {
            // No subscriber is not an error.
            let _ = self.events.send(PhoneSwitchChanged { phone: phone.clone(), enabled });
        }
        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PhoneSwitchChanged> {
        self.events.subscribe()
    }

    /// Forgets every phone's switch (the sign-out: they are per-phone state of the account); every
    /// phone is on again.
    pub fn forget_all(location: &StoreLocation) -> Result<(), StoreError> {
        let db = FeatureDb::open_global(location, "phone_switch", MIGRATIONS)?;
        db.transaction(|tx| tx.execute("DELETE FROM phone_switch", []).map(|_| ()).map_err(StoreError::from))
    }
}

#[cfg(test)]
mod tests;
