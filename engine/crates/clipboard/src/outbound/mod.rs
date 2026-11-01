//! PC → phone announcements (outbound tag 9).
//!
//! It covers change detection, debounce and echo suppression (poll the change count, ignore the
//! count of the app's own last write, 200 ms trailing debounce), announcing (gates, classification,
//! the SHA-256 content hash, the dedupe) and the announcement message.
//!
//! The steps of announcing, which comments here cite by number:
//! 1. Gates: the account is enrolled, the global switch is on, and some phone supports the clipboard.
//! 2. Classification: the pasteboard's highest recognized kind; nothing is announced without one.
//! 3. Hash: the SHA-256 content hash of what is announced.
//! 4. Dedupe: content whose hash was the last one announced is not announced again.
//! 5. Publish: one announcement to every connected phone.

use std::sync::Arc;

use base64::Engine as _;
use sha2::{Digest, Sha256};
use tokio::time::{Instant, MissedTickBehavior};
use tracing::{debug, info, warn};

use super::image::announcement_png;
use super::pasteboard::{ClipKind, ClipboardPasteboard, ReadError, classify, read_image, read_plain_text};
use super::{Clipboard, ClipboardEvent, blocking, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use phonelink_transport::platform::PublishOutcome;

/// What the change-count poll compares against.
#[derive(Debug, Default)]
pub(super) struct WatchState {
    /// The last change count seen; `None` until the first successful read, which is the baseline
    /// (content present when watching starts is not a change).
    pub last_seen: Option<i64>,
    /// The change count the pasteboard had once the app's latest write was complete.
    pub own_write: Option<i64>,
    /// The last read failed (logged when a streak starts and when it ends).
    pub failing: bool,
}

/// The content hash: base64 (standard, padded) of SHA-256.
pub fn content_hash(bytes: &[u8]) -> String {
    base64::prelude::BASE64_STANDARD.encode(Sha256::digest(bytes))
}

impl Clipboard {
    /// One poll: a change count that differs from the last one seen is a change, unless it equals the
    /// count recorded for the app's own latest write. Reads no content. Blocking.
    fn poll_change(&self) -> bool {
        let mut watch = self.watch.lock_or_panic();
        let Some(count) = read_change_count(&*self.pasteboard, &mut watch.failing) else {
            return false;
        };

        match watch.last_seen.replace(count) {
            None => false,
            Some(last) if last == count => false,
            Some(_) if watch.own_write == Some(count) => {
                debug!(change_count = count, "pasteboard change is the engine's own write; ignored");
                false
            }
            Some(_) => true,
        }
    }

    /// Announces the pasteboard, when the debounce fires.
    async fn announce(self: Arc<Self>) {
        let guard = self.announcing.lock().await;
        if self.is_stopped() {
            return;
        }
        if !self.announcement_gates_open() {
            return;
        }

        // 2–3. Classify and hash.
        let me = self.clone();
        let Some(Some((kind, hash))) = blocking("pasteboard read", move || me.read_for_announcement()).await else {
            return;
        };

        if self.is_stopped() {
            debug!(?kind, "feature stopped while the pasteboard was read; no announcement");
            return;
        }
        if !self.record_new_hash(kind, hash) {
            return;
        }

        drop(guard);
        self.publish_announcement(kind).await;
    }

    /// Step 1: account enrolled, global switch on, some phone supported (the platform layer is up:
    /// the feature is installed after it). Logs why an announcement is not made.
    fn announcement_gates_open(&self) -> bool {
        if !self.peers.is_enrolled() {
            debug!("account not enrolled; no announcement");
            return false;
        }
        self.global_switch_on() && self.some_phone_supported()
    }

    /// The global switch, `false` (logged) when it is off or unreadable.
    fn global_switch_on(&self) -> bool {
        match self.store.global_switch() {
            Ok(true) => true,
            Ok(false) => {
                debug!("global switch off; no announcement");
                false
            }
            Err(e) => {
                warn!(error = %e, "global switch unreadable; no announcement");
                false
            }
        }
    }

    /// Whether some phone supports the clipboard, `false` (logged) when not or unreadable.
    fn some_phone_supported(&self) -> bool {
        match self.any_phone_supported() {
            Ok(true) => true,
            Ok(false) => {
                debug!("no phone supports the clipboard; no announcement");
                false
            }
            Err(e) => {
                warn!(error = %e, "settings unreadable; no announcement");
                false
            }
        }
    }

    /// Step 4: dedupe; the hash is stored before sending, whatever the send's result. `false` when
    /// the content was already announced.
    fn record_new_hash(&self, kind: ClipKind, hash: String) -> bool {
        let mut last = self.last_hash.lock_or_panic();
        if last.as_deref() == Some(hash.as_str()) {
            debug!(?kind, "pasteboard content already announced; skipped");
            return false;
        }
        *last = Some(hash);
        true
    }

    /// Step 5: one announcement to every connected phone with a context source; the result is
    /// logged. A shutdown while the targets are being found abandons it before anything is sent.
    async fn publish_announcement(&self, kind: ClipKind) {
        let correlation_id = new_guid_lower();
        let handle = tokio::select! {
            biased;
            _ = self.shutdown.cancelled() => {
                debug!(?kind, "feature stopped before the announcement went out");
                return;
            }
            handle = self.msaep.publish(wire::TAG, wire::announcement(&correlation_id)) => handle,
        };

        info!(?kind, %correlation_id, message_id = %handle.message_id, target_count = handle.targets.len(), "PC clipboard announced");
        self.emit(ClipboardEvent::Announced { kind });

        tokio::spawn(async move {
            match handle.outcome().await {
                outcome @ (PublishOutcome::Success | PublishOutcome::NoResults) => {
                    debug!(%correlation_id, ?outcome, "announcement finished");
                }
                outcome @ (PublishOutcome::PartialSuccess | PublishOutcome::Failure) => {
                    warn!(%correlation_id, ?outcome, "announcement not delivered to every phone");
                }
            }
        });
    }

    /// Steps 2–3: the kind and the content hash, `None` when nothing is recognized or the read
    /// fails. Blocking.
    fn read_for_announcement(&self) -> Option<(ClipKind, String)> {
        let types = match self.pasteboard.types() {
            Ok(types) => types,
            Err(e) => {
                warn!(error = %e, "pasteboard types unreadable; no announcement");
                return None;
            }
        };

        let Some(kind) = classify(&types) else {
            debug!(raw_types = ?types, "nothing recognized on the pasteboard; no announcement");
            return None;
        };

        let bytes = match kind {
            // HTML is announced by the hash of its plain-text representation; the HTML is not read.
            ClipKind::Text | ClipKind::Html => self.announcement_text(kind)?,
            ClipKind::Image => self.announcement_image()?,
        };

        Some((kind, content_hash(&bytes)))
    }

    /// The plain text a `kind` clip is announced by, `None` (logged) when it cannot be read or is
    /// over the read limit. Blocking.
    fn announcement_text(&self, kind: ClipKind) -> Option<Vec<u8>> {
        match read_plain_text(&*self.pasteboard, self.config.text_read_limit) {
            Ok(text) => Some(text.into_bytes()),
            Err(e @ ReadError::TooLong { .. }) => {
                info!(?kind, error = %e, "copied text over the read limit; no announcement");
                None
            }
            Err(e) => {
                warn!(?kind, error = %e, "copied text not read; no announcement");
                None
            }
        }
    }

    /// The PNG an image on the pasteboard is announced by, `None` (logged) when it cannot be read or
    /// decoded. Blocking.
    fn announcement_image(&self) -> Option<Vec<u8>> {
        let image = match read_image(&*self.pasteboard) {
            Ok(image) => image,
            Err(e) => {
                warn!(error = %e, "copied image not read; no announcement");
                return None;
            }
        };

        match announcement_png(&*self.codec, &image) {
            Ok(png) => Some(png),
            Err(e) => {
                warn!(error = %e, "copied image does not decode; no announcement");
                None
            }
        }
    }
}

/// Starts the watcher: poll the change count every `poll_interval`; each detected change re-arms the
/// `debounce` timer; when it fires, announce.
pub(super) fn spawn_watcher(clipboard: &Arc<Clipboard>) {
    let weak = Arc::downgrade(clipboard);
    let shutdown = clipboard.shutdown.clone();
    let (poll_interval, debounce) = (clipboard.config.poll_interval, clipboard.config.debounce);

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(poll_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut deadline: Option<Instant> = None;

        // A poll that panicked, logged once per streak: the poll runs every `poll_interval`.
        let mut poll_panicked = false;

        loop {
            let fire_at = deadline.unwrap_or_else(Instant::now);
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let Some(c) = weak.upgrade() else { break };
                    if poll(c, &mut poll_panicked).await {
                        deadline = Some(Instant::now() + debounce);
                    }
                }
                _ = tokio::time::sleep_until(fire_at), if deadline.is_some() => {
                    deadline = None;
                    let Some(c) = weak.upgrade() else { break };
                    tokio::spawn(c.announce());
                }
            }
        }

        debug!("pasteboard watcher stopped");
    });
}

/// The pasteboard's change count, `None` when it is unreadable; a streak of failed reads (`failing`)
/// is logged when it starts and when it ends.
fn read_change_count(pasteboard: &dyn ClipboardPasteboard, failing: &mut bool) -> Option<i64> {
    match pasteboard.change_count() {
        Ok(count) => {
            if std::mem::replace(failing, false) {
                info!("pasteboard change count readable again");
            }
            Some(count)
        }
        Err(e) => {
            if !std::mem::replace(failing, true) {
                warn!(error = %e, "pasteboard change count unreadable; no announcement until it reads again");
            }
            None
        }
    }
}

/// One change-count poll off the async runtime; `false` when it panicked, which is logged when a
/// streak of panics starts.
async fn poll(clipboard: Arc<Clipboard>, panicked: &mut bool) -> bool {
    match tokio::task::spawn_blocking(move || clipboard.poll_change()).await {
        Ok(changed) => {
            *panicked = false;
            changed
        }
        Err(e) => {
            if !std::mem::replace(panicked, true) {
                warn!(error = %e, "pasteboard change-count poll panicked; no announcement, repeats are not logged");
            }
            false
        }
    }
}

#[cfg(test)]
mod tests;
