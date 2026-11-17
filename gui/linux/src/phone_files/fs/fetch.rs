//! Downloads of phone files for the opens that read them, and the engine transfers they and the
//! uploads run as.

use std::collections::hash_map::Entry;
use std::fs::File;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use fuser::Errno;
use phonelink_core::sync::MutexExt;
use phonelink_engine::api::phone_files::{PhoneFileError, PhoneFileProgress};
use tracing::{error, warn};

use super::PhoneFs;
use crate::phone_files::errno::errno;
use crate::phone_files::requesters::{Requesters, watch_requesters};

/// The progress of a download nothing shows.
struct Silent;

impl PhoneFileProgress for Silent {
    fn progress(&self, _done: u64, _total: u64) {}
}

/// A download that concurrent opens of one file share. The engine downloads a phone's files one at a
/// time, and refuses a second download of a file while one runs.
pub(super) struct Fetch {
    result: Mutex<Option<Result<Arc<File>, Errno>>>,
    done: Condvar,
    /// The processes whose opens wait for it.
    requesters: Arc<Requesters>,
}

impl Fetch {
    fn new() -> Arc<Self> {
        Arc::new(Self { result: Mutex::new(None), done: Condvar::new(), requesters: Requesters::new() })
    }
}

impl PhoneFs {
    /// The content of the phone file `identifier`, for an open by the thread `pid`. Concurrent opens of
    /// the file share one download.
    pub(super) fn fetch(&self, identifier: &str, pid: u32) -> Result<Arc<File>, Errno> {
        let (fetch, first) = match self.fetches.lock_or_recover().entry(identifier.to_owned()) {
            Entry::Occupied(entry) => (entry.get().clone(), false),
            Entry::Vacant(entry) => (entry.insert(Fetch::new()).clone(), true),
        };
        fetch.requesters.add(pid);

        if first {
            let result = self.download(identifier, fetch.requesters.clone());
            // An open from now on downloads the file again: it may have changed on the phone.
            self.fetches.lock_or_recover().remove(identifier);
            *fetch.result.lock_or_recover() = Some(result.clone());
            fetch.done.notify_all();
            return result;
        }

        let mut result = fetch.result.lock_or_recover();
        loop {
            if let Some(outcome) = result.as_ref() {
                return outcome.clone();
            }
            result = fetch.done.wait(result).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Runs the engine request `request` (a download or an upload) to its end, cancelled when every one
    /// of `requesters` is being killed or the mount goes away.
    pub(super) fn transfer<T>(
        &self,
        request: &str,
        requesters: Arc<Requesters>,
        call: impl Future<Output = Result<T, PhoneFileError>>,
    ) -> Result<T, PhoneFileError> {
        let _running = self.requests.begin(request);
        let _watch = watch_requesters(requesters, self.files(), request.to_owned());
        self.runtime.block_on(call)
    }

    /// Downloads the phone file `identifier` and opens it. The open file keeps the content, so the
    /// engine's temporary file is removed at once.
    fn download(&self, identifier: &str, requesters: Arc<Requesters>) -> Result<Arc<File>, Errno> {
        let files = self.files();
        let request = self.request_id();
        let download = files.download(self.domain.clone(), identifier.to_owned(), request.clone(), Arc::new(Silent));
        let content = self.transfer(&request, requesters, download).map_err(|e| {
            warn!(identifier, error = %e, "a phone file was not downloaded");
            errno(&e)
        })?;

        let opened = File::open(&content.path);
        if let Err(e) = std::fs::remove_file(&content.path) {
            warn!(error = %e, "a downloaded phone file's temporary copy was not removed");
        }
        let file = opened.map_err(|e| {
            error!(error = %e, "a downloaded phone file was not opened");
            Errno::from(e)
        })?;
        Ok(Arc::new(file))
    }
}
