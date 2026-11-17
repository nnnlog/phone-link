//! The file handles: opening, reading, writing, flushing and releasing them.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::sync::PoisonError;

use fuser::{Errno, FileHandle, FopenFlags, INodeNo, OpenFlags, ReplyData, ReplyEmpty, ReplyOpen, ReplyWrite};
use phonelink_core::sync::MutexExt;
use tracing::warn;

use super::{Open, PhoneFs};
use crate::phone_files::staging::Stage;

/// Reads into `buffer` from `offset` until it is full or the file ends. FUSE pads a short answer with
/// zeros up to the size the kernel asked for, so only the end of the file may answer short.
fn read_full(file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
    let mut read = 0;
    while read < buffer.len() {
        match file.read_at(&mut buffer[read..], offset + read as u64) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(read)
}

impl PhoneFs {
    /// `destroy`: the mount went away while new files were still open, so their staging files go too.
    pub(super) fn discard_open_staging(&mut self) {
        let opened = std::mem::take(self.opened.get_mut().unwrap_or_else(PoisonError::into_inner));
        for open in opened.into_values() {
            if let Open::Created { created, .. } = open
                && let Err(e) = std::fs::remove_file(&created.staging)
            {
                warn!(error = %e, "a staged new file was not removed");
            }
        }
    }

    /// `open`: a handle reading `ino`, for the thread `pid`; a phone file is downloaded for it.
    pub(super) fn open_file(&self, ino: INodeNo, flags: OpenFlags, pid: u32, reply: ReplyOpen) {
        // Only the handle `create` returns writes, and only to a new file: the phone has no way to
        // replace a file's content. Nothing on the phone changes.
        if flags.0 & libc::O_ACCMODE != libc::O_RDONLY {
            return reply.error(Errno::EACCES);
        }

        // A created file not yet uploaded is read from its staging file.
        let file = match self.created(ino.0) {
            Some(created) => Ok(created.file.clone()),
            None => self.identifier(ino).and_then(|identifier| self.fetch(&identifier, pid)),
        };
        match file {
            Ok(file) => reply.opened(self.handle(Open::Read(file)), FopenFlags::empty()),
            Err(e) => reply.error(e),
        }
    }

    /// `read`: up to `size` bytes of the handle `fh`'s file from `offset`.
    pub(super) fn read_file(&self, fh: FileHandle, offset: u64, size: u32, reply: ReplyData) {
        let file = match self.opened(fh) {
            Some(Open::Read(file)) => file,
            Some(Open::Created { created, .. }) => created.file.clone(),
            None => return reply.error(Errno::EBADF),
        };

        let mut buffer = vec![0; size as usize];
        match read_full(&file, &mut buffer, offset) {
            Ok(read) => reply.data(&buffer[..read]),
            Err(e) => reply.error(Errno::from(e)),
        }
    }

    /// `write`: `data` at `offset` in the staging file of the created file the handle `fh` writes.
    pub(super) fn write_created(&self, fh: FileHandle, offset: u64, data: &[u8], reply: ReplyWrite) {
        let Some(Open::Created { created, .. }) = self.opened(fh) else { return reply.error(Errno::EBADF) };
        let stage = created.stage.lock_or_recover();
        // Uploaded at an earlier close of a duplicate descriptor: the phone's copy cannot change.
        if matches!(*stage, Stage::Uploaded) {
            return reply.error(Errno::EACCES);
        }

        match created.file.write_all_at(data, offset) {
            Ok(()) => reply.written(data.len() as u32),
            Err(e) => reply.error(Errno::from(e)),
        }
    }

    /// `flush`, at every `close()` of a descriptor: the first one after the file was created uploads it,
    /// and the upload's error is `close()`'s.
    pub(super) fn flush_file(&self, fh: FileHandle, pid: u32, reply: ReplyEmpty) {
        match self.opened(fh) {
            Some(Open::Created { ino, created }) => match self.upload(ino, &created, pid) {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(e),
            },
            Some(Open::Read(_)) => reply.ok(),
            None => reply.error(Errno::EBADF),
        }
    }

    /// `release`: the handle `fh` goes, and with a created file's handle its staging file.
    pub(super) fn release_handle(&self, fh: FileHandle, reply: ReplyEmpty) {
        // The upload ran in flush. What is left is the handle and the staging file; a created file not
        // uploaded by now never existed on the phone, so its name goes too.
        let open = self.opened.lock_or_recover().remove(&fh.0);
        if let Some(Open::Created { ino, created }) = open {
            self.inodes.lock_or_recover().pending.remove(&ino);
            if let Err(e) = std::fs::remove_file(&created.staging) {
                warn!(error = %e, "a staged new file was not removed");
            }
        }
        reply.ok();
    }
}
