//! The feature's shared state: each phone's state, the sync bookkeeping and the status it shows.

use super::{
    ImageState, PhoneState, PhotosEvent, PhotosState, PhotosStatus, RecordView, ThumbState, UiState, capability,
    is_enabled, store, wire,
};
use phonelink_app::consts::{permission, platform};
use phonelink_app::{AppEvent, AppLayer, AppOutcome, AppRequest, AppResponse, PhoneRecord};
use phonelink_core::config::features::PhotosConfig;
use phonelink_core::edge::Clock;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::NewPhotoGate;
use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::shared::notifier::SystemNotifier;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

pub(crate) struct PhotosInner {
    pub(crate) config: PhotosConfig,
    pub(crate) app: Weak<AppLayer>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) notifier: Arc<dyn SystemNotifier>,
    pub(super) stores: PhoneStores,
    pub(super) phones: Mutex<HashMap<DcgClient, Arc<PhoneState>>>,
    pub(super) last_view: Mutex<Option<RecordView>>,
    pub(super) events: broadcast::Sender<PhotosEvent>,
    pub(crate) runtime: Handle,
    pub(super) shutdown: CancellationToken,
    /// Set once after install by the feature whose new-photo notifications take precedence.
    pub(crate) new_photo_gate: OnceLock<Arc<dyn NewPhotoGate>>,
}

impl Drop for PhotosInner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl PhotosInner {
    pub(crate) fn emit(&self, event: PhotosEvent) {
        // No receiver is not an error.
        let _ = self.events.send(event);
    }

    pub(crate) fn now_ms(&self) -> i64 {
        phonelink_core::time::unix_ms(self.clock.now())
    }

    /// `phone`'s state, its store opened (and migrated) on first use.
    pub(crate) fn phone_state(self: &Arc<Self>, phone: &DcgClient) -> Result<Arc<PhoneState>, StoreError> {
        let (ps, created) = {
            let mut phones = self.phones.lock_or_panic();
            match phones.get(phone) {
                Some(ps) => (ps.clone(), false),
                None => {
                    let ps = Arc::new(PhoneState {
                        phone: phone.clone(),
                        db: self.stores.get(phone)?,
                        thumbs: Mutex::new(ThumbState::default()),
                        images: Mutex::new(ImageState::default()),
                        ui: Mutex::new(UiState::default()),
                        full_stored: Notify::new(),
                    });
                    phones.insert(phone.clone(), ps.clone());
                    (ps, true)
                }
            }
        };

        if created {
            self.refresh_sync_state(&ps);
        }
        Ok(ps)
    }

    /// Sends one photos request to `phone`; `None` when the App layer is gone, `phone` is no longer
    /// the selected phone, or the request could not be built (logged).
    pub(crate) async fn send(&self, phone: &DcgClient, request: AppRequest) -> Option<AppResponse> {
        let app = self.app.upgrade()?;
        if app.selected_phone().as_ref() != Some(phone) {
            return None;
        }

        match app.request(request).await {
            Ok(r) => Some(r),
            Err(e) => {
                error!(%phone, error = %e, "photos request not built");
                None
            }
        }
    }

    pub(super) fn set_permission(&self, bit: u32, granted: bool) {
        if let Some(app) = self.app.upgrade() {
            app.update_phone_record(|r| r.set_permission(bit, granted));
        }
    }

    pub(super) fn record(&self) -> Option<PhoneRecord> {
        self.app.upgrade().and_then(|a| a.phone_record())
    }

    /// Cache entries an item message from the phone wrote (inserted, overwritten, or their full
    /// bytes dropped): an item a delete took out of the displayed gallery returns to it. Only the
    /// phone's data does this.
    pub(crate) fn rows_written(&self, ps: &PhoneState, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let mut ui = ps.ui.lock_or_panic();
        for id in ids {
            ui.hidden.remove(id);
        }
    }

    /// Rows that left the cache: their UI state goes with them.
    pub(crate) fn rows_deleted(&self, ps: &PhoneState, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }

        let mut ui = ps.ui.lock_or_panic();
        for id in ids {
            ui.hidden.remove(id);
            ui.deleting.remove(id);
            ui.pending_thumbnails.remove(id);
            ui.pending_full.remove(id);
        }
    }

    /// A sync completed (`current` and the sync time are stored).
    pub(crate) fn sync_completed(self: &Arc<Self>, ps: &Arc<PhoneState>) {
        info!(phone = %ps.phone, "mediaInfo sync completed");
        self.refresh_sync_state(ps);
        self.emit(PhotosEvent::StatusChanged { phone: ps.phone.clone() });
    }

    /// Re-reads the sequence pair; when the up-to-date state changed (or is read the first time) the
    /// indicator shows while not up to date, for at most the configured 2 minutes. Becoming up to
    /// date records the sync time.
    ///
    /// The pair is read while the indicator is held: each refresh decides on the pair as it is when
    /// its turn comes, so one racing another writer (the connect reply storing `expected`, a
    /// completion storing `current`) cannot apply a pair it read before the other's write.
    pub(crate) fn refresh_sync_state(self: &Arc<Self>, ps: &Arc<PhoneState>) {
        let (generation, up, became_up) = {
            let mut ui = ps.ui.lock_or_panic();
            let up = match store::state(&ps.db) {
                Ok(s) => s.up_to_date(),
                Err(e) => {
                    error!(phone = %ps.phone, error = %e, "photos store unreadable; syncing state unchanged");
                    return;
                }
            };

            let ind = &mut ui.indicator;
            if ind.up_to_date == Some(up) {
                return;
            }

            let first = ind.up_to_date.is_none();
            ind.up_to_date = Some(up);
            ind.generation += 1;
            ind.updating = !up;
            (ind.generation, up, up && !first)
        };

        if became_up {
            if let Err(e) = store::record_sync_time(&ps.db, self.now_ms()) {
                error!(phone = %ps.phone, error = %e, "sync time not recorded");
            }
        }

        if !up {
            let (weak, ps2, max) = (Arc::downgrade(self), ps.clone(), self.config.syncing_indicator_max);
            self.runtime.spawn(async move {
                tokio::time::sleep(max).await;
                let Some(inner) = weak.upgrade() else { return };

                let ended = {
                    let mut ui = ps2.ui.lock_or_panic();
                    let ind = &mut ui.indicator;
                    let ends = ind.generation == generation && ind.updating;
                    if ends {
                        ind.updating = false;
                    }
                    ends
                };

                if ended {
                    inner.emit(PhotosEvent::StatusChanged { phone: ps2.phone.clone() });
                }
            });
        }

        self.emit(PhotosEvent::StatusChanged { phone: ps.phone.clone() });
    }

    /// Waits until `id`'s full bytes are stored, at most `timeout`.
    pub(crate) async fn wait_for_full(&self, ps: &PhoneState, id: i64, timeout: Duration) -> Option<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            let notified = ps.full_stored.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            match store::full_bytes(&ps.db, id) {
                Ok(Some(bytes)) => return Some(bytes),
                Ok(None) => {}
                Err(e) => {
                    error!(phone = %ps.phone, error = %e, "photos store unreadable while waiting for full bytes");
                    return None;
                }
            }

            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    /// The media sync request: not sent while the "syncing" indicator shows; the indicator shows
    /// while it is in flight. Success or Accepted → `Photos` permission `true`; Denied,
    /// `PermanentlyDenied` or `NeedsPermission` → `false`. `trigger` is the permission request's
    /// trigger name.
    pub(crate) async fn media_sync(
        self: &Arc<Self>,
        ps: &Arc<PhoneState>,
        trace: TraceContext,
        trigger: &'static str,
    ) -> Option<AppOutcome> {
        {
            let mut ui = ps.ui.lock_or_panic();
            if ui.indicator.updating {
                debug!(phone = %ps.phone, "media sync request not sent; a sync is running");
                return None;
            }
            ui.indicator.updating = true;
        }
        self.emit(PhotosEvent::StatusChanged { phone: ps.phone.clone() });

        let response = self.send(&ps.phone, wire::sync_request(trace, trigger)).await;
        let outcome = response.map(|r| r.outcome);
        log_media_sync(&ps.phone, outcome.as_ref());

        match outcome {
            Some(AppOutcome::Success | AppOutcome::Accepted) => self.set_permission(permission::PHOTOS, true),
            Some(AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission) => {
                self.set_permission(permission::PHOTOS, false)
            }
            _ => {}
        }

        ps.ui.lock_or_panic().indicator.updating = false;
        self.emit(PhotosEvent::StatusChanged { phone: ps.phone.clone() });
        outcome
    }

    /// The mapping of the photo-access and full-access requests: Success/Accepted → `Photos` and
    /// `PhotosFull` `true`; `NeedsPermission` → `Photos` `true`, `PhotosFull` `false`;
    /// Denied/PermanentlyDenied → both `false`.
    pub(super) fn apply_access_reply(&self, outcome: &AppOutcome) {
        let (photos, full) = match outcome {
            AppOutcome::Success | AppOutcome::Accepted => (true, true),
            AppOutcome::NeedsPermission => (true, false),
            AppOutcome::Denied | AppOutcome::PermanentlyDenied => (false, false),
            _ => return,
        };

        self.set_permission(permission::PHOTOS, photos);
        self.set_permission(permission::PHOTOS_FULL, full);
    }

    /// The cache maintenance of every open phone store and the selected phone's.
    pub(crate) fn run_maintenance(self: &Arc<Self>) {
        let selected = self.app.upgrade().and_then(|a| a.selected_phone());
        if let Some(phone) = &selected {
            if let Err(e) = self.phone_state(phone) {
                error!(%phone, error = %e, "photos store unavailable; no cache maintenance");
            }
        }

        let phones: Vec<Arc<PhoneState>> = self.phones.lock_or_panic().values().cloned().collect();
        let now = self.now_ms();
        let older = i64::try_from(self.config.dehydrate_after.as_millis()).unwrap_or(i64::MAX);
        for ps in phones {
            self.maintain(&ps, now, older);
        }
    }

    /// The cache maintenance of one phone store: drops full-resolution bytes older than `older` ms.
    fn maintain(&self, ps: &PhoneState, now: i64, older: i64) {
        match store::maintain(&ps.db, now, self.config.dehydrate_keep_recent, older) {
            Ok(ids) if ids.is_empty() => {}
            Ok(ids) => {
                info!(phone = %ps.phone, count = ids.len(), "cache maintenance dropped full-resolution bytes");
                self.emit(PhotosEvent::GalleryChanged { phone: ps.phone.clone() });
            }
            Err(e) => error!(phone = %ps.phone, error = %e, "cache maintenance failed"),
        }
    }

    pub(super) fn spawn_maintenance(self: &Arc<Self>) {
        let (weak, shutdown, period) = (Arc::downgrade(self), self.shutdown.clone(), self.config.maintenance_interval);
        self.runtime.spawn(async move {
            loop {
                match weak.upgrade() {
                    Some(inner) => inner.run_maintenance(),
                    None => break,
                }

                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(period) => {}
                }
            }
        });
    }

    pub(super) fn spawn_app_listener(self: &Arc<Self>, mut rx: broadcast::Receiver<AppEvent>) {
        let (weak, shutdown) = (Arc::downgrade(self), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    e = rx.recv() => e,
                };

                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(AppEvent::PermissionsChanged { .. } | AppEvent::PhoneRecordChanged { .. }) => {
                        inner.on_record_changed()
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "App events missed; the status is read again");
                        inner.on_record_changed();
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Announces a change of the record-derived status; a newly selected phone cancels the other
    /// phones' fetches.
    pub(super) fn on_record_changed(self: &Arc<Self>) {
        let Some(record) = self.record() else { return };
        let view = RecordView::of(&record);
        let previous = self.last_view.lock_or_panic().replace(view.clone());

        if previous.as_ref() == Some(&view) {
            return;
        }

        if previous.as_ref().is_some_and(|p| p.phone != view.phone) {
            let others: Vec<Arc<PhoneState>> =
                self.phones.lock_or_panic().iter().filter(|(p, _)| **p != view.phone).map(|(_, s)| s.clone()).collect();
            for ps in others {
                self.cancel_all_thumbnails(&ps);
                self.cancel_all_full(&ps);
            }
        }

        self.emit(PhotosEvent::StatusChanged { phone: view.phone });
    }

    pub(super) fn status_of(
        &self,
        phone: &DcgClient,
        record: Option<&PhoneRecord>,
        st: &PhotosState,
        syncing: bool,
    ) -> PhotosStatus {
        let record = record.filter(|r| r.phone() == *phone);
        let perm = |bit: u32, default: bool| record.map_or(default, |r| r.permission_or(bit, default));
        let android = record.is_some_and(|r| r.platform == platform::ANDROID);

        let photos = perm(permission::PHOTOS, true);
        let photos_full = perm(permission::PHOTOS_FULL, false);
        let photos_delete = perm(permission::PHOTOS_DELETE, false);
        let post_notifications = perm(permission::POST_NOTIFICATIONS, true);
        let auto_launch = perm(permission::AUTO_LAUNCH, false);

        let delete_supported = android && st.capabilities & capability::MEDIA_DELETE != 0;
        // Android with an OS major version ≥ 13 (`PhoneRecord::android_major_version`).
        let ask_supported = record
            .and_then(PhoneRecord::android_major_version)
            .is_some_and(|v| v >= self.config.post_notification_ask_min_os);
        let delete_enabled = delete_supported && (!ask_supported || photos_delete || post_notifications || auto_launch);
        let full_access_reminder = android
            && record.is_some_and(|r| r.android_sdk_version >= self.config.full_access_reminder_min_sdk)
            && !st.full_access_reminder_dismissed
            && post_notifications
            && !photos_delete
            && photos
            && !photos_full;

        PhotosStatus {
            phone: phone.clone(),
            enabled: record.is_some_and(is_enabled),
            consent: st.consent,
            photos_permission: photos,
            photos_full_permission: photos_full,
            photos_delete_permission: photos_delete,
            capabilities: st.capabilities,
            deletion_allowed: st.deletion_allowed,
            delete_offered: delete_supported && st.deletion_allowed,
            delete_enabled,
            new_photo_notifications: st.new_photo_notifications,
            full_access_reminder,
            syncing,
            up_to_date: st.up_to_date(),
            current_sequence: st.current_sequence,
            expected_sequence: st.expected_sequence,
            last_sync_ms: st.last_sync_ms,
            snapshot_size: self.config.media_info_snapshot_size,
            grouping: self.config.media_grouping,
        }
    }
}

/// The media sync request's answer (`None`: not sent; [`PhotosInner::send`] logged why when it is
/// a failure).
fn log_media_sync(phone: &DcgClient, outcome: Option<&AppOutcome>) {
    match outcome {
        Some(o @ (AppOutcome::Success | AppOutcome::Accepted)) => {
            info!(%phone, outcome = ?o, "media sync request answered");
        }
        Some(o) => warn!(%phone, outcome = ?o, error = o.cause(), "media sync request failed"),
        None => debug!(%phone, "media sync request not sent"),
    }
}
