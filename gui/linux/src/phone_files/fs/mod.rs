//! The FUSE file system over the phone's files, [`PhoneFs`], whose requests the session's threads serve
//! at once.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_files::PhoneFilesApi;
use tokio::runtime::Handle;

use super::inodes::Inodes;
use super::requests::Requests;
use super::staging::Created;
use fetch::Fetch;

mod attrs;
mod entries;
mod fetch;
mod filesystem;
mod io;
mod new_files;
mod tables;
#[cfg(test)]
mod tests;

/// How long the kernel may keep entries and attributes before it asks again.
const TTL: Duration = Duration::from_secs(1);

/// An open file handle. The files are shared and read and written with `read_at`/`write_at`, so no
/// lock on the handle table is held during IO.
#[derive(Clone)]
enum Open {
    /// Opened for reading: a phone file's downloaded content, or a created file's staging file.
    Read(Arc<File>),
    /// The handle `create` returned: it writes the staging file and uploads it on `flush`.
    Created { ino: u64, created: Arc<Created> },
}

/// The file system the session's threads serve at once: its tables are behind locks held only for
/// lookups and updates, never across a download, an upload or file IO.
pub(super) struct PhoneFs {
    engine: Arc<Engine>,
    runtime: Handle,
    domain: String,
    /// The folder new files are staged in; `None` when neither `XDG_CACHE_HOME` nor `HOME` names one.
    uploads: Option<PathBuf>,
    /// Shared with the mount's `MountLink`, which tells the kernel what a change on the phone made stale.
    inodes: Arc<Mutex<Inodes>>,
    /// Shared with the mount's `MountLink`, which cancels them when the mount goes away.
    requests: Arc<Requests>,
    opened: Mutex<HashMap<u64, Open>>,
    /// Running downloads, by item identifier.
    fetches: Mutex<HashMap<String, Arc<Fetch>>>,
    next_handle: AtomicU64,
    /// Numbers engine requests and staging files.
    serial: AtomicU64,
    uid: u32,
    gid: u32,
}

impl PhoneFs {
    /// The file system of the phone `domain`'s files from `engine`, its new files staged in `uploads`,
    /// owned by the user and group this process runs as.
    pub(super) fn new(
        engine: Arc<Engine>,
        runtime: Handle,
        domain: String,
        uploads: Option<PathBuf>,
        inodes: Arc<Mutex<Inodes>>,
        requests: Arc<Requests>,
    ) -> Self {
        let (uid, gid) = process_owner();
        Self {
            engine,
            runtime,
            domain,
            uploads,
            inodes,
            requests,
            opened: Mutex::default(),
            fetches: Mutex::default(),
            next_handle: AtomicU64::new(1),
            serial: AtomicU64::new(1),
            uid,
            gid,
        }
    }

    fn files(&self) -> Arc<PhoneFilesApi> {
        self.engine.clone().phone_files()
    }

    fn serial(&self) -> u64 {
        self.serial.fetch_add(1, Ordering::Relaxed)
    }

    fn request_id(&self) -> String {
        format!("fuse-{}", self.serial())
    }
}

/// The user and the group this process runs as, who own every item of the mount.
#[expect(unsafe_code, reason = "getuid and getgid have no safe wrapper in the dependencies")]
fn process_owner() -> (u32, u32) {
    (unsafe { libc::getuid() }, unsafe { libc::getgid() })
}
