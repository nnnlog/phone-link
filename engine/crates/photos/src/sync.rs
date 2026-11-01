//! The inbound `mediaInfo` processor: the metadata and item messages of the media-list sync (phone
//! → PC), and malformed inbound data and its replies.
//!
//! A metadata message opens a sync: the cache is reconciled against the phone's list in one
//! transaction and the reply names the ids the PC needs (`outgoing`) and those that should carry an
//! inline thumbnail (`requested`). The processor then stays in the App layer's unfinished-processor
//! table under the sync's key while ids are pending; each item message removes the ids it carries,
//! and the sync completes when none is pending. An item message with no open sync is processed on
//! its own with an empty pending set. A malformed metadata message is answered `{ result: 1 }`; a
//! malformed item message `{ result: 1, message }`, which also closes its sync without completing it.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, error, info, warn};

use phonelink_app::consts::{key as app_key, permission};
use phonelink_app::{
    ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, rejection_message, simple_answer,
};
use phonelink_core::codec::valueset::ValueSet;

use super::store::{self, Completion};
use super::wire;
use super::{PhotosEvent, PhotosInner};

/// The factory of `mediaInfo` processors.
pub(crate) struct MediaInfoFactory {
    pub(crate) inner: Arc<PhotosInner>,
}

/// One sync (or one item message processed on its own).
pub(crate) struct MediaInfoProcessor {
    inner: Arc<PhotosInner>,
    /// The ids the PC asked for that have not arrived as non-removed items.
    pending: HashSet<i64>,
    /// Becomes `current` when the sync completes.
    sequence_number: Option<i64>,
    /// A pending id reported removed was logged for this sync.
    removed_pending_logged: bool,
}

/// A rejected item message: `{ result: Int32 1, message: "<reason> 80004005" }`.
fn item_rejection(reason: &'static str) -> Rejection {
    Rejection::with_answer(reason, wire::item_rejection_answer(rejection_message(reason)))
}

impl ContentProcessorFactory for MediaInfoFactory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        if payload.contains_key(app_key::METADATA) {
            // The metadata phase reads and applies the message.
            return Ok(Some(Box::new(MediaInfoProcessor {
                inner: self.inner.clone(),
                pending: HashSet::new(),
                sequence_number: None,
                removed_pending_logged: false,
            })));
        }

        // An item message with no open sync: it is processed on its own, which reads its
        // `correlationVector` (required) and `sequenceNumber`.
        if ctx.correlation_vector(payload).is_none() {
            return Err(item_rejection("item message without a correlationVector"));
        }

        let sequence_number = payload.i64(wire::key::SEQUENCE_NUMBER).filter(|s| *s != -1);
        Ok(Some(Box::new(MediaInfoProcessor {
            inner: self.inner.clone(),
            pending: HashSet::new(),
            sequence_number,
            removed_pending_logged: false,
        })))
    }
}

#[async_trait]
impl ContentProcessor for MediaInfoProcessor {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let m = match wire::parse_metadata(payload) {
            Ok(m) => m,
            Err(m) => return Processed::Rejected(m.rejection()),
        };

        if m.repeated > 0 {
            debug!(phone = %ctx.phone, repeated_count = m.repeated, "mediaInfo metadata: repeated ids ignored");
        }

        let inner = &self.inner;
        let ps = match inner.phone_state(ctx.phone) {
            Ok(ps) => ps,
            Err(e) => {
                return Processed::Rejected(Rejection::new("photos store unavailable").because(e));
            }
        };

        let now = inner.now_ms();
        let reconciled = match store::reconcile(&ps.db, &m, m.settings.map(|s| s.capabilities), now) {
            Ok(r) => r,
            Err(e) => {
                return Processed::Rejected(Rejection::new("photos store failure").because(e));
            }
        };

        if let Some(granted) = m.settings.and_then(|s| s.has_delete_permission) {
            ctx.app.update_phone_record(|r| r.set_permission(permission::PHOTOS_DELETE, granted));
        }

        let n = if inner.config.media_optimized_thumbnails {
            0
        } else {
            reconciled.outgoing.len().min(usize::try_from(inner.config.auto_hydrated_thumbnails_count).unwrap_or(0))
        };
        let requested = &reconciled.outgoing[..n];
        self.pending = reconciled.outgoing.iter().copied().collect();
        self.sequence_number = m.sequence_number;

        inner.rows_deleted(&ps, &reconciled.deleted);
        if !reconciled.deleted.is_empty() {
            inner.emit(PhotosEvent::GalleryChanged { phone: ctx.phone.clone() });
        }
        if reconciled.capabilities_changed || m.settings.is_some_and(|s| s.has_delete_permission.is_some()) {
            inner.emit(PhotosEvent::StatusChanged { phone: ctx.phone.clone() });
        }

        let answer = wire::metadata_reply(
            &m.correlation_vector,
            ctx.app.standard_keys().contract_version,
            &reconciled.outgoing,
            requested,
        );

        if reconciled.outgoing.is_empty() {
            inner.sync_completed(&ps);
            Processed::completed(answer)
        } else {
            info!(
                phone = %ctx.phone,
                outgoing_count = reconciled.outgoing.len(),
                requested_count = n,
                "mediaInfo sync opened"
            );
            Processed::open(answer)
        }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let batch = match wire::parse_items(payload) {
            Ok(b) => b,
            Err(m) => return Processed::Rejected(item_rejection(m.reason)),
        };

        let inner = &self.inner;
        let ps = match inner.phone_state(ctx.phone) {
            Ok(ps) => ps,
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "photos store unavailable; items refused");
                return Processed::Rejected(item_rejection("photos store unavailable"));
            }
        };

        // Removed ids do not leave the pending set.
        let mut pending = self.pending.clone();
        for item in &batch.items {
            pending.remove(&item.id);
        }

        let completed = pending.is_empty();
        let now = inner.now_ms();
        let completion = completed.then_some(Completion { now_ms: now, sequence_number: self.sequence_number });

        let written = match store::apply_items(&ps.db, &batch.items, &batch.removed, now, completion) {
            Ok(written) => written,
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "photos store failure; items refused");
                return Processed::Rejected(item_rejection("photos store failure"));
            }
        };

        // Each item message of the sync may say it again: logged once per sync.
        if !self.removed_pending_logged && batch.removed.iter().any(|id| pending.contains(id)) {
            self.removed_pending_logged = true;
            warn!(phone = %ctx.phone, "mediaInfo items: a pending id reported removed; it stays pending");
        }
        self.pending = pending;

        // The entries the phone's data wrote return to the displayed gallery after a delete.
        inner.rows_written(&ps, &written);
        inner.rows_deleted(&ps, &batch.removed);
        if !batch.items.is_empty() || !batch.removed.is_empty() {
            inner.emit(PhotosEvent::GalleryChanged { phone: ctx.phone.clone() });
        }

        inner.notify_new_photos(&ps, &batch.items);
        if completed {
            inner.sync_completed(&ps);
        }
        Processed::Answer { answer: simple_answer(0), completed }
    }
}
