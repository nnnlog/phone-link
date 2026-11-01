//! The two fetch dispatchers of a phone: the scheduling of the thumbnail and full-resolution
//! fetches (PC → phone), the common rules of outbound `/media/v1` requests, and their trace context
//! names.
//!
//! Each keeps a priority stack of ids ([`Pool`]): adding ids pushes them so that the first id of a
//! call ends on top; re-adding an id moves it to the top and keeps its state; picking takes the
//! topmost queued id whose cached item still lacks the data and drops every queued id it passes over;
//! an id stays in flight until a reply handles it or its entry is cancelled. The thumbnail dispatcher
//! runs one worker that waits 1000 ms, then sends batches of up to 25 one at a time until a batch
//! comes out empty, each named after the trigger that started the worker; the full-resolution
//! dispatcher sends one id per request with up to 5 in flight, each named after the trigger that
//! queued the id.
//! A failed attempt re-queues the id, the 4th failure gives it up, and a stop-all outcome clears the
//! stack. The pause before each batch or request is `min(min · 2^consecutiveFailures, max)` since the
//! last request — `0 ms` by default: retries at once, as Phone Link on Windows does.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tracing::{debug, error, info, warn};

use phonelink_app::{AppOutcome, AppResponse};
use phonelink_core::types::TraceContext;

use super::{PhoneState, PhotosEvent, PhotosInner, store, wire};
use phonelink_core::sync::MutexExt;

/// An entry's phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    /// Sent (or picked to be sent) and not yet handled.
    Requested,
}

/// One id of a dispatcher's stack.
#[derive(Debug, Clone)]
pub struct Entry {
    pub id: i64,
    pub phase: Phase,
    pub failures: u32,
    /// The entry's trace context; a full-resolution entry's retries reuse its correlation id.
    pub trace: TraceContext,
    /// The `ypTriggerId` of the trigger that queued the entry; re-adding the id keeps it. A
    /// full-resolution request carries it; a thumbnail batch carries its worker's trigger instead.
    pub trigger: &'static str,
}

/// An entry [`Pool::pick`] marked requested.
#[derive(Debug, Clone)]
pub struct Picked {
    pub id: i64,
    pub trace: TraceContext,
    pub trigger: &'static str,
}

/// The priority stack of one dispatcher (top = the end of the vector).
#[derive(Debug, Default)]
pub struct Pool {
    stack: Vec<Entry>,
}

/// What [`Pool::fail`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failed {
    /// Counted and queued again.
    Requeued,
    /// The entry's last allowed attempt failed: removed.
    GaveUp,
    /// No entry for the id (cancelled meanwhile).
    NotQueued,
}

impl Pool {
    /// Pushes `ids` so that the first ends on top; an id already in the stack moves to the top and
    /// keeps its phase, failure count, trace and trigger. `trace` makes a new entry's trace context;
    /// `trigger` names a new entry's requests.
    pub fn add(&mut self, ids: &[i64], trigger: &'static str, mut trace: impl FnMut() -> TraceContext) {
        for &id in ids.iter().rev() {
            match self.stack.iter().position(|e| e.id == id) {
                Some(i) => {
                    let e = self.stack.remove(i);
                    self.stack.push(e);
                }
                None => self.stack.push(Entry { id, phase: Phase::Queued, failures: 0, trace: trace(), trigger }),
            }
        }
    }

    /// Removes `id`'s entry; `true` when there was one.
    pub fn remove(&mut self, id: i64) -> bool {
        match self.stack.iter().position(|e| e.id == id) {
            Some(i) => {
                self.stack.remove(i);
                true
            }
            None => false,
        }
    }

    /// Removes every entry; returns their ids.
    pub fn clear(&mut self) -> Vec<i64> {
        self.stack.drain(..).map(|e| e.id).collect()
    }

    /// Counts a failed attempt of `id` and queues it again; when the count before this failure
    /// already exceeds `allowed_failures`, the entry is given up (so at most `allowed_failures + 2`
    /// attempts: 4 with the default 2).
    pub fn fail(&mut self, id: i64, allowed_failures: u32) -> Failed {
        let Some(i) = self.stack.iter().position(|e| e.id == id) else {
            return Failed::NotQueued;
        };

        let before = self.stack[i].failures;
        self.stack[i].failures += 1;
        self.stack[i].phase = Phase::Queued;

        if before > allowed_failures {
            self.stack.remove(i);
            Failed::GaveUp
        } else {
            Failed::Requeued
        }
    }

    pub fn requested_count(&self) -> usize {
        self.stack.iter().filter(|e| e.phase == Phase::Requested).count()
    }

    pub fn has_queued(&self) -> bool {
        self.stack.iter().any(|e| e.phase == Phase::Queued)
    }

    /// Scans from the top for a queued entry whose id is in `lacking` (cached and still without the
    /// data), marks it requested and returns it; every queued entry passed over on the way (data
    /// present, or no longer cached) is dropped and returned in the second list. Requested entries
    /// are skipped.
    pub fn pick(&mut self, lacking: &HashSet<i64>) -> (Option<Picked>, Vec<i64>) {
        let mut dropped = Vec::new();
        let mut found = None;
        for e in self.stack.iter().rev() {
            if e.phase != Phase::Queued {
                continue;
            }
            if lacking.contains(&e.id) {
                found = Some(e.id);
                break;
            }
            dropped.push(e.id);
        }

        self.stack.retain(|e| !dropped.contains(&e.id));
        let picked = found.and_then(|id| self.stack.iter_mut().find(|e| e.id == id)).map(|e| {
            e.phase = Phase::Requested;
            Picked { id: e.id, trace: e.trace.clone(), trigger: e.trigger }
        });
        (picked, dropped)
    }

    #[cfg(feature = "test-support")]
    /// The ids from top to bottom.
    pub fn ids(&self) -> Vec<i64> {
        self.stack.iter().rev().map(|e| e.id).collect()
    }

    #[cfg(feature = "test-support")]
    pub fn entry(&self, id: i64) -> Option<&Entry> {
        self.stack.iter().find(|e| e.id == id)
    }
}

/// The thumbnail dispatcher's state.
#[derive(Debug, Default)]
pub struct ThumbState {
    pub pool: Pool,
    /// A worker task is running.
    pub worker: bool,
    pub consecutive_failures: u32,
    pub last_request: Option<Instant>,
}

/// The full-resolution dispatcher's state.
#[derive(Debug, Default)]
pub struct ImageState {
    pub pool: Pool,
    /// A dispatch task is spawned and has not picked yet.
    pub dispatch_pending: bool,
    pub consecutive_failures: u32,
    pub last_request: Option<Instant>,
}

/// The stop-all outcomes.
///
/// Needs permission, needs initial permission, metered sync disabled, all features disabled,
/// two-phone mode, non-default profile — also a send or connection that failed with *metered sync
/// disabled* or *LTW disabled*.
pub fn is_stop_all(outcome: &AppOutcome) -> bool {
    matches!(
        outcome,
        AppOutcome::NeedsPermission
            | AppOutcome::NeedsInitialPermission
            | AppOutcome::AllFeaturesDisabled
            | AppOutcome::TwoPhoneMode
            | AppOutcome::NonDefaultProfile
    ) || outcome.is_metered_sync_disabled()
        || outcome.is_ltw_disabled()
}

/// Why one fetch attempt failed.
#[derive(Debug, Clone, Copy)]
enum AttemptFailure<'a> {
    /// The phone's outcome was not a success.
    Outcome(&'a AppOutcome),
    /// A success whose payload is missing or does not parse, for this reason.
    Malformed(&'static str),
}

impl std::fmt::Display for AttemptFailure<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Outcome(o) => write!(f, "{o:?}"),
            Self::Malformed(reason) => write!(f, "malformed reply: {reason}"),
        }
    }
}

/// The cached ids that lack a thumbnail; `None` (the worker ends) when the store is unreadable.
fn lacking_thumbnails(ps: &PhoneState) -> Option<HashSet<i64>> {
    match store::ids_without_thumbnail(&ps.db) {
        Ok(l) => Some(l),
        Err(e) => {
            error!(phone = %ps.phone, error = %e, "photos store unreadable; thumbnail worker stops");
            ps.thumbs.lock_or_panic().worker = false;
            None
        }
    }
}

/// The pause still to wait before the next batch or request.
pub fn pause(min: Duration, max: Duration, consecutive_failures: u32, last_request: Option<Instant>) -> Duration {
    let factor = 1u32.checked_shl(consecutive_failures).unwrap_or(u32::MAX);
    let cooldown = min.saturating_mul(factor).min(max);
    match last_request {
        Some(t) => cooldown.saturating_sub(t.elapsed()),
        None => Duration::ZERO,
    }
}

impl PhotosInner {
    /// Queues thumbnail fetches (first id on top) and starts the worker when none runs; a worker this
    /// call starts names every batch it sends after `trigger`.
    pub(crate) fn request_thumbnails(self: &Arc<Self>, ps: &Arc<PhoneState>, ids: &[i64], trigger: &'static str) {
        if ids.is_empty() {
            return;
        }

        let start = {
            let mut st = ps.thumbs.lock_or_panic();
            st.pool.add(ids, trigger, || TraceContext::new_root(Vec::new()));
            !std::mem::replace(&mut st.worker, true)
        };

        if start {
            let (this, ps) = (self.clone(), ps.clone());
            self.runtime.spawn(async move { this.thumbnail_worker(ps, trigger).await });
        }
    }

    /// Queues full-resolution fetches (first id on top), named after `trigger`, and dispatches.
    pub(crate) fn request_full(self: &Arc<Self>, ps: &Arc<PhoneState>, ids: &[i64], trigger: &'static str) {
        if ids.is_empty() {
            return;
        }
        let mut st = ps.images.lock_or_panic();
        st.pool.add(ids, trigger, || TraceContext::new_root(Vec::new()));
        self.try_dispatch_full(ps, &mut st);
    }

    /// Local cancellation (no message): removes the entries; a request already sent completes and
    /// its reply is still applied.
    pub(crate) fn cancel_thumbnails(&self, ps: &PhoneState, ids: &[i64]) {
        let mut st = ps.thumbs.lock_or_panic();
        for id in ids {
            st.pool.remove(*id);
        }
    }

    pub(crate) fn cancel_full(&self, ps: &PhoneState, ids: &[i64]) {
        let mut st = ps.images.lock_or_panic();
        for id in ids {
            st.pool.remove(*id);
        }
    }

    pub(crate) fn cancel_all_thumbnails(&self, ps: &PhoneState) -> Vec<i64> {
        ps.thumbs.lock_or_panic().pool.clear()
    }

    pub(crate) fn cancel_all_full(&self, ps: &PhoneState) -> Vec<i64> {
        ps.images.lock_or_panic().pool.clear()
    }

    async fn thumbnail_worker(self: Arc<Self>, ps: Arc<PhoneState>, trigger: &'static str) {
        tokio::time::sleep(self.config.thumbnail_initial_delay).await;
        let root = TraceContext::new_root(Vec::new());

        loop {
            let wait = self.thumbnail_pause(&ps);
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }

            let Some(lacking) = lacking_thumbnails(&ps) else { return };
            let (batch, dropped) = self.pick_thumbnail_batch(&ps, &lacking);
            self.settled(&ps, wire::content_type::THUMBNAILS, &dropped);

            if batch.is_empty() {
                return;
            }

            debug!(phone = %ps.phone, count = batch.len(), "thumbnail batch");
            match self.send(&ps.phone, wire::thumbnails_request(&batch, root.child(), trigger)).await {
                Some(response) => self.handle_thumbnails(&ps, &batch, response),
                None => {
                    info!(phone = %ps.phone, "thumbnail fetches cancelled; the phone is no longer selected");
                    let mut st = ps.thumbs.lock_or_panic();
                    st.pool.clear();
                    st.worker = false;
                    return;
                }
            }
        }
    }

    /// The pause before the next thumbnail batch.
    fn thumbnail_pause(&self, ps: &PhoneState) -> Duration {
        let st = ps.thumbs.lock_or_panic();
        pause(self.config.retry_pause_min, self.config.retry_pause_max, st.consecutive_failures, st.last_request)
    }

    /// Picks the next batch of ids lacking a thumbnail, and the queued ids passed over; an empty batch
    /// ends the worker.
    fn pick_thumbnail_batch(&self, ps: &PhoneState, lacking: &HashSet<i64>) -> (Vec<i64>, Vec<i64>) {
        let mut st = ps.thumbs.lock_or_panic();
        let mut batch = Vec::new();
        let mut dropped = Vec::new();

        while batch.len() < self.config.thumbnail_batch {
            let (picked, d) = st.pool.pick(lacking);
            dropped.extend(d);
            match picked {
                Some(picked) => batch.push(picked.id),
                None => break,
            }
        }

        if batch.is_empty() {
            // Under the same lock as the check, so a later request starts a new worker.
            st.worker = false;
        } else {
            st.last_request = Some(Instant::now());
        }
        (batch, dropped)
    }

    fn handle_thumbnails(self: &Arc<Self>, ps: &Arc<PhoneState>, batch: &[i64], response: AppResponse) {
        if response.outcome.is_success() {
            let failure = match response.values().map(wire::parse_items) {
                Some(Ok(items)) => return self.thumbnails_received(ps, batch, items.items),
                Some(Err(m)) => AttemptFailure::Malformed(m.reason),
                None => AttemptFailure::Malformed("no payload"),
            };
            return self.thumbnail_batch_failed(ps, batch, failure);
        }

        if is_stop_all(&response.outcome) {
            warn!(phone = %ps.phone, outcome = ?response.outcome, error = response.outcome.cause(), "thumbnail fetches stopped");
            ps.thumbs.lock_or_panic().pool.clear();
            return;
        }

        self.thumbnail_batch_failed(ps, batch, AttemptFailure::Outcome(&response.outcome));
    }

    /// A failed attempt of `batch`: each id is re-queued or given up.
    fn thumbnail_batch_failed(&self, ps: &PhoneState, batch: &[i64], failure: AttemptFailure<'_>) {
        let gave_up = {
            let mut st = ps.thumbs.lock_or_panic();
            st.consecutive_failures = st.consecutive_failures.saturating_add(1);

            let mut gave_up = Vec::new();
            for id in batch {
                if st.pool.fail(*id, self.config.allowed_failures) == Failed::GaveUp {
                    gave_up.push(*id);
                }
            }
            gave_up
        };

        if gave_up.is_empty() {
            debug!(phone = %ps.phone, error = %failure, "thumbnail batch failed; queued again");
        } else {
            warn!(phone = %ps.phone, error = %failure, gave_up_count = gave_up.len(), "thumbnail batch failed; thumbnails given up");
        }

        self.settled(ps, wire::content_type::THUMBNAILS, &gave_up);
    }

    fn thumbnails_received(self: &Arc<Self>, ps: &Arc<PhoneState>, batch: &[i64], items: Vec<wire::MediaItem>) {
        let (to_store, gave_up) = {
            let mut st = ps.thumbs.lock_or_panic();
            st.consecutive_failures = 0;
            if items.is_empty() {
                // Not found: every id of the batch is done.
                for id in batch {
                    st.pool.remove(*id);
                }
                (Vec::new(), Vec::new())
            } else {
                let mut to_store = Vec::new();
                let mut gave_up = Vec::new();
                let allowed = self.config.allowed_failures;

                for item in items {
                    if item.thumbnail.is_none() {
                        if st.pool.fail(item.id, allowed) == Failed::GaveUp {
                            gave_up.push(item.id);
                        }
                    } else {
                        st.pool.remove(item.id);
                        to_store.push(item);
                    }
                }
                (to_store, gave_up)
            }
        };

        self.settled(ps, wire::content_type::THUMBNAILS, &gave_up);
        if to_store.is_empty() {
            return;
        }

        if let Err(e) = store::upsert_items(&ps.db, &to_store, self.now_ms()) {
            error!(phone = %ps.phone, error = %e, "thumbnails not stored");
            return;
        }

        // A fetch reply is not the phone's data: an item a delete took out of the gallery stays out.
        let ids: Vec<i64> = to_store.iter().map(|i| i.id).collect();
        self.settled(ps, wire::content_type::THUMBNAILS, &ids);
        self.emit(PhotosEvent::GalleryChanged { phone: ps.phone.clone() });
    }

    /// Spawns a dispatch when none is pending and fewer than the concurrency limit are in flight.
    fn try_dispatch_full(self: &Arc<Self>, ps: &Arc<PhoneState>, st: &mut ImageState) {
        if !st.dispatch_pending && st.pool.requested_count() < self.config.full_concurrency {
            st.dispatch_pending = true;
            let (this, ps) = (self.clone(), ps.clone());
            self.runtime.spawn(async move { this.dispatch_full(ps).await });
        }
    }

    async fn dispatch_full(self: Arc<Self>, ps: Arc<PhoneState>) {
        let wait = {
            let st = ps.images.lock_or_panic();
            pause(self.config.retry_pause_min, self.config.retry_pause_max, st.consecutive_failures, st.last_request)
        };

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }

        let lacking = match store::ids_without_full(&ps.db) {
            Ok(l) => l,
            Err(e) => {
                error!(phone = %ps.phone, error = %e, "photos store unreadable; full-resolution dispatch stops");
                ps.images.lock_or_panic().dispatch_pending = false;
                return;
            }
        };

        let (picked, dropped) = {
            let mut st = ps.images.lock_or_panic();
            st.dispatch_pending = false;
            let (picked, dropped) = st.pool.pick(&lacking);
            if picked.is_some() {
                st.last_request = Some(Instant::now());
                if st.pool.has_queued() {
                    self.try_dispatch_full(&ps, &mut st);
                }
            }
            (picked, dropped)
        };

        self.settled(&ps, wire::content_type::PHOTOS, &dropped);
        let Some(Picked { id, trace, trigger }) = picked else { return };
        let request = wire::photos_request(id, trace, trigger);

        match self.send(&ps.phone, request).await {
            Some(response) => self.handle_full(&ps, id, response),
            None => {
                info!(phone = %ps.phone, "full-resolution fetches cancelled; the phone is no longer selected");
                ps.images.lock_or_panic().pool.clear();
                return;
            }
        }

        let mut st = ps.images.lock_or_panic();
        if st.pool.has_queued() {
            self.try_dispatch_full(&ps, &mut st);
        }
    }

    fn handle_full(self: &Arc<Self>, ps: &Arc<PhoneState>, id: i64, response: AppResponse) {
        if response.outcome.is_success() {
            let failure = match response.values().map(wire::parse_full_reply) {
                Some(Ok(images)) => return self.full_received(ps, id, images),
                Some(Err(m)) => AttemptFailure::Malformed(m.reason),
                None => AttemptFailure::Malformed("no payload"),
            };
            return self.full_fetch_failed(ps, id, failure);
        }

        if is_stop_all(&response.outcome) {
            warn!(phone = %ps.phone, outcome = ?response.outcome, error = response.outcome.cause(), "full-resolution fetches stopped");
            ps.images.lock_or_panic().pool.clear();
            return;
        }

        self.full_fetch_failed(ps, id, AttemptFailure::Outcome(&response.outcome));
    }

    /// A failed attempt of `id`: it is re-queued or given up.
    fn full_fetch_failed(&self, ps: &PhoneState, id: i64, failure: AttemptFailure<'_>) {
        let failed = {
            let mut st = ps.images.lock_or_panic();
            st.consecutive_failures = st.consecutive_failures.saturating_add(1);
            st.pool.fail(id, self.config.allowed_failures)
        };

        if failed == Failed::GaveUp {
            warn!(phone = %ps.phone, id, error = %failure, "full-resolution fetch failed; given up");
            self.settled(ps, wire::content_type::PHOTOS, &[id]);
        } else {
            debug!(phone = %ps.phone, id, error = %failure, "full-resolution fetch failed; queued again");
        }
    }

    fn full_received(self: &Arc<Self>, ps: &Arc<PhoneState>, id: i64, images: Vec<wire::FullImage>) {
        let (to_store, gave_up) = {
            let mut st = ps.images.lock_or_panic();
            st.consecutive_failures = 0;
            if images.is_empty() {
                // Not found.
                st.pool.remove(id);
                (Vec::new(), Vec::new())
            } else {
                let mut to_store = Vec::new();
                let mut gave_up = Vec::new();
                let allowed = self.config.allowed_failures;

                for image in images {
                    if image.bytes.is_empty() {
                        if st.pool.fail(image.id, allowed) == Failed::GaveUp {
                            gave_up.push(image.id);
                        }
                    } else {
                        st.pool.remove(image.id);
                        to_store.push((image.id, image.bytes));
                    }
                }
                (to_store, gave_up)
            }
        };

        self.settled(ps, wire::content_type::PHOTOS, &gave_up);
        self.store_full(ps, &to_store);
    }

    /// Stores full-resolution bytes for cached items and wakes the waiters. A fetch reply is not the
    /// phone's data: an item a delete took out of the gallery stays out, also when a request sent
    /// before the delete (or a prefetch) is answered after it.
    fn store_full(&self, ps: &PhoneState, images: &[(i64, Vec<u8>)]) {
        if images.is_empty() {
            return;
        }

        let stored = match store::set_full_bytes(&ps.db, images) {
            Ok(s) => s,
            Err(e) => {
                error!(phone = %ps.phone, error = %e, "full-resolution bytes not stored");
                return;
            }
        };

        self.settled(ps, wire::content_type::PHOTOS, &stored);
        ps.full_stored.notify_waiters();
        for id in &stored {
            self.emit(PhotosEvent::FullImageStored { phone: ps.phone.clone(), id: *id });
        }
        if !stored.is_empty() {
            self.emit(PhotosEvent::GalleryChanged { phone: ps.phone.clone() });
        }
    }

    /// The gallery's "pending" marks of `ids` end (data received, already present, or given up).
    fn settled(&self, ps: &PhoneState, content_type: &str, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let mut ui = ps.ui.lock_or_panic();
        let set =
            if content_type == wire::content_type::PHOTOS { &mut ui.pending_full } else { &mut ui.pending_thumbnails };
        for id in ids {
            set.remove(id);
        }
    }
}

#[cfg(test)]
mod tests;
