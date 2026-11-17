//! The folders' entries: looking names up, listing folders, and making, deleting and renaming items on
//! the phone.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use fuser::{Errno, FileType, Generation, INodeNo, ReplyDirectory, ReplyEmpty, ReplyEntry};
use phonelink_core::sync::MutexExt;
use phonelink_engine::api::phone_files::PhoneFileItem;

use super::{PhoneFs, TTL};
use crate::phone_files::errno::errno;
use crate::phone_files::inodes::ROOT;
use crate::phone_files::staging::Stage;

/// Whether `name` is a trash folder: `.Trash`, or `.Trash-<uid>` as GIO and KIO name a user's own.
fn is_trash(name: &OsStr) -> bool {
    let name = name.as_bytes();
    name == b".Trash" || name.starts_with(b".Trash-")
}

/// Whether a new item `name` in the folder `parent` is refused: a trash folder in the root. File
/// managers move a deleted file of another file system into such a folder at its top; when they cannot
/// make one, they offer to delete the file instead.
pub(super) fn refused(parent: INodeNo, name: &OsStr) -> bool {
    parent.0 == ROOT && is_trash(name)
}

impl PhoneFs {
    fn child_of(&self, parent: &str, name: &str) -> Result<PhoneFileItem, Errno> {
        self.files()
            .children(self.domain.clone(), parent.to_owned())
            .map_err(|e| errno(&e))?
            .into_iter()
            .find(|item| item.name == name)
            .ok_or(Errno::ENOENT)
    }

    fn child(&self, parent: INodeNo, name: &OsStr) -> Result<PhoneFileItem, Errno> {
        let parent = self.identifier(parent)?;
        self.child_of(&parent, &name.to_string_lossy())
    }

    /// `lookup`: the item `name` in the folder `parent_ino`, a created file not yet uploaded included.
    pub(super) fn look_up(&self, parent_ino: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent = match self.identifier(parent_ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let name = name.to_string_lossy();

        match self.child_of(&parent, &name) {
            Ok(item) => {
                let ino = {
                    let mut inodes = self.inodes.lock_or_recover();
                    let ino = inodes.of(&item.identifier);
                    inodes.record(ino, parent_ino.0, &name);
                    ino
                };
                reply.entry(&TTL, &self.item_attr(ino, &item), Generation(0));
            }
            // A created file is found from its staging file until its upload.
            Err(e) if e == Errno::ENOENT => {
                let named = self.inodes.lock_or_recover().named(&parent, &name);
                match named.map(|(ino, created)| self.staged_attr(ino, &created.file)) {
                    Some(Ok(attr)) => {
                        self.inodes.lock_or_recover().record(attr.ino.0, parent_ino.0, &name);
                        reply.entry(&TTL, &attr, Generation(0));
                    }
                    Some(Err(e)) => reply.error(e),
                    None => reply.error(Errno::ENOENT),
                }
            }
            Err(e) => reply.error(e),
        }
    }

    /// `readdir`: the entries of the folder `ino` from `offset` on, the created files not yet uploaded
    /// included.
    pub(super) fn list(&self, ino: INodeNo, offset: u64, mut reply: ReplyDirectory) {
        let identifier = match self.identifier(ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let children = match self.files().children(self.domain.clone(), identifier.clone()) {
            Ok(children) => children,
            Err(e) => return reply.error(errno(&e)),
        };

        let mut entries =
            vec![(ino.0, FileType::Directory, ".".to_owned()), (ROOT, FileType::Directory, "..".to_owned())];
        {
            let mut inodes = self.inodes.lock_or_recover();
            for item in children {
                let kind = if item.is_directory { FileType::Directory } else { FileType::RegularFile };
                entries.push((inodes.of(&item.identifier), kind, item.name));
            }
            // Created files are listed until their upload, as lookups find them; once the phone has the
            // name, its item is listed instead.
            let mut created = inodes.listed_in(&identifier);
            created.retain(|(_, name)| !entries.iter().any(|(_, _, taken)| taken == name));
            entries.extend(created.into_iter().map(|(ino, name)| (ino, FileType::RegularFile, name)));
        }

        for (index, (entry, kind, name)) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(INodeNo(entry), index as u64 + 1, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    /// `mkdir`: a new folder `name` on the phone, in the folder `parent_ino`.
    pub(super) fn make_folder(&self, parent_ino: INodeNo, name: &OsStr, reply: ReplyEntry) {
        if refused(parent_ino, name) {
            return reply.error(Errno::EPERM);
        }
        let parent = match self.identifier(parent_ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };

        let files = self.files();
        let name = name.to_string_lossy().into_owned();
        match self.runtime.block_on(files.create_folder(self.domain.clone(), parent, name.clone())) {
            Ok(item) => {
                let ino = {
                    let mut inodes = self.inodes.lock_or_recover();
                    let ino = inodes.of(&item.identifier);
                    inodes.record(ino, parent_ino.0, &name);
                    ino
                };
                reply.entry(&TTL, &self.item_attr(ino, &item), Generation(0));
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    /// `unlink`: the file `name` in the folder `parent` is deleted from the phone, or never goes there
    /// when it is a created file not yet uploaded.
    pub(super) fn unlink_file(&self, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let parent = match self.identifier(parent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let name = name.to_string_lossy();
        let named = self.inodes.lock_or_recover().named(&parent, &name);
        if let Some((ino, created)) = named {
            let mut stage = created.stage.lock_or_recover();
            if matches!(*stage, Stage::Staged) {
                // Never uploaded, so it only ever existed here. Its open handle keeps the staging file
                // until released, and its closes upload nothing.
                *stage = Stage::Discarded;
                let mut inodes = self.inodes.lock_or_recover();
                if let Some(pending) = inodes.pending.get_mut(&ino) {
                    pending.listed = false;
                }
                inodes.entries.remove(&ino);
                return reply.ok();
            }
            // Uploaded meanwhile: it is deleted from the phone like any other file.
        }

        let item = match self.child_of(&parent, &name) {
            Ok(item) => item,
            Err(e) => return reply.error(e),
        };
        // The engine deletes a folder with everything in it; unlink is for files.
        if item.is_directory {
            return reply.error(Errno::EISDIR);
        }
        let files = self.files();
        match self.runtime.block_on(files.delete(self.domain.clone(), item.identifier.clone())) {
            Ok(()) => {
                self.forget_entry(&item.identifier);
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    /// `rmdir`: the empty folder `name` in the folder `parent` is deleted from the phone.
    pub(super) fn remove_folder(&self, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let item = match self.child(parent, name) {
            Ok(item) => item,
            Err(e) => return reply.error(e),
        };
        if !item.is_directory {
            return reply.error(Errno::ENOTDIR);
        }

        let files = self.files();
        // The engine deletes a folder with everything in it; rmdir deletes an empty folder only.
        match files.children(self.domain.clone(), item.identifier.clone()) {
            Ok(children) if !children.is_empty() => return reply.error(Errno::ENOTEMPTY),
            Ok(_) => {}
            Err(e) => return reply.error(errno(&e)),
        }
        if !self.inodes.lock_or_recover().listed_in(&item.identifier).is_empty() {
            return reply.error(Errno::ENOTEMPTY);
        }

        match self.runtime.block_on(files.delete(self.domain.clone(), item.identifier.clone())) {
            Ok(()) => {
                self.forget_entry(&item.identifier);
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    /// `rename`: the item `name` in the folder `parent` becomes `newname` in the folder `newparent` on
    /// the phone.
    pub(super) fn rename_item(
        &self,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        reply: ReplyEmpty,
    ) {
        let new_parent_ino = newparent.0;
        if refused(newparent, newname) {
            return reply.error(Errno::EPERM);
        }
        let parent = match self.identifier(parent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        let name = name.to_string_lossy();
        // A created file has no item on the phone to rename until its upload.
        if self.inodes.lock_or_recover().named(&parent, &name).is_some() {
            return reply.error(Errno::EBUSY);
        }
        let item = match self.child_of(&parent, &name) {
            Ok(item) => item,
            Err(e) => return reply.error(e),
        };
        let target = match self.identifier(newparent) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };

        let files = self.files();
        let newname = newname.to_string_lossy().into_owned();
        match self.runtime.block_on(files.rename(self.domain.clone(), item.identifier.clone(), target, newname.clone()))
        {
            Ok(_) => {
                // The kernel moved its entry along: a change report of this move finds nothing stale.
                let mut inodes = self.inodes.lock_or_recover();
                if let Some(ino) = inodes.by_id.get(&item.identifier).copied() {
                    inodes.record(ino, new_parent_ino, &newname);
                }
                drop(inodes);
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }
}
