//! The tree as the file manager reads it. Every entry has an identifier: the decimal `MediaStore` id of a
//! phone item, `dir:<location>` for a synthetic folder, [`ROOT`] for the root. An entry's parent is the
//! entry at the folder part of its location. A file's content version follows its size and
//! modification time (its downloaded content stays while they are unchanged); its metadata version
//! follows the phone's checksum and its location.

use super::path::{self, Location};
use super::store::{self, Entry, Row, Store};
use super::{Inner, PhoneFilesError};
use phonelink_core::types::DcgClient;
use phonelink_proto::phone_files::v1::FilesConfigurationPayload;

/// The identifier of the tree's root.
pub const ROOT: &str = "";

/// One entry of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub identifier: String,
    /// The parent folder's identifier ([`ROOT`] for an entry at the top, and for the root itself).
    pub parent: String,
    /// The name shown; empty for the root.
    pub name: String,
    pub is_directory: bool,
    pub size: i64,
    /// Unix seconds.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    /// `phone_files.v1.FileType` (`0` for a synthetic folder or the root).
    pub file_type: i32,
    pub content_version: String,
    pub metadata_version: String,
    /// The phone makes a thumbnail of it.
    pub has_thumbnail: bool,
    /// A folder the tree adds between the root and a phone item; the phone has no entry for it.
    pub synthetic: bool,
}

/// What changed after a generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemChanges {
    pub updated: Vec<Item>,
    pub removed: Vec<String>,
    /// The generation these changes bring the reader to.
    pub generation: i64,
}

/// A folder of the tree, as an operation inside it needs it.
pub(super) struct Folder {
    pub location: Location,
    /// The phone's entry for it (none for the root and synthetic folders).
    pub row: Option<Row>,
}

fn root_item() -> Item {
    Item {
        identifier: ROOT.to_owned(),
        parent: ROOT.to_owned(),
        name: String::new(),
        is_directory: true,
        size: 0,
        created: None,
        modified: None,
        file_type: 0,
        content_version: String::new(),
        metadata_version: String::new(),
        has_thumbnail: false,
        synthetic: true,
    }
}

impl Inner {
    /// The identifier of the entry at `location`'s folder.
    fn parent_identifier(store: &Store, location: &str) -> Result<String, PhoneFilesError> {
        let folder = path::parent(location);
        if folder.is_empty() {
            return Ok(ROOT.to_owned());
        }
        if let Some(row) = store.row_at(folder)? {
            return Ok(store::row_identifier(row.id));
        }
        Ok(store::folder_identifier(&store.folder_at(folder)?.unwrap_or_else(|| folder.to_owned())))
    }

    pub(super) fn item_of_row(
        &self,
        store: &Store,
        row: &Row,
        config: &FilesConfigurationPayload,
    ) -> Result<Item, PhoneFilesError> {
        Ok(Item {
            identifier: store::row_identifier(row.id),
            parent: Self::parent_identifier(store, &row.location)?,
            name: row.name.clone(),
            is_directory: row.is_directory,
            size: row.size,
            created: row.created,
            modified: row.modified,
            file_type: row.file_type,
            content_version: if row.is_directory {
                String::new()
            } else {
                format!("{}:{}", row.size, row.modified.unwrap_or(-1))
            },
            metadata_version: format!("{}:{}", row.checksum, row.location),
            has_thumbnail: self.has_thumbnail(row, config),
            synthetic: false,
        })
    }

    fn item_of_folder(store: &Store, location: &str) -> Result<Item, PhoneFilesError> {
        Ok(Item {
            identifier: store::folder_identifier(location),
            parent: Self::parent_identifier(store, location)?,
            name: path::leaf(location).to_owned(),
            is_directory: true,
            size: 0,
            created: None,
            modified: None,
            file_type: 0,
            content_version: String::new(),
            metadata_version: location.to_owned(),
            has_thumbnail: false,
            synthetic: true,
        })
    }

    pub(super) fn item(&self, phone: &DcgClient, identifier: &str) -> Result<Option<Item>, PhoneFilesError> {
        if identifier == ROOT {
            return Ok(Some(root_item()));
        }

        let store = self.store(phone)?;
        let config = self.phone_config(phone);
        match store.entry(identifier)? {
            None => Ok(None),
            Some(Entry::Row(row)) => Ok(Some(self.item_of_row(&store, &row, &config)?)),
            Some(Entry::Folder(location)) => Ok(Some(Self::item_of_folder(&store, &location)?)),
        }
    }

    /// The folder `identifier` names; `None` when it names no folder of the tree.
    pub(super) fn folder(&self, store: &Store, identifier: &str) -> Result<Option<Folder>, PhoneFilesError> {
        if identifier == ROOT {
            return Ok(Some(Folder { location: String::new(), row: None }));
        }
        Ok(match store.entry(identifier)? {
            Some(Entry::Row(row)) if row.is_directory => {
                Some(Folder { location: row.location.clone(), row: Some(row) })
            }
            Some(Entry::Folder(location)) => Some(Folder { location, row: None }),
            _ => None,
        })
    }

    pub(super) fn children(&self, phone: &DcgClient, identifier: &str) -> Result<Option<Vec<Item>>, PhoneFilesError> {
        let store = self.store(phone)?;
        let Some(folder) = self.folder(&store, identifier)? else {
            return Ok(None);
        };

        let config = self.phone_config(phone);
        let (rows, folders) = store.children(&folder.location)?;
        let mut items = Vec::with_capacity(rows.len() + folders.len());
        for location in &folders {
            items.push(Self::item_of_folder(&store, location)?);
        }
        for row in &rows {
            items.push(self.item_of_row(&store, row, &config)?);
        }

        Ok(Some(items))
    }

    pub(super) fn changes(&self, phone: &DcgClient, generation: i64) -> Result<Option<ItemChanges>, PhoneFilesError> {
        let store = self.store(phone)?;
        let Some(changes) = store.changes_since(generation)? else {
            return Ok(None);
        };

        let config = self.phone_config(phone);
        let mut updated = Vec::with_capacity(changes.rows.len() + changes.folders.len());
        for location in &changes.folders {
            updated.push(Self::item_of_folder(&store, location)?);
        }
        for row in &changes.rows {
            updated.push(self.item_of_row(&store, row, &config)?);
        }

        Ok(Some(ItemChanges { updated, removed: changes.removed, generation: changes.generation }))
    }
}
