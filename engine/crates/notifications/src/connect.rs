//! The `SyncNotifications` connect provider, Android only: `configuration.AudioInfoSyncEnabled`
//! always; with both notification permissions on, the reconcile keys and
//! `supportsNotificationSyncCompletion` when the stored `NotificationsCapabilities` has
//! `EcrHandlesNotifications`, else a separate full sync (a) started while the connect is built,
//! without waiting. From the response: `permissions.notifications`, `capabilities.notifications`,
//! and `notificationSyncCompleted`, whose `true` sets the "last updated" time.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::error;

use phonelink_app::consts::{capability as phone_capability, permission};
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::store::{self, value as stored};
use super::triggers::FullSync;
use super::wire::{capability, key, operation};
use super::{Inner, NotificationsEvent, device_permission, is_android};

pub(crate) struct Contributor {
    inner: Arc<Inner>,
}

impl Contributor {
    pub(crate) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: inner.clone() }
    }
}

#[async_trait]
impl ConnectContributor for Contributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let mut c = ConnectContribution::default();
        if !is_android(ctx.record) {
            return c;
        }

        c.configuration
            .insert(key::AUDIO_INFO_SYNC_ENABLED, ctx.record.has_capability(phone_capability::AUDIO_CONTROL));

        let inner = &self.inner;
        let db = match inner.db(ctx.phone) {
            Ok(db) => db,
            Err(e) => {
                error!(error = %e, "notifications store not opened; the connect carries no reconcile keys");
                return c;
            }
        };

        let gate = Inner::pc_permission(&db).and_then(|pc| Ok((pc, Inner::capabilities(&db)?)));
        let (pc, capabilities) = match gate {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "notifications state not read; the connect carries no reconcile keys");
                return c;
            }
        };

        if !(pc && device_permission(ctx.record)) {
            return c;
        }

        if capabilities & capability::ECR_HANDLES_NOTIFICATIONS != 0 {
            match store::keys_and_post_times(&db) {
                Ok((keys, post_times)) => {
                    let count = i32::try_from(keys.len()).expect("fewer than 2^31 stored notifications");
                    c.top_level.insert(key::OPERATION, operation::EXISTING_NOTIFICATION);
                    c.top_level.insert(key::NOTIFICATION_KEYS_COUNT, count);
                    c.top_level.insert(key::NOTIFICATION_KEYS, Value::StringArray(keys));
                    c.top_level.insert(key::POST_TIMES, Value::Int64Array(post_times));
                    c.top_level.insert(key::SUPPORTS_NOTIFICATION_SYNC_COMPLETION, true);
                }
                Err(e) => error!(error = %e, "stored notifications not read; the connect carries no reconcile keys"),
            }
        } else {
            inner.begin_full_sync(ctx.phone, false);
            inner.spawn_full_sync(ctx.phone.clone(), FullSync::Connect { trigger: ctx.trigger.to_string() });
        }

        c
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        if !is_android(ctx.record) {
            return;
        }

        if let Some(granted) = response.object(key::PERMISSIONS).and_then(|p| p.bool(key::NOTIFICATIONS_ENTRY)) {
            ctx.app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, granted));
        }

        let reported = response.object(key::CAPABILITIES).and_then(|c| c.i32(key::NOTIFICATIONS_ENTRY));
        if let Some(capabilities) = reported {
            let stored = self.inner.db(ctx.phone).and_then(|db| {
                let before = Inner::capabilities(&db)?;
                store::set_value(&db, stored::CAPABILITIES, i64::from(capabilities))?;
                Ok(before)
            });
            match stored {
                Ok(before) if before != capabilities => {
                    self.inner.emit(NotificationsEvent::StatusChanged { phone: ctx.phone.clone() })
                }
                Ok(_) => {}
                Err(e) => error!(error = %e, "NotificationsCapabilities not stored; the stored capabilities are kept"),
            }
        }

        if response.bool(key::NOTIFICATION_SYNC_COMPLETED) == Some(true) {
            self.inner.set_last_updated(ctx.phone);
        }
    }
}
