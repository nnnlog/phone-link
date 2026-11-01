//! The features' persistent stores.
//!
//! It covers one SQLite database per phone and feature, at
//! `<root>/phones/<dcgClientId>.<Environment>/<feature>.sqlite3`, plus one per feature at
//! `<root>/<feature>.sqlite3` for settings that belong to no phone; private in-memory databases in
//! tests.
//!
//! They hold the state each feature's protocol depends on; each feature owns its schema as a
//! list of migrations applied in order and counted by SQLite's `user_version`.
//!
//! The stores hold phone content (notifications, messages, contacts, photos), so the directories the
//! engine creates are `0700` and database files `0600`; an existing directory the caller hands over
//! keeps its mode.

use std::collections::HashMap;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, Transaction};
use tracing::warn;

use phonelink_core::types::DcgClient;

/// Where the feature stores live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreLocation {
    /// Database files under this directory.
    Directory(PathBuf),
    /// A fresh private in-memory database for every open: nothing survives the opener (tests).
    Memory,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database file or its directory could not be created, secured, or removed; `what` says
    /// which, without its path, which names the phone.
    #[error("store {what}: {message}")]
    Path { what: &'static str, message: String },
    /// A phone id that cannot name a directory, or a feature name that cannot name a file.
    #[error("invalid store name: {0}")]
    InvalidName(String),
    /// The database was written by a newer schema than this build knows.
    #[error("{feature} store has schema version {found}, newer than this build's {known}")]
    NewerSchema { feature: &'static str, found: u32, known: u32 },
    /// Stored data that does not decode as its writer wrote it.
    #[error("corrupt store: {0}")]
    Corrupt(String),
}

/// Takes a store lock. A poisoned lock means a panic while it was held; a transaction open then was
/// rolled back when it was dropped during the unwind, so the connection is consistent and the store
/// stays usable (the panic itself is isolated where it happened, e.g. by the App layer's inbound
/// worker). The poisoning is cleared, so it is logged once.
#[expect(clippy::disallowed_methods, reason = "logs the poisoning before recovering")]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        warn!("feature store lock poisoned by an earlier panic; its transaction was rolled back");
        // Every lock recovers the same way; clearing the flag logs the panic once, not on each later lock.
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

/// One open feature database.
pub struct FeatureDb {
    feature: &'static str,
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for FeatureDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeatureDb").field("feature", &self.feature).finish_non_exhaustive()
    }
}

/// Deletes every phone's databases (`<root>/phones/`) — the sign-out.
///
/// The settings that belong to no phone are kept (Phone Link on Windows keeps them). Nothing to do in
/// memory or when no phone store was ever created.
pub fn delete_phone_stores(location: &StoreLocation) -> Result<(), StoreError> {
    let StoreLocation::Directory(root) = location else {
        return Ok(());
    };

    let dir = root.join("phones");
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Path { what: "phones directory not removed", message: e.to_string() }),
    }
}

/// A DCG client id names the phone's directory: letters, digits and `-` only, so it cannot reach
/// outside `phones/`.
fn phone_dir_name(phone: &DcgClient) -> Result<String, StoreError> {
    let ok = !phone.id.is_empty() && phone.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !ok {
        return Err(StoreError::InvalidName("a phone id that cannot name a directory".into()));
    }
    Ok(format!("{}.{}", phone.id, phone.env))
}

fn check_feature_name(feature: &str) -> Result<(), StoreError> {
    let ok = !feature.is_empty() && feature.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok { Ok(()) } else { Err(StoreError::InvalidName(format!("feature {feature:?}"))) }
}

fn path_err(what: &'static str, e: impl std::fmt::Display) -> StoreError {
    StoreError::Path { what, message: e.to_string() }
}

/// Creates `dir` (and its missing parents) `0700`; an existing directory is left as it is.
fn private_dir(dir: &Path) -> Result<(), StoreError> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| path_err("directory not created", e))
}

impl FeatureDb {
    /// Opens (creating it) `feature`'s database for `phone` and brings its schema up to date.
    pub fn open_phone(
        location: &StoreLocation,
        phone: &DcgClient,
        feature: &'static str,
        migrations: &[&str],
    ) -> Result<Self, StoreError> {
        check_feature_name(feature)?;
        let phone_dir = phone_dir_name(phone)?;

        let file = match location {
            StoreLocation::Memory => None,
            StoreLocation::Directory(root) => {
                let dir = root.join("phones").join(phone_dir);
                private_dir(&root.join("phones"))?;
                private_dir(&dir)?;
                Some(dir.join(format!("{feature}.sqlite3")))
            }
        };
        Self::open(file.as_deref(), feature, migrations)
    }

    /// Opens (creating it) `feature`'s database for settings that belong to no phone.
    pub fn open_global(
        location: &StoreLocation,
        feature: &'static str,
        migrations: &[&str],
    ) -> Result<Self, StoreError> {
        check_feature_name(feature)?;
        let file = match location {
            StoreLocation::Memory => None,
            StoreLocation::Directory(root) => {
                private_dir(root)?;
                Some(root.join(format!("{feature}.sqlite3")))
            }
        };
        Self::open(file.as_deref(), feature, migrations)
    }

    fn open(file: Option<&Path>, feature: &'static str, migrations: &[&str]) -> Result<Self, StoreError> {
        let mut conn = match file {
            None => Connection::open_in_memory()?,
            Some(path) => {
                let conn = Connection::open(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| path_err("database file not made private", e))?;
                conn.pragma_update(None, "journal_mode", "WAL")?;
                conn
            }
        };

        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut conn, feature, migrations)?;
        Ok(Self { feature, conn: Mutex::new(conn) })
    }

    #[cfg(feature = "test-support")]
    /// The feature that owns this database.
    pub fn feature(&self) -> &'static str {
        self.feature
    }

    /// Runs `f` on the connection (reads, or a single statement that needs no transaction).
    pub fn read<R>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<R>) -> Result<R, StoreError> {
        let conn = lock(&self.conn);
        Ok(f(&conn)?)
    }

    /// Runs `f` inside one transaction: committed when `f` returns `Ok`, rolled back otherwise —
    /// so a feature's change to its store is applied whole or not at all.
    pub fn transaction<R, E: From<rusqlite::Error>>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<R, E>,
    ) -> Result<R, E> {
        let mut conn = lock(&self.conn);
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

/// Applies `migrations[user_version..]`, each in its own transaction that also advances
/// `user_version`. A database newer than `migrations` fails loudly.
fn migrate(conn: &mut Connection, feature: &'static str, migrations: &[&str]) -> Result<(), StoreError> {
    let known = u32::try_from(migrations.len()).map_err(|_| StoreError::InvalidName(format!("feature {feature:?}")))?;
    let found: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found > known {
        return Err(StoreError::NewerSchema { feature, found, known });
    }

    for (n, sql) in migrations.iter().enumerate().skip(found as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", n as u32 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

/// The per-phone databases of one feature, opened on first use and kept open.
pub struct PhoneStores {
    location: StoreLocation,
    feature: &'static str,
    migrations: &'static [&'static str],
    open: Mutex<HashMap<DcgClient, Arc<FeatureDb>>>,
}

impl PhoneStores {
    pub fn new(location: StoreLocation, feature: &'static str, migrations: &'static [&'static str]) -> Self {
        Self { location, feature, migrations, open: Mutex::new(HashMap::new()) }
    }

    /// `phone`'s database, opened (and migrated) on first use. A [`StoreLocation::Memory`] database
    /// lives as long as this `PhoneStores`.
    pub fn get(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, StoreError> {
        let mut open = lock(&self.open);
        if let Some(db) = open.get(phone) {
            return Ok(db.clone());
        }
        let db = Arc::new(FeatureDb::open_phone(&self.location, phone, self.feature, self.migrations)?);
        open.insert(phone.clone(), db.clone());
        Ok(db)
    }
}

#[cfg(test)]
mod tests;
