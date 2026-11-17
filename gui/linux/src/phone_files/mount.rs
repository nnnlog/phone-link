//! Mounting the phone's files at `~/PhoneLink`: the folder, the FUSE session and its options, and the
//! link that reports the phone's changes to the kernel.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use fuser::{BackgroundSession, Config, MountOption};
use phonelink_engine::api::engine::Engine;
use tokio::runtime::Handle;
use tracing::{error, info, warn};

use super::MountLink;
use super::fs::PhoneFs;
use super::inodes::Inodes;
use super::link::report_changes;
use super::requests::Requests;
use super::staging::staging_dir;

/// Request threads. A download or an upload blocks the thread it runs on until it ends, so the others
/// keep listing folders and reading opened files meanwhile.
const THREADS: usize = 4;

/// The folder the phone's files are mounted at: `PhoneLink` in the home folder.
pub(crate) fn mount_point() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("PhoneLink")
}

/// Mounts the files of the phone `domain` from `engine`, served on `runtime`; `None` when the folder
/// was not made or the mount failed, which is logged.
pub(crate) fn mount(engine: Arc<Engine>, runtime: Handle, domain: String) -> Option<(BackgroundSession, MountLink)> {
    let point = mount_point();
    // A mount an earlier run left behind (it ended without unmounting) would keep the folder busy.
    let _ = std::process::Command::new("fusermount3").arg("-u").arg("-q").arg(&point).status();
    if let Err(e) = std::fs::create_dir_all(&point) {
        error!(path = %point.display(), error = %e, "the phone files folder was not made");
        return None;
    }
    let uploads = staging_dir(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"));
    if uploads.is_none() {
        warn!("neither XDG_CACHE_HOME nor HOME is an absolute path: new files cannot be copied to the phone");
    }

    let mounted_engine = Arc::downgrade(&engine);
    let files = engine.clone().phone_files();
    // The changes the kernel hears about start from the tree as it is now.
    let generation = files
        .generation(domain.clone())
        .inspect_err(|e| warn!(error = %e, "the phone files' generation was not read"))
        .ok();
    let inodes = Arc::new(Mutex::new(Inodes::default()));
    let requests = Arc::new(Requests::default());
    let filesystem = PhoneFs::new(engine, runtime, domain.clone(), uploads, inodes.clone(), requests.clone());

    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::FSName("phonelink".to_owned()),
        MountOption::Subtype("phonelink".to_owned()),
        // The kernel enforces the permission bits, so programs that ask before writing (access(2))
        // see phone files as read-only.
        MountOption::DefaultPermissions,
        MountOption::NoExec,
        MountOption::NoSuid,
        MountOption::NoDev,
    ];
    config.n_threads = Some(THREADS);
    // Each thread reads requests from its own clone of the FUSE descriptor instead of all contending
    // for one.
    config.clone_fd = true;
    let session = match fuser::spawn_mount(filesystem, &point, &config) {
        Ok(session) => session,
        Err(e) => {
            error!(path = %point.display(), error = %e, "the phone files were not mounted");
            return None;
        }
    };
    info!(path = %point.display(), "phone files mounted");

    let (changed, changes) = mpsc::channel();
    let notifier = session.notifier();
    let reporter_files = files.clone();
    let reporter_domain = domain.clone();
    thread::spawn(move || report_changes(changes, reporter_files, reporter_domain, inodes, notifier, generation));
    Some((session, MountLink::new(domain, mounted_engine, files, requests, changed)))
}
