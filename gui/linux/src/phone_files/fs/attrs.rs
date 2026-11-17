//! The attributes the kernel gets of the root, the phone's items and the created files, and the
//! attribute changes the mount takes.

use std::fs::File;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{Errno, FileAttr, FileType, INodeNo, ReplyAttr, TimeOrNow};
use phonelink_core::sync::MutexExt;
use phonelink_engine::api::phone_files::PhoneFileItem;

use super::{PhoneFs, TTL};
use crate::phone_files::errno::errno;
use crate::phone_files::inodes::ROOT;
use crate::phone_files::staging::{Stage, restage};

/// The permission bits: files are read-only, as the phone cannot replace their content; folders take
/// new files and folders.
pub(super) fn permissions(directory: bool) -> u16 {
    if directory { 0o755 } else { 0o444 }
}

fn time(seconds: Option<i64>) -> SystemTime {
    seconds.and_then(|s| u64::try_from(s).ok()).map_or(UNIX_EPOCH, |s| UNIX_EPOCH + Duration::from_secs(s))
}

impl PhoneFs {
    fn attr(&self, ino: u64, directory: bool, size: u64, created: SystemTime, modified: SystemTime) -> FileAttr {
        FileAttr {
            ino: INodeNo(ino),
            size,
            blocks: size.div_ceil(512),
            atime: modified,
            mtime: modified,
            ctime: modified,
            crtime: created,
            kind: if directory { FileType::Directory } else { FileType::RegularFile },
            perm: permissions(directory),
            nlink: if directory { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    pub(super) fn item_attr(&self, ino: u64, item: &PhoneFileItem) -> FileAttr {
        let size = u64::try_from(item.size).unwrap_or_default();
        self.attr(ino, item.is_directory, size, time(item.created), time(item.modified))
    }

    fn root_attr(&self) -> FileAttr {
        self.attr(ROOT, true, 0, UNIX_EPOCH, SystemTime::now())
    }

    /// A created file's attributes, from its staging file: its size so far and its times.
    pub(super) fn staged_attr(&self, ino: u64, file: &File) -> Result<FileAttr, Errno> {
        let metadata = file.metadata()?;
        let modified = metadata.modified()?;
        // Not every file system keeps a birth time.
        let created = metadata.created().unwrap_or(modified);
        Ok(self.attr(ino, false, metadata.len(), created, modified))
    }

    /// The attributes of `ino`: the root, a created file not yet uploaded, or a phone item.
    pub(super) fn attr_of(&self, ino: INodeNo) -> Result<FileAttr, Errno> {
        if ino.0 == ROOT {
            return Ok(self.root_attr());
        }
        if let Some(created) = self.created(ino.0) {
            return self.staged_attr(ino.0, &created.file);
        }

        let identifier = self.identifier(ino)?;
        let item = self.files().item(self.domain.clone(), identifier).map_err(|e| errno(&e))?;
        Ok(self.item_attr(ino.0, &item))
    }

    /// `setattr`: a created file takes a new size and times; a phone file refuses a new size and keeps
    /// its other attributes.
    pub(super) fn set_attr(
        &self,
        ino: INodeNo,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        reply: ReplyAttr,
    ) {
        // A created file not yet uploaded takes a new size and times in its staging file.
        if let Some(created) = self.created(ino.0) {
            let stage = created.stage.lock_or_recover();
            if !matches!(*stage, Stage::Uploaded) {
                let restaged = restage(&created.file, size, atime, mtime);
                drop(stage);
                return match restaged.map_err(Errno::from).and_then(|()| self.staged_attr(ino.0, &created.file)) {
                    Ok(attr) => reply.attr(&TTL, &attr),
                    Err(e) => reply.error(e),
                };
            }
        }

        let attr = match self.attr_of(ino) {
            Ok(attr) => attr,
            Err(e) => return reply.error(e),
        };
        // The phone has no way to change a file's content, its size included.
        if size.is_some_and(|size| size != attr.size) {
            return reply.error(Errno::EACCES);
        }
        // Mode, owner and times have no way to the phone either. They stay as they are rather than fail
        // the request, so that copies which carry them over still succeed.
        reply.attr(&TTL, &attr);
    }
}
