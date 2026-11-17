//! The processes waiting for a download or an upload, watched while it runs so that it is cancelled
//! once every one of them is being killed.

use std::io;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use phonelink_core::sync::MutexExt;
use phonelink_engine::api::phone_files::PhoneFilesApi;
use tracing::{info, warn};

/// How often a running download or upload checks whether the processes waiting for it are being killed.
const REQUESTER_CHECK: Duration = Duration::from_millis(500);

/// Whether `/proc/<pid>/status` text shows a SIGKILL pending for the thread (`SigPnd`) or its process
/// (`ShdPnd`): the kernel ends a process for any fatal signal that way.
pub(super) fn kill_pending(status: &str) -> bool {
    const SIGKILL_BIT: u64 = 1 << (libc::SIGKILL - 1);
    status
        .lines()
        .filter_map(|line| line.strip_prefix("SigPnd:").or_else(|| line.strip_prefix("ShdPnd:")))
        .filter_map(|mask| u64::from_str_radix(mask.trim(), 16).ok())
        .any(|mask| mask & SIGKILL_BIT != 0)
}

/// Whether the thread `pid` that made a request is being killed, or is gone.
fn dying(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/status")) {
        Ok(status) => kill_pending(&status),
        Err(e) if e.kind() == io::ErrorKind::NotFound => true,
        Err(e) => {
            warn!(pid, error = %e, "the status of a process waiting for a phone file was not read");
            false
        }
    }
}

/// The processes waiting for one download or upload, watched while it runs: once every one of them is
/// being killed, the transfer is cancelled. Only processes alive when they started waiting count, so a
/// file closed by a process that is already exiting still goes to the phone.
pub(super) struct Requesters {
    pids: Mutex<Vec<u32>>,
}

impl Requesters {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self { pids: Mutex::new(Vec::new()) })
    }

    /// Adds the thread `pid` unless it is already being killed. The kernel gives 0 for a process this
    /// app's process namespace cannot see; one it cannot watch is never taken for gone.
    pub(super) fn add(&self, pid: u32) {
        if pid != 0 && !dying(pid) {
            self.pids.lock_or_recover().push(pid);
        }
    }

    /// Every watched process is being killed (and there is one).
    fn all_dying(&self) -> bool {
        let pids = self.pids.lock_or_recover().clone();
        !pids.is_empty() && pids.into_iter().all(dying)
    }
}

/// Watches `requesters` while the engine request `request` runs, and cancels it when they are all being
/// killed. The watch ends when the returned sender is dropped.
pub(super) fn watch_requesters(requesters: Arc<Requesters>, files: Arc<PhoneFilesApi>, request: String) -> Sender<()> {
    let (stop, stopped) = mpsc::channel::<()>();
    thread::spawn(move || {
        loop {
            match stopped.recv_timeout(REQUESTER_CHECK) {
                Err(RecvTimeoutError::Timeout) if requesters.all_dying() => {
                    info!(request, "every process waiting for a phone file transfer is being killed; it is cancelled");
                    files.cancel(request);
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    stop
}
