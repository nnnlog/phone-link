//! The messaging feature's file types: a MIME type's family and preferred extension, and a file
//! name's type, from the shared MIME-info database of the XDG data folders, with macOS's answers where
//! they differ.

mod mac_answers;
mod mime_database;

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub use mime_database::MimeDatabase;
use phonelink_messaging::MessagingFileTypes;
use phonelink_messaging::edge::FileFamily;
use tracing::warn;

/// The `mime` directories of the XDG base directories, most important first.
fn mime_dirs() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let home = env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = env("XDG_DATA_DIRS").map_or_else(
        || vec![PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")],
        |dirs| std::env::split_paths(&dirs).collect(),
    );
    home.into_iter().chain(system).map(|dir| dir.join("mime")).collect()
}

/// The messaging MIME-type lookups from the shared MIME-info database.
#[derive(Debug)]
pub struct SharedMimeFileTypes {
    database: MimeDatabase,
}

impl SharedMimeFileTypes {
    pub fn database(&self) -> &MimeDatabase {
        &self.database
    }
}

impl MessagingFileTypes for SharedMimeFileTypes {
    fn family(&self, content_type: String) -> FileFamily {
        self.database.family_of(&content_type)
    }

    fn file_extension(&self, content_type: String) -> Option<String> {
        self.database.extension_of(&content_type)
    }
}

/// The app's file types, read from the system's database the first time they are asked for.
pub fn file_types() -> &'static Arc<SharedMimeFileTypes> {
    static FILE_TYPES: OnceLock<Arc<SharedMimeFileTypes>> = OnceLock::new();
    FILE_TYPES.get_or_init(|| {
        let dirs = mime_dirs();
        let database = MimeDatabase::load(dirs.clone());
        if !database.is_loaded() {
            let searched: Vec<String> = dirs.iter().map(|dir| dir.display().to_string()).collect();
            warn!(
                folders = %searched.join(", "),
                "no shared MIME-info database in the folders: message attachments are typed by their media type alone"
            );
        }
        Arc::new(SharedMimeFileTypes { database })
    })
}
