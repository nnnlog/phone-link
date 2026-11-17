//! The inode table: the inode numbers handed to the kernel for the phone's items and the files created
//! on the mount, the entries the kernel got for them, and what it must forget when the phone's tree
//! changes.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use phonelink_engine::api::phone_files::PhoneFileChanges;

use super::notes::Note;
use super::staging::Created;

/// The root's inode number, which FUSE fixes.
pub(super) const ROOT: u64 = 1;

/// A created file in the inode table, from `create` until its upload or the release of its handle.
pub(super) struct Pending {
    pub(super) created: Arc<Created>,
    /// Found by name in lookups and listings; unlinking it ends that, while its handle stays open.
    pub(super) listed: bool,
}

/// The inodes handed to the kernel. A phone item keeps one inode while the app runs. A created file has
/// an inode of its own, which names the phone's item once its upload ends.
#[derive(Default)]
pub(super) struct Inodes {
    by_ino: HashMap<u64, String>,
    pub(super) by_id: HashMap<String, u64>,
    pub(super) pending: HashMap<u64, Pending>,
    /// The entry the kernel last got for an inode (a lookup, a new file or folder, a rename): the folder
    /// and the name it may cache, which a change on the phone makes stale.
    pub(super) entries: HashMap<u64, (u64, String)>,
    next: u64,
}

impl Inodes {
    pub(super) fn fresh(&mut self) -> u64 {
        self.next = self.next.max(ROOT) + 1;
        self.next
    }

    /// The inode of the phone item `identifier` (`""` is the root), handed out on first use.
    pub(super) fn of(&mut self, identifier: &str) -> u64 {
        if identifier.is_empty() {
            return ROOT;
        }
        if let Some(ino) = self.by_id.get(identifier) {
            return *ino;
        }

        let ino = self.fresh();
        self.by_ino.insert(ino, identifier.to_owned());
        self.by_id.insert(identifier.to_owned(), ino);
        ino
    }

    pub(super) fn identifier(&self, ino: u64) -> Option<String> {
        if ino == ROOT { Some(String::new()) } else { self.by_ino.get(&ino).cloned() }
    }

    pub(super) fn created(&self, ino: u64) -> Option<Arc<Created>> {
        self.pending.get(&ino).map(|pending| pending.created.clone())
    }

    /// The listed created file `name` in the folder `parent`.
    pub(super) fn named(&self, parent: &str, name: &str) -> Option<(u64, Arc<Created>)> {
        self.pending
            .iter()
            .find(|(_, p)| p.listed && p.created.parent == parent && p.created.name == name)
            .map(|(ino, p)| (*ino, p.created.clone()))
    }

    /// The listed created files in the folder `parent`, in inode order so that listings are stable.
    pub(super) fn listed_in(&self, parent: &str) -> Vec<(u64, String)> {
        let mut listed: Vec<(u64, String)> = self
            .pending
            .iter()
            .filter(|(_, p)| p.listed && p.created.parent == parent)
            .map(|(ino, p)| (*ino, p.created.name.clone()))
            .collect();
        listed.sort_unstable();
        listed
    }

    /// The created file `ino` is on the phone as `identifier`: the inode names that item from now on.
    pub(super) fn settle(&mut self, ino: u64, identifier: &str) {
        self.pending.remove(&ino);
        self.by_ino.insert(ino, identifier.to_owned());
        // A listing that ran after the upload ended may have given the item an inode already; that one
        // stays the item's for later lookups, and this one keeps working for the open handles.
        self.by_id.entry(identifier.to_owned()).or_insert(ino);
    }

    /// The kernel got `ino` as `name` in the folder `parent`.
    pub(super) fn record(&mut self, ino: u64, parent: u64, name: &str) {
        self.entries.insert(ino, (parent, name.to_owned()));
    }

    /// The inode of the folder `identifier` when the kernel was given one (`""` is the root).
    fn known(&self, identifier: &str) -> Option<u64> {
        if identifier.is_empty() { Some(ROOT) } else { self.by_id.get(identifier).copied() }
    }

    /// How many folders down from the root the kernel knows `ino`, by its recorded entries.
    fn depth(&self, ino: u64) -> usize {
        let mut depth = 0;
        let mut current = ino;
        // A folder cannot be its own ancestor; the bound only keeps a broken record from looping.
        while let Some((parent, _)) = self.entries.get(&current)
            && depth < self.entries.len()
        {
            depth += 1;
            current = *parent;
        }
        depth
    }

    /// What the kernel must forget after `changes`. An updated item's attributes and content go; its
    /// old entry goes too when it was renamed or moved (a file's as a deletion, which file managers
    /// show), and a cached miss of its new name is dropped. A removed item is a deletion of the entry the
    /// kernel knew, the deepest first, so that a folder is empty in the kernel's cache when its turn
    /// comes.
    pub(super) fn notes_for(&mut self, changes: &PhoneFileChanges) -> Vec<Note> {
        let mut notes = Vec::new();
        for item in &changes.updated {
            let parent = self.known(&item.parent);
            if let Some(ino) = self.by_id.get(&item.identifier).copied() {
                notes.push(Note::Inode { ino });
                let moved = self
                    .entries
                    .get(&ino)
                    .is_some_and(|(old_parent, old_name)| parent != Some(*old_parent) || item.name != *old_name);
                if moved && let Some((old_parent, old_name)) = self.entries.remove(&ino) {
                    // A moved folder keeps what the kernel holds inside it, so its old entry is dropped
                    // rather than deleted.
                    notes.push(if item.is_directory {
                        Note::Entry { parent: old_parent, name: old_name }
                    } else {
                        Note::Delete { parent: old_parent, child: ino, name: old_name }
                    });
                    notes.push(Note::Inode { ino: old_parent });
                }
            }
            if let Some(parent) = parent {
                notes.push(Note::Entry { parent, name: item.name.clone() });
                notes.push(Note::Inode { ino: parent });
            }
        }

        let mut removed: Vec<(usize, u64)> = changes
            .removed
            .iter()
            .filter_map(|identifier| self.by_id.get(identifier).copied())
            .filter(|ino| self.entries.contains_key(ino))
            .map(|ino| (self.depth(ino), ino))
            .collect();
        removed.sort_unstable_by(|a, b| b.cmp(a));
        for (_, ino) in removed {
            if let Some((parent, name)) = self.entries.remove(&ino) {
                notes.push(Note::Delete { parent, child: ino, name });
                notes.push(Note::Inode { ino: parent });
            }
        }

        let mut seen = HashSet::new();
        notes.retain(|note| seen.insert(note.clone()));
        notes
    }

    /// What the kernel must forget when the changes are unknown: every entry and inode it was given.
    pub(super) fn notes_for_all(&self) -> Vec<Note> {
        let mut entries: Vec<(&u64, &(u64, String))> = self.entries.iter().collect();
        entries.sort_unstable();
        let mut inodes: Vec<u64> = self.by_ino.keys().copied().chain([ROOT]).collect();
        inodes.sort_unstable();
        entries
            .into_iter()
            .map(|(_, (parent, name))| Note::Entry { parent: *parent, name: name.clone() })
            .chain(inodes.into_iter().map(|ino| Note::Inode { ino }))
            .collect()
    }
}
