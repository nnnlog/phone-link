//! The app's side of a mount: the phone's tree changes reported to the kernel, and the mount's
//! transfers cancelled when the mount goes away.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use fuser::Notifier;
use phonelink_core::sync::MutexExt;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_files::PhoneFilesApi;
use tracing::warn;

use super::inodes::Inodes;
use super::notes::Note;
use super::requests::Requests;

/// How long an unmount waits for the downloads and uploads it cancelled to end, so that none still
/// holds the folder busy when it is unmounted.
const CANCEL_WAIT: Duration = Duration::from_secs(2);

/// The app's side of a mount: it reports the phone's tree changes to the kernel, and cancels the
/// mount's running downloads and uploads when it is dropped, before the mount itself goes.
pub(crate) struct MountLink {
    domain: String,
    /// The engine the mount serves the phone's files from.
    engine: Weak<Engine>,
    files: Arc<PhoneFilesApi>,
    requests: Arc<Requests>,
    /// Wakes the thread that turns the tree's changes into kernel notifications; it ends with the link.
    changed: Sender<()>,
}

impl MountLink {
    /// The link of the mount of the phone `domain`'s files from `engine`, whose reporting thread
    /// `changed` wakes.
    pub(super) fn new(
        domain: String,
        engine: Weak<Engine>,
        files: Arc<PhoneFilesApi>,
        requests: Arc<Requests>,
        changed: Sender<()>,
    ) -> Self {
        Self { domain, engine, files, requests, changed }
    }

    /// Whether the mount shows the files of the phone `domain` from `engine`.
    pub(crate) fn serves(&self, domain: &str, engine: &Arc<Engine>) -> bool {
        self.domain.eq_ignore_ascii_case(domain)
            && self.engine.upgrade().is_some_and(|mounted| Arc::ptr_eq(&mounted, engine))
    }

    /// The tree of the phone `domain` changed.
    pub(crate) fn items_changed(&self, domain: &str) {
        if domain.eq_ignore_ascii_case(&self.domain) {
            // The thread ends only with the link, so the send cannot fail while it exists.
            let _ = self.changed.send(());
        }
    }
}

impl Drop for MountLink {
    fn drop(&mut self) {
        self.requests.cancel_all(&self.files, CANCEL_WAIT);
    }
}

/// Turns each change of the tree into the kernel notifications it calls for, until the link is dropped.
/// It runs on a thread of its own: a notification waits for the folder it names, which a request of the
/// mount may hold while it waits for the phone.
pub(super) fn report_changes(
    changed: Receiver<()>,
    files: Arc<PhoneFilesApi>,
    domain: String,
    inodes: Arc<Mutex<Inodes>>,
    notifier: Notifier,
    mut generation: Option<i64>,
) {
    while changed.recv().is_ok() {
        // Changes that arrived meanwhile are read together.
        while changed.try_recv().is_ok() {}

        let since = match generation {
            Some(since) => files.changes(domain.clone(), since),
            None => Ok(None),
        };
        let notes = match since {
            Ok(Some(changes)) => {
                generation = Some(changes.generation);
                inodes.lock_or_recover().notes_for(&changes)
            }
            // Too old to tell what changed (or never read): everything the kernel holds goes.
            Ok(None) => {
                generation = files
                    .generation(domain.clone())
                    .inspect_err(|e| warn!(error = %e, "the phone files' generation was not read"))
                    .ok();
                inodes.lock_or_recover().notes_for_all()
            }
            Err(e) => {
                warn!(error = %e, "the phone files' changes were not read; the kernel's caches expire on their own");
                continue;
            }
        };

        for note in notes {
            let sent = match (note.send(&notifier), &note) {
                // A removed folder the kernel still holds entries in (the phone reported only the
                // folder): it is dropped from the cache instead.
                (Err(e), Note::Delete { parent, name, .. }) if e.raw_os_error() == Some(libc::ENOTEMPTY) => {
                    Note::Entry { parent: *parent, name: name.clone() }.send(&notifier)
                }
                (sent, _) => sent,
            };
            if let Err(e) = sent {
                warn!(?note, error = %e, "the kernel was not told about a change of the phone's files");
            }
        }
    }
}
