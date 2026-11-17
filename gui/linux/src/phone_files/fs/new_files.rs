//! New files: made in a private staging file by `create`, and uploaded to the phone by the first close
//! of a descriptor of them.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::{Arc, Mutex};

use fuser::{Errno, FopenFlags, Generation, INodeNo, ReplyCreate};
use phonelink_core::sync::MutexExt;
use tracing::{error, warn};

use super::entries::refused;
use super::{Open, PhoneFs, TTL};
use crate::phone_files::errno::errno;
use crate::phone_files::inodes::Pending;
use crate::phone_files::requesters::Requesters;
use crate::phone_files::staging::{Created, Stage, private_dir};

impl PhoneFs {
    /// Whether this mount's phone is the selected phone and connected. The engine holds an upload until
    /// the phone is back, and the kernel waits for a flush's answer even when the closing process is
    /// killed, so without the phone a new file fails at once instead of leaving `close()` waiting.
    fn reachable(&self) -> bool {
        self.files().state().is_ok_and(|state| state.available && state.domain.eq_ignore_ascii_case(&self.domain))
    }

    /// A new, empty staging file in the private uploads folder, readable and writable by the user alone.
    fn stage(&self) -> Result<(String, File), Errno> {
        let Some(dir) = &self.uploads else {
            error!("no folder to stage new files in: neither XDG_CACHE_HOME nor HOME is an absolute path");
            return Err(Errno::EIO);
        };
        if let Err(e) = private_dir(dir) {
            error!(folder = %dir.display(), error = %e, "the folder for new files was not made private");
            return Err(Errno::from(e));
        }

        loop {
            let path = dir.join(format!("{}-{}", std::process::id(), self.serial()));
            let Some(staging) = path.to_str() else {
                error!(folder = %dir.display(), "the folder for new files is not a UTF-8 path, as the engine needs");
                return Err(Errno::EIO);
            };
            match OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path) {
                Ok(file) => return Ok((staging.to_owned(), file)),
                // Left behind by an earlier run under the same process id: the next name is free.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    error!(folder = %dir.display(), error = %e, "a new file was not staged");
                    return Err(Errno::from(e));
                }
            }
        }
    }

    /// `create`: a new file `name` in the folder `parent_ino`, staged until its upload.
    pub(super) fn create_file(&self, parent_ino: INodeNo, name: &OsStr, reply: ReplyCreate) {
        if refused(parent_ino, name) {
            return reply.error(Errno::EPERM);
        }
        let parent = match self.identifier(parent_ino) {
            Ok(identifier) => identifier,
            Err(e) => return reply.error(e),
        };
        // Without the phone the upload at close could only fail: fail before the content is written.
        if !self.reachable() {
            warn!("the phone is not connected: a new file was not made");
            return reply.error(Errno::EIO);
        }

        let (staging, file) = match self.stage() {
            Ok(staged) => staged,
            Err(e) => return reply.error(e),
        };
        let mut attr = match self.staged_attr(0, &file) {
            Ok(attr) => attr,
            Err(e) => {
                let _ = std::fs::remove_file(&staging);
                return reply.error(e);
            }
        };
        let name = name.to_string_lossy().into_owned();
        let created =
            Arc::new(Created { parent, name, staging, file: Arc::new(file), stage: Mutex::new(Stage::Staged) });

        let ino = {
            let mut inodes = self.inodes.lock_or_recover();
            // Two files of one name cannot both reach the phone.
            if inodes.named(&created.parent, &created.name).is_some() {
                None
            } else {
                let ino = inodes.fresh();
                inodes.pending.insert(ino, Pending { created: created.clone(), listed: true });
                inodes.record(ino, parent_ino.0, &created.name);
                Some(ino)
            }
        };
        let Some(ino) = ino else {
            let _ = std::fs::remove_file(&created.staging);
            return reply.error(Errno::EEXIST);
        };

        attr.ino = INodeNo(ino);
        let handle = self.handle(Open::Created { ino, created });
        reply.created(&TTL, &attr, Generation(0), handle, FopenFlags::empty());
    }

    /// Uploads the created file `ino` while it is staged, for a close by the thread `pid`. Its stage
    /// lock is held for the whole upload, so a write, a size change or another close of the same file
    /// waits for the outcome; at most one upload of the file succeeds, and a failed one is tried again
    /// by the next close.
    pub(super) fn upload(&self, ino: u64, created: &Created, pid: u32) -> Result<(), Errno> {
        let mut stage = created.stage.lock_or_recover();
        if !matches!(*stage, Stage::Staged) {
            return Ok(());
        }
        if !self.reachable() {
            warn!("the phone is not connected: a new file was not sent to it");
            return Err(Errno::EIO);
        }

        let files = self.files();
        let request = self.request_id();
        // No times: the engine sends the staging file's own, the last write's or those a copy set.
        let upload = files.upload(
            self.domain.clone(),
            created.parent.clone(),
            created.name.clone(),
            created.staging.clone(),
            None,
            None,
            request.clone(),
        );
        let requesters = Requesters::new();
        requesters.add(pid);
        match self.transfer(&request, requesters, upload) {
            Ok(item) => {
                *stage = Stage::Uploaded;
                self.inodes.lock_or_recover().settle(ino, &item.identifier);
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "a new file was not sent to the phone");
                Err(errno(&e))
            }
        }
    }
}
