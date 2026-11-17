//! The inode and handle tables as the requests read and change them.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use fuser::{Errno, FileHandle, INodeNo};
use phonelink_core::sync::MutexExt;

use super::{Open, PhoneFs};
use crate::phone_files::staging::Created;

impl PhoneFs {
    /// The phone identifier of `ino` (`""` is the root).
    pub(super) fn identifier(&self, ino: INodeNo) -> Result<String, Errno> {
        self.inodes.lock_or_recover().identifier(ino.0).ok_or(Errno::ENOENT)
    }

    /// The created file `ino`, until its upload or the release of its handle.
    pub(super) fn created(&self, ino: u64) -> Option<Arc<Created>> {
        self.inodes.lock_or_recover().created(ino)
    }

    /// The kernel dropped its entry of the phone item `identifier`: it was deleted through this mount.
    pub(super) fn forget_entry(&self, identifier: &str) {
        let mut inodes = self.inodes.lock_or_recover();
        if let Some(ino) = inodes.by_id.get(identifier).copied() {
            inodes.entries.remove(&ino);
        }
    }

    /// A new handle of `open`.
    pub(super) fn handle(&self, open: Open) -> FileHandle {
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.opened.lock_or_recover().insert(handle, open);
        FileHandle(handle)
    }

    pub(super) fn opened(&self, fh: FileHandle) -> Option<Open> {
        self.opened.lock_or_recover().get(&fh.0).cloned()
    }
}
