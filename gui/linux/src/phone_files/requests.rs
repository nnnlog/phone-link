//! The downloads and uploads running for the mount, so that an unmount cancels them.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use phonelink_core::sync::MutexExt;
use phonelink_engine::api::phone_files::PhoneFilesApi;
use tracing::{info, warn};

/// The downloads and uploads running for the mount, by engine request id, so that an unmount cancels
/// them.
#[derive(Default)]
pub(super) struct Requests {
    running: Mutex<HashSet<String>>,
    ended: Condvar,
}

impl Requests {
    pub(super) fn begin(&self, id: &str) -> RunningRequest<'_> {
        self.running.lock_or_recover().insert(id.to_owned());
        RunningRequest { requests: self, id: id.to_owned() }
    }

    /// Cancels every running request and waits up to `wait` for them to end.
    pub(super) fn cancel_all(&self, files: &PhoneFilesApi, wait: Duration) {
        let mut running = self.running.lock_or_recover();
        if running.is_empty() {
            return;
        }
        info!(count = running.len(), "phone file transfers are cancelled: the phone's files are unmounted");
        for id in running.iter() {
            files.cancel(id.clone());
        }

        let deadline = Instant::now() + wait;
        while !running.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                warn!(
                    count = running.len(),
                    "cancelled phone file transfers had not ended when the files were unmounted"
                );
                return;
            }
            running = self.ended.wait_timeout(running, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

/// A request in [`Requests`] until it is dropped.
pub(super) struct RunningRequest<'a> {
    requests: &'a Requests,
    id: String,
}

impl Drop for RunningRequest<'_> {
    fn drop(&mut self) {
        let mut running = self.requests.running.lock_or_recover();
        running.remove(&self.id);
        if running.is_empty() {
            self.requests.ended.notify_all();
        }
    }
}
