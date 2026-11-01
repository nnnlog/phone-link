//! The photos contribution to the connect message.
//!
//! Request: `configuration` › `MediaInfoSync` (the phone is Android), `PhotosLegacySync` `false`,
//! `MediaInfoSnapshotSize` `2000`, `AutoHydratedThumbnailsCount` (`25`); `sequenceNumbers` ›
//! `media_info` (the stored `current`, `-1` when none) only with the phone's `Photos` capability bit —
//! and left out when the store cannot be read, rather than sent as an invented value.
//! Response, read from the top level: `media_info` → `expected` (only with the capability bit and
//! `result == 0`; absent or mistyped → `-1`), `permissions.photos` / `photos_full` /
//! `photos_delete`, `capabilities.photos` (absent → kept), `mediaSettings.dcim_directory`
//! (ignored); every reply re-arms the full-access reminder. A key that is absent or has another type
//! is skipped.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, error};

use phonelink_app::consts::{capability, key as app_key, permission, platform};
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::store::{self, Setting};
use super::wire::key;
use super::{PhotosEvent, PhotosInner};

/// The provider registered for [`phonelink_app::ConnectProvider::Photos`].
pub(crate) struct PhotosContributor {
    pub(crate) inner: Arc<PhotosInner>,
}

#[async_trait]
impl ConnectContributor for PhotosContributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let config = &self.inner.config;
        let mut c = ConnectContribution::default();
        c.configuration.insert(key::MEDIA_INFO_SYNC, ctx.record.platform == platform::ANDROID);
        c.configuration.insert(key::PHOTOS_LEGACY_SYNC, false);
        c.configuration.insert(key::MEDIA_INFO_SNAPSHOT_SIZE, config.media_info_snapshot_size);
        c.configuration.insert(key::AUTO_HYDRATED_THUMBNAILS_COUNT, config.auto_hydrated_thumbnails_count);

        if ctx.record.has_capability(capability::PHOTOS) {
            match self.inner.phone_state(ctx.phone).and_then(|ps| store::state(&ps.db)) {
                Ok(s) => c.sequence_numbers.insert(key::MEDIA_INFO, Value::Int64(s.current_sequence)),
                // An unreadable store is not "nothing stored": no value is stated.
                Err(e) => error!(phone = %ctx.phone, error = %e, "photos store unreadable; media_info left out"),
            }
        }

        c
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        let inner = &self.inner;
        let ps = match inner.phone_state(ctx.phone) {
            Ok(ps) => ps,
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "photos store unavailable; connect reply not applied");
                return;
            }
        };

        if ctx.record.has_capability(capability::PHOTOS) && response.i32(app_key::RESULT) == Some(0) {
            let expected = response.i64(key::MEDIA_INFO).unwrap_or(-1);
            if let Err(e) = store::set_expected(&ps.db, expected) {
                error!(phone = %ctx.phone, error = %e, "media_info expected not stored");
            }
        }

        if let Some(p) = response.object(key::PERMISSIONS) {
            for (name, bit) in [
                (key::PERMISSION_PHOTOS, permission::PHOTOS),
                (key::PERMISSION_PHOTOS_FULL, permission::PHOTOS_FULL),
                (key::PERMISSION_PHOTOS_DELETE, permission::PHOTOS_DELETE),
            ] {
                if let Some(granted) = p.bool(name) {
                    ctx.app.update_phone_record(|r| r.set_permission(bit, granted));
                }
            }
        }

        if let Err(e) = store::set_setting(&ps.db, Setting::FullAccessReminderDismissed, false) {
            error!(phone = %ctx.phone, error = %e, "full-access reminder not re-armed");
        }

        if let Some(caps) = response.object(key::CAPABILITIES).and_then(|c| c.i32(key::CAPABILITY_PHOTOS)) {
            if let Err(e) = store::set_capabilities(&ps.db, caps as u32) {
                error!(phone = %ctx.phone, error = %e, "photo capabilities not stored");
            }
        }

        if response.object(key::MEDIA_SETTINGS).is_some_and(|m| m.str(key::DCIM_DIRECTORY).is_some()) {
            debug!(phone = %ctx.phone, "mediaSettings.dcim_directory ignored; macOS has no phone-files sync root");
        }

        inner.refresh_sync_state(&ps);
        inner.emit(PhotosEvent::StatusChanged { phone: ctx.phone.clone() });
    }
}
