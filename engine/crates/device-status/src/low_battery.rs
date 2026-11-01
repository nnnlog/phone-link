//! The low-battery notification.

use super::{Inner, battery, key, select};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::FeatureDb;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

impl Inner {
    /// The selected battery level and charging state of `phone`.
    pub(super) fn selected_battery(&self, phone: &DcgClient) -> (Option<i32>, bool) {
        let record = self.record_for(phone);
        let mut state = self.state.lock_or_panic();
        let ps = state.phone(phone);
        let default_keys = phonelink_app::StatusKeys::default();
        let sel = select(record.as_ref().map_or(&default_keys, |r| &r.status_keys), ps.cdeh.as_ref());
        (sel.battery_percent, sel.battery_charging)
    }

    /// The rule runs at start, on a change of the selected phone and on every change of its
    /// selected battery level — not of the charging state.
    pub(super) fn check_battery(&self) {
        let Some(phone) = self.env.app.selected_phone() else { return };
        let (level, _) = self.selected_battery(&phone);
        let now = (key(&phone), level);

        {
            let mut state = self.state.lock_or_panic();
            if state.battery.as_ref() == Some(&now) {
                return;
            }
            state.battery = Some(now);
        }

        // Off the caller's task (the MSAEP observer runs on the router's); the store read and the
        // post are short.
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                inner.evaluate_low_battery(&phone);
            }
        });
    }

    pub(super) fn evaluate_low_battery(&self, phone: &DcgClient) {
        let _guard = self.battery_lock.lock_or_panic();
        let (level, charging) = self.selected_battery(phone);
        let Some(percent) = level.filter(|_| battery::is_low(level, charging, self.config.low_battery_threshold))
        else {
            return;
        };

        let Some((db, settings)) = self.low_battery_settings(phone) else {
            return;
        };
        let now = self.env.edges.clock.now();
        if !settings.enabled || !battery::interval_passed(settings.last_shown, now, self.config.low_battery_interval) {
            return;
        }

        if !self.env.notifier.is_authorized() {
            debug!(%phone, "low-battery notification not posted; notifications are not authorized");
            return;
        }
        self.post_low_battery(phone, &db, settings, percent, now);
    }

    /// The phone's store and its low-battery settings, `None` (logged once while it lasts) when
    /// either is unavailable.
    fn low_battery_settings(&self, phone: &DcgClient) -> Option<(Arc<FeatureDb>, battery::LowBatterySettings)> {
        let read = self.db(phone).and_then(|db| Ok((battery::load(&db)?, db)));
        match read {
            Ok((settings, db)) => {
                self.low_battery_store_failing.clear();
                Some((db, settings))
            }
            Err(e) => {
                if self.low_battery_store_failing.starts() {
                    error!(%phone, error = %e, "low-battery settings not read; no low-battery notification is posted");
                }
                None
            }
        }
    }

    /// Posts the low-battery notification and, once posted, records when it was shown.
    fn post_low_battery(
        &self,
        phone: &DcgClient,
        db: &FeatureDb,
        settings: battery::LowBatterySettings,
        percent: i32,
        now: SystemTime,
    ) {
        let name = self
            .record_for(phone)
            .and_then(|r| r.display_name.filter(|n| !n.trim().is_empty()).or(r.name))
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Phone".to_owned());

        match self.env.notifier.post(battery::notification(phone, &name, percent)) {
            Ok(()) => {
                self.low_battery_post_failing.clear();
                info!(%phone, percent, "low-battery notification posted");
                if let Err(e) = battery::save(db, battery::LowBatterySettings { last_shown: Some(now), ..settings }) {
                    error!(%phone, error = %e, "low-battery notification time not stored; the next level change may post it again");
                }
            }
            Err(e) => {
                if self.low_battery_post_failing.starts() {
                    warn!(%phone, error = %e, "low-battery notification not posted; each level change tries again");
                }
            }
        }
    }
}
