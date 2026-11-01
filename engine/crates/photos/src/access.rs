//! Photo access: the permission requests and their follow-ups.

use super::{FileAction, PhoneState, Photos, PhotosError, PhotosEvent, Setting, trace_name};
use phonelink_app::consts::permission;
use phonelink_app::{AppOutcome, RequestOptions};
use phonelink_core::types::TraceContext;
use std::sync::Arc;
use tracing::info;

impl Photos {
    /// The bytes for a file action (copy, save-as, share, open, open-with or drag-out): cached, or
    /// fetched (top priority, named after the action) and waited for at most 30 s; `None` when they
    /// did not arrive.
    pub async fn full_image_for_action(&self, id: i64, action: FileAction) -> Result<Option<Vec<u8>>, PhotosError> {
        let (_, ps) = self.current()?;
        if !self.row(&ps, id)?.has_full {
            self.mark_and_fetch_full(&ps, &[id], action.trigger());
        }
        Ok(self.inner.wait_for_full(&ps, id, self.inner.config.full_bytes_wait).await)
    }

    // --- permissions ------------------------------------------------------------------------

    pub(super) async fn permission_request(
        &self,
        permission_type: u32,
        silent: bool,
        trigger: &'static str,
        scenario: &'static str,
    ) -> Result<(AppOutcome, TraceContext), PhotosError> {
        let app = self.inner.app.upgrade().ok_or(PhotosError::NoPhone)?;
        let request =
            app.permission_request(permission_type, silent, RequestOptions::user_initiated().named(trigger, scenario));
        let trace = request.trace.clone().unwrap_or_else(|| TraceContext::new_root(Vec::new()));
        let response = app.request(request).await?;

        // Another outcome is the app's to log: it gets it, or the prompt it leads to.
        if matches!(response.outcome, AppOutcome::Success | AppOutcome::Accepted) {
            info!(permission_type, silent, outcome = ?response.outcome, "photos permission request accepted");
        }
        Ok((response.outcome, trace))
    }

    /// After a photo-access or full-access reply: the media sync request when `Photos` is held, a
    /// child of the permission request's trace that keeps its `trigger`.
    pub(super) async fn access_follow_up(
        &self,
        ps: &Arc<PhoneState>,
        outcome: &AppOutcome,
        trace: &TraceContext,
        trigger: &'static str,
    ) {
        self.inner.apply_access_reply(outcome);
        let held = self.inner.record().is_none_or(|r| r.permission_or(permission::PHOTOS, true));
        self.inner.emit(PhotosEvent::StatusChanged { phone: ps.phone.clone() });

        if held {
            let (inner, ps, child) = (self.inner.clone(), ps.clone(), trace.child());
            self.inner.runtime.spawn(async move {
                inner.media_sync(&ps, child, trigger).await;
            });
        }
    }

    /// "Allow photo access on the phone": `/permission` type `1`, not silent; then the media sync
    /// request when `Photos` is held.
    pub async fn request_photo_access(&self) -> Result<AppOutcome, PhotosError> {
        let (_, ps) = self.current()?;
        let trigger = trace_name::ERROR_ACTION_INVOKED;
        let (outcome, trace) =
            self.permission_request(permission::PHOTOS, false, trigger, trace_name::PHOTOS_DEVICE_PERMISSION).await?;
        self.access_follow_up(&ps, &outcome, &trace, trigger).await;
        Ok(outcome)
    }

    /// The full-access reminder's action: `/permission` type `0x20000000`, not silent; then the media
    /// sync request when `Photos` is held.
    pub async fn request_full_access(&self) -> Result<AppOutcome, PhotosError> {
        let (_, ps) = self.current()?;
        let trigger = trace_name::FULL_PERMISSION_REQUEST_INVOKED;
        let (outcome, trace) = self
            .permission_request(permission::PHOTOS_FULL, false, trigger, trace_name::PHOTOS_DEVICE_FULL_PERMISSION)
            .await?;
        self.access_follow_up(&ps, &outcome, &trace, trigger).await;
        Ok(outcome)
    }

    /// The user dismissed the full-access reminder (re-armed by every connect reply).
    pub fn dismiss_full_access_reminder(&self) -> Result<(), PhotosError> {
        self.set(Setting::FullAccessReminderDismissed, true)
    }

    /// The "missing permission" prompt's action: `/permission` type `0x200000`, not silent;
    /// Success/Accepted grant `PhotosDelete`. The delete is not resumed.
    pub async fn request_delete_permission(&self) -> Result<AppOutcome, PhotosError> {
        let (phone, _) = self.current()?;

        let (outcome, _) = self
            .permission_request(
                permission::PHOTOS_DELETE,
                false,
                trace_name::PERMISSION_REQUIRED,
                trace_name::PHOTOS_DELETE_PERMISSION,
            )
            .await?;

        if matches!(outcome, AppOutcome::Success | AppOutcome::Accepted) {
            self.inner.set_permission(permission::PHOTOS_DELETE, true);
        }
        self.inner.emit(PhotosEvent::StatusChanged { phone });
        Ok(outcome)
    }

    // --- delete -------------------------------------------------------------------------------
}
