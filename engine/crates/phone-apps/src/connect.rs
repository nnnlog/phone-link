//! The phone apps' contribution to the connect message.
//!
//! Request: `configuration` › `RecentApps` (*recent apps supported*) and `AppHandoff` (*app handoff
//! enabled*), both from the **stored** remoting capabilities, and `Exp006` `true` (the phone reads it
//! as "use the id/checksum sync"); with the phone's `PhoneApps` capability bit, `sequenceNumbers` ›
//! `phoneApps` (the stored `current`, `-1` when none) and, with recent apps supported, `recentApps` —
//! left out when the store cannot be read, rather than sent as an invented value.
//! Response, read from the top level only with the capability bit and `result == 0`: `phoneApps` →
//! `expected` of the app list and, with recent apps supported, `recentApps` → `expected` of the recent
//! list (absent or another type → `-1`). The reply's `capabilities.remoting` was stored with the
//! phone record before this provider runs.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::error;

use phonelink_app::consts::key as app_key;
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::store::{self, Content};
use super::wire::key;
use super::{Inner, PhoneAppsEvent};
use phonelink_features::shared::launch_gates;

/// The provider registered for [`phonelink_app::ConnectProvider::PhoneApps`].
pub(crate) struct Contributor {
    pub(crate) inner: Arc<Inner>,
}

#[async_trait]
impl ConnectContributor for Contributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let recent = launch_gates::recent_apps_supported(ctx.record);
        let mut c = ConnectContribution::default();
        c.configuration.insert(key::CONFIG_RECENT_APPS, recent);
        c.configuration.insert(key::CONFIG_APP_HANDOFF, launch_gates::app_handoff_enabled(ctx.record));
        c.configuration.insert(key::CONFIG_EXP006, true);

        if launch_gates::phone_apps_capable(ctx.record) {
            match self.inner.phone_state(ctx.phone).and_then(|ps| store::sync_state(&ps.db)) {
                Ok(s) => {
                    c.sequence_numbers.insert(key::SEQ_PHONE_APPS, Value::Int64(s.phone_apps.current));
                    if recent {
                        c.sequence_numbers.insert(key::SEQ_RECENT_APPS, Value::Int64(s.recent_apps.current));
                    }
                }
                // An unreadable store is not "nothing stored": no value is stated.
                Err(e) => {
                    error!(phone = %ctx.phone, error = %e, "phone apps store not read; the connect leaves the sequence numbers out")
                }
            }
        }

        c
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        let inner = &self.inner;
        let ps = match inner.phone_state(ctx.phone) {
            Ok(ps) => ps,
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "phone apps store not opened; the connect reply is not applied");
                return;
            }
        };

        if launch_gates::phone_apps_capable(ctx.record) && response.i32(app_key::RESULT) == Some(0) {
            let mut pairs = vec![(Content::PhoneApps, response.i64(key::SEQ_PHONE_APPS).unwrap_or(-1))];
            if launch_gates::recent_apps_supported(ctx.record) {
                pairs.push((Content::RecentApps, response.i64(key::SEQ_RECENT_APPS).unwrap_or(-1)));
            }

            let failures: Vec<_> = pairs
                .into_iter()
                .filter_map(|(content, expected)| {
                    store::set_expected(&ps.db, content, expected).err().map(|e| (content, e))
                })
                .collect();

            if let Some((content, e)) = failures.first() {
                error!(
                    phone = %ctx.phone,
                    error = %e,
                    content_kind = ?content,
                    failed_count = failures.len(),
                    "expected sequence numbers not stored"
                );
            }
        }

        inner.refresh_sync_state(&ps);
        inner.emit(PhoneAppsEvent::StatusChanged { phone: ctx.phone.clone() });
    }
}
