//! Deleting photos: the prompt, the request and its reply.

use super::{
    DeletePrompt, DeleteResult, DeleteState, PhoneState, PhotoRow, Photos, PhotosError, PhotosEvent, PhotosStatus,
    Setting, UiState, capability, store, trace_name, wire,
};
use phonelink_app::AppOutcome;
use phonelink_app::consts::permission;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::TraceContext;
use std::collections::HashSet;
use tracing::{error, info};

impl Photos {
    /// Whether the delete action of `id` is available (the "Delete offered" feature gate): offered,
    /// enabled for the phone, and the item cached, displayed (an item a delete took out of the
    /// gallery has no delete action), showing an image (thumbnail or full bytes) and not being
    /// deleted already.
    pub(super) fn delete_action(
        &self,
        status: &PhotosStatus,
        ps: &PhoneState,
        id: i64,
        ui: &UiState,
    ) -> Result<PhotoRow, PhotosError> {
        if !status.delete_offered {
            return Err(PhotosError::DeleteNotOffered);
        }
        if !status.delete_enabled {
            return Err(PhotosError::DeleteNotEnabled);
        }

        let row = self.row(ps, id)?;
        let deleting = ui.deleting.get(&id) == Some(&DeleteState::Deleting);
        if !(row.has_thumbnail || row.has_full) || deleting || ui.hidden.contains(&id) {
            return Err(PhotosError::ItemDeleteNotEnabled(id));
        }
        Ok(row)
    }

    /// The delete pre-check of `id`, the first step of its delete action (refused like
    /// [`Photos::delete`] when the action is not available): when the phone sets
    /// `PermissionRequiredMediaDelete` and `PhotosDelete` is not held, a silent `/permission` type
    /// `0x200000` is sent first; Success/Accepted grant it and lead to the confirmation, anything else
    /// to the "missing permission" prompt.
    pub async fn prepare_delete(&self, id: i64) -> Result<DeletePrompt, PhotosError> {
        let status = self.status()?;
        let (_, ps) = self.current()?;
        self.delete_action(&status, &ps, id, &ps.ui.lock_or_panic())?;

        if status.capabilities & capability::PERMISSION_REQUIRED_MEDIA_DELETE == 0 || status.photos_delete_permission {
            return Ok(DeletePrompt::Confirmation);
        }

        let (outcome, _) = self
            .permission_request(
                permission::PHOTOS_DELETE,
                true,
                trace_name::PERMISSION_REQUIRED,
                trace_name::PHOTOS_DELETE_PERMISSION,
            )
            .await?;

        if matches!(outcome, AppOutcome::Success | AppOutcome::Accepted) {
            self.inner.set_permission(permission::PHOTOS_DELETE, true);
            self.inner.emit(PhotosEvent::StatusChanged { phone: status.phone });
            Ok(DeletePrompt::Confirmation)
        } else {
            Ok(DeletePrompt::MissingPermission)
        }
    }

    /// Deletes one item on the phone (after the user confirmed): `requestType 0` with its cached
    /// checksum, the delete timeout — refused, sending nothing, when the delete action of
    /// the item is not available (not offered, not enabled, not displayed, no image shown, or its
    /// delete already in flight). `NeedsPermission` clears `PhotosDelete`; a Success reply with both id
    /// arrays removes the deleted ids from the displayed gallery after 250 ms and asks for the "delete
    /// failed" notice when an id failed or none succeeded; every other requested id reverts. The
    /// cache entry stays until the phone's data omits or removes it.
    ///
    /// Once the item is marked "deleting" nothing returns early: the reply's handling always ends the
    /// mark and announces [`PhotosEvent::DeleteFinished`], also when the store cannot be read then.
    pub async fn delete(&self, id: i64) -> Result<DeleteResult, PhotosError> {
        let status = self.status()?;
        let (phone, ps) = self.current()?;
        let row = {
            // Checked and marked in one step, so a second delete of the item is refused.
            let mut ui = ps.ui.lock_or_panic();
            let row = self.delete_action(&status, &ps, id, &ui)?;
            ui.deleting.insert(id, DeleteState::Deleting);
            row
        };
        self.inner.emit(PhotosEvent::GalleryChanged { phone: phone.clone() });

        let request = wire::delete_request(
            &[id],
            &[row.checksum],
            TraceContext::new_root(Vec::new()),
            self.inner.config.media_delete_timeout,
        );
        let response = self.inner.send(&phone, request).await;
        let outcome = response.as_ref().map_or(AppOutcome::Canceled, |r| r.outcome.clone());

        if outcome == AppOutcome::NeedsPermission {
            self.inner.set_permission(permission::PHOTOS_DELETE, false);
        }

        let (mut deleted, mut failed_notice) = (Vec::new(), false);
        if outcome.is_success() {
            if let Some((ok, failed)) = response.as_ref().and_then(|r| r.values()).and_then(wire::parse_delete_reply) {
                failed_notice = !failed.is_empty() || ok.is_empty();
                deleted = ok;
            }
        }

        // The requested ids the phone did not report deleted revert; one it reported deleted does
        // not, even when the phone's data removed it from the cache meanwhile.
        let reverted: Vec<i64> = [id].into_iter().filter(|r| !deleted.contains(r)).collect();

        // Only the deleted ids still displayed have an item to take out of the gallery; a store that
        // cannot be read displays none (the item stays, no longer "deleting").
        let displayed: HashSet<i64> = match self.displayed(&ps) {
            Ok(items) => items.iter().map(|i| i.id).collect(),
            Err(e) => {
                error!(%phone, id, error = %e, "photos store unreadable after a delete reply; nothing leaves the gallery");
                HashSet::new()
            }
        };

        let shown: Vec<i64> = deleted.iter().copied().filter(|d| displayed.contains(d)).collect();
        {
            let mut ui = ps.ui.lock_or_panic();
            for d in &shown {
                ui.deleting.insert(*d, DeleteState::Deleted);
            }
            // The requested item's delete is no longer in flight: unless it is now "deleted", its mark
            // goes — reverted, or reported deleted but no longer displayed.
            if !shown.contains(&id) {
                ui.deleting.remove(&id);
            }
        }

        if !shown.is_empty() {
            let (inner, ps2, ids, delay) =
                (self.inner.clone(), ps.clone(), shown, self.inner.config.delete_removal_delay);
            self.inner.runtime.spawn(async move {
                tokio::time::sleep(delay).await;

                {
                    let mut ui = ps2.ui.lock_or_panic();
                    for d in &ids {
                        if ui.deleting.get(d) == Some(&DeleteState::Deleted) {
                            ui.deleting.remove(d);
                            ui.hidden.insert(*d);
                        }
                    }
                }

                inner.emit(PhotosEvent::GalleryChanged { phone: ps2.phone.clone() });
            });
        }

        // Another end is the app's to log: it gets the result.
        if outcome.is_success() && !failed_notice {
            info!(%phone, id, deleted_count = deleted.len(), "photo deleted");
        }

        let result = DeleteResult { outcome, deleted, reverted, failed_notice };
        self.inner.emit(PhotosEvent::GalleryChanged { phone: phone.clone() });
        self.inner.emit(PhotosEvent::DeleteFinished { phone, result: result.clone() });
        Ok(result)
    }

    // --- PC-side settings ---------------------------------------------------------------------

    pub(super) fn set(&self, setting: Setting, value: bool) -> Result<(), PhotosError> {
        let (phone, ps) = self.current()?;
        store::set_setting(&ps.db, setting, value)?;
        info!(%phone, setting_kind = ?setting, enabled = value, "photos setting changed");
        self.inner.emit(PhotosEvent::StatusChanged { phone });
        Ok(())
    }

    /// The PC-side photos consent (default on). Off shows the consent prompt and suppresses the
    /// new-photo notification; no wire effect.
    pub fn set_consent(&self, on: bool) -> Result<(), PhotosError> {
        self.set(Setting::Consent, on)
    }

    /// "Deletion allowed" (default on).
    pub fn set_deletion_allowed(&self, on: bool) -> Result<(), PhotosError> {
        self.set(Setting::DeletionAllowed, on)
    }

    /// "New-photo notifications" (default on).
    pub fn set_new_photo_notifications(&self, on: bool) -> Result<(), PhotosError> {
        self.set(Setting::NewPhotoNotifications, on)
    }

    /// Runs the cache maintenance now (it also runs at start and every 24 h).
    pub fn run_cache_maintenance(&self) {
        self.inner.run_maintenance();
    }
}
