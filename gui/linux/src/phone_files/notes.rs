//! The changes to the kernel's caches that the phone's tree changes call for.

use std::ffi::OsStr;
use std::io;

use fuser::{INodeNo, Notifier};

/// A change to the kernel's caches, as the phone's tree changed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Note {
    /// Forget the entry `name` in the folder `parent` (a negative one included).
    Entry { parent: u64, name: String },
    /// Forget the attributes and the cached content of `ino`.
    Inode { ino: u64 },
    /// The item `child`, known as `name` in `parent`, is gone: watchers of the folder hear a deletion.
    Delete { parent: u64, child: u64, name: String },
}

impl Note {
    pub(super) fn send(&self, notifier: &Notifier) -> io::Result<()> {
        match self {
            Note::Entry { parent, name } => notifier.inval_entry(INodeNo(*parent), OsStr::new(name)),
            Note::Inode { ino } => notifier.inval_inode(INodeNo(*ino), 0, 0),
            Note::Delete { parent, child, name } => {
                notifier.delete(INodeNo(*parent), INodeNo(*child), OsStr::new(name))
            }
        }
    }
}
