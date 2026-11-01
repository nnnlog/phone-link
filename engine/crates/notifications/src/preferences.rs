//! Per-app notification settings operations: the `/settings` requests — one in flight at a time —
//! and their answers, the settings view's sync-all (25), single-app changes (26) from the list's
//! context menu, its undo and the settings list (with full sync (d)), multi-app changes (27, or one
//! 26 per app), and the local-only banner flags, mutes, mute-all and throttling opt-out.

use std::sync::Arc;

use phonelink_app::{AppLayer, AppOutcome, AppRequest};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, StoreError};

use super::outbound::settings_request;
use super::settings::{self, AppFlags, SettingRow, SyncSettingItem};
use super::store::{self, value as stored};
use super::triggers::FullSync;
use super::wire::{BATCH_NOTIFICATION_SETTINGS, names, setting};
use super::{
    AppSettingView, Inner, NotificationsError, NotificationsEvent, PreferencesChange, PreferencesResult,
    PreferencesSource, app_list_supported,
};

fn accepted(outcome: &AppOutcome) -> bool {
    matches!(outcome, AppOutcome::Success | AppOutcome::Accepted)
}

impl Inner {
    /// Sends one settings request (one in flight at a time); on *Success* the answer's settings
    /// replace the stored rows with their `(groupId, key)`.
    async fn send_settings(
        &self,
        app: &AppLayer,
        phone: &DcgClient,
        request: AppRequest,
    ) -> Result<AppOutcome, NotificationsError> {
        let _one_at_a_time = self.settings_gate.lock().await;
        let response = app.request(request).await?;

        if response.outcome.is_success() {
            let rows = response.values().map(settings::parse_response).unwrap_or_default();
            if !rows.is_empty() {
                store::put_settings(&*self.db(phone)?, &rows)?;
                self.emit(NotificationsEvent::SettingsChanged { phone: phone.clone() });
            }
        }
        Ok(response.outcome)
    }

    /// Trigger 25 when the phone supports the per-app list; `None` otherwise.
    pub(crate) async fn open_settings_view(&self) -> Result<Option<AppOutcome>, NotificationsError> {
        let (app, phone) = self.selected()?;
        let record = app.phone_record().ok_or(NotificationsError::NoPhone)?;

        if !app_list_supported(&record) {
            return Ok(None);
        }

        let request = settings_request(
            settings::sync_all_body,
            false,
            names::TRIGGER_SETTINGS_VIEW_OPENED,
            names::SCENARIO_SYNC_ALL_SETTINGS,
        );
        Ok(Some(self.send_settings(&app, &phone, request).await?))
    }

    /// Trigger 26 with the feed flag of `flags`.
    async fn send_sync_setting(
        &self,
        app: &AppLayer,
        phone: &DcgClient,
        package: &str,
        flags: AppFlags,
        trigger: &str,
    ) -> Result<AppOutcome, NotificationsError> {
        let item = SyncSettingItem::new(package, flags);
        let request = settings_request(
            |cv| settings::setting_changed_body(&item, cv),
            true,
            trigger,
            names::SCENARIO_UPDATE_SETTING,
        );
        self.send_settings(app, phone, request).await
    }

    /// The local row of `package` keeps the banner bit of `flags` and its mute.
    fn write_local_flags(db: &FeatureDb, package: &str, flags: AppFlags) -> Result<(), StoreError> {
        let mute = store::setting(db, setting::LOCAL_GROUP, package)?.map_or(0, |r| r.mute_expiration_ticks);
        store::put_settings(
            db,
            &[SettingRow {
                group_id: setting::LOCAL_GROUP.to_string(),
                key: package.to_string(),
                setting_type: setting::TYPE_FLAGS,
                value: flags.banner_part().0.to_string(),
                mute_expiration_ticks: mute,
            }],
        )
    }

    /// The list's context-menu toggle and its undo: trigger 26 with the new feed flag — sent even
    /// when only the banner part changed — then the local banner flag. The outcome is the request's.
    pub(crate) async fn update_app_setting(
        &self,
        package: &str,
        flags: AppFlags,
        trigger: &str,
    ) -> Result<AppOutcome, NotificationsError> {
        let (app, phone) = self.selected()?;
        let outcome = self.send_sync_setting(&app, &phone, package, flags, trigger).await?;
        Self::write_local_flags(&*self.db(&phone)?, package, flags)?;
        self.emit(NotificationsEvent::SettingsChanged { phone });
        Ok(outcome)
    }

    /// The settings list: trigger 26 only when the feed flag changes, and full sync (d) when that
    /// request ends *Unknown*; the banner flag is written only when the sync part succeeded.
    pub(crate) async fn change_app_setting(
        self: &Arc<Self>,
        package: &str,
        flags: AppFlags,
    ) -> Result<AppOutcome, NotificationsError> {
        let (app, phone) = self.selected()?;
        let db = self.db(&phone)?;
        let (old, _) = self.app_flags(&db, package)?;

        let mut outcome = AppOutcome::Success;
        if old.feed() != flags.feed() {
            outcome =
                self.send_sync_setting(&app, &phone, package, flags.feed_part(), names::TRIGGER_SETTINGS_LIST).await?;
            if outcome == AppOutcome::Unknown {
                self.begin_full_sync(&phone, false);
                outcome = self.run_full_sync(&phone, FullSync::SettingsList).await?;
            }
        }

        if old.banner() != flags.banner() && outcome.is_success() {
            Self::write_local_flags(&db, package, flags)?;
            self.emit(NotificationsEvent::SettingsChanged { phone });
        }
        Ok(outcome)
    }

    /// A banner-menu change: the local banner flag only.
    pub(crate) fn set_local_flags(&self, package: &str, flags: AppFlags) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        Self::write_local_flags(&*self.db(&phone)?, package, flags)?;
        self.emit(NotificationsEvent::SettingsChanged { phone });
        Ok(())
    }

    /// The app's mute expiry (UTC ticks; `0` = unmuted) in its local row; a new row reads All.
    pub(crate) fn set_local_mute(&self, package: &str, until_ticks: i64) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        let db = self.db(&phone)?;
        let value = store::setting(&db, setting::LOCAL_GROUP, package)?
            .map_or_else(|| "AllNotifications".to_string(), |r| r.value);

        store::put_settings(
            &db,
            &[SettingRow {
                group_id: setting::LOCAL_GROUP.to_string(),
                key: package.to_string(),
                setting_type: setting::TYPE_FLAGS,
                value,
                mute_expiration_ticks: until_ticks,
            }],
        )?;

        self.emit(NotificationsEvent::SettingsChanged { phone });
        Ok(())
    }

    pub(crate) fn set_mute_all(&self, on: bool) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        let until = if on {
            let ms = i64::try_from(self.config.mute_all_duration.as_millis()).unwrap_or(i64::MAX);
            phonelink_core::time::ticks_from_unix_ms(self.now_ms().saturating_add(ms))
        } else {
            0
        };

        store::set_value(&*self.db(&phone)?, stored::MUTE_ALL_UNTIL_TICKS, until)?;
        self.emit(NotificationsEvent::StatusChanged { phone });
        Ok(())
    }

    pub(crate) fn set_throttling_enabled(&self, phone: &DcgClient, on: bool) -> Result<(), NotificationsError> {
        store::set_value(&*self.db(phone)?, stored::THROTTLING_ENABLED, i64::from(on))?;
        self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() });
        Ok(())
    }

    pub(crate) fn app_setting(&self, package: &str) -> Result<AppSettingView, NotificationsError> {
        let (_, phone) = self.selected()?;
        let (flags, mute) = self.app_flags(&*self.db(&phone)?, package)?;
        let now_ticks = phonelink_core::time::ticks_from_unix_ms(self.now_ms());
        Ok(AppSettingView {
            package_name: package.to_string(),
            flags,
            muted_until_ms: (mute > now_ticks).then(|| phonelink_core::time::unix_ms_from_ticks(mute)),
        })
    }

    /// A multi-app change of the setup / preferences dialog: the overrides' local banner flags, then
    /// the sync flags of the overrides and of the apps that follow the default (not on the first
    /// setup save) — trigger 27 when batching is available, else one trigger 26 per app — then, with
    /// quiet-by-default, the default-following apps lose their local rows (a muted one keeps its
    /// row, with the default's banner bit).
    pub(crate) async fn apply_preferences(
        &self,
        change: PreferencesChange,
    ) -> Result<PreferencesResult, NotificationsError> {
        let (app, phone) = self.selected()?;
        let db = self.db(&phone)?;
        let was_set = Self::bool_value(&db, stored::DEFAULT_PREFERENCE_SET, false)?;

        if let Some(d) = change.new_default {
            store::set_value(&db, stored::DEFAULT_PREFERENCE, i64::from(d.0))?;
            store::set_value(&db, stored::DEFAULT_PREFERENCE_SET, 1)?;
        }

        let effective_default = Self::default_preference(&db)?;
        let first_run = change.source == PreferencesSource::Setup && !was_set;
        let trigger = match change.source {
            PreferencesSource::SettingsDialog => names::TRIGGER_PREFERENCES_DIALOG,
            PreferencesSource::Setup => names::TRIGGER_PREFERENCES_SETUP,
        };

        let mut apps: Vec<(String, AppFlags)> = change.overrides.clone();
        if !first_run {
            for package in &change.default_matching {
                match apps.iter_mut().find(|(p, _)| p == package) {
                    Some(slot) => slot.1 = effective_default,
                    None => apps.push((package.clone(), effective_default)),
                }
            }
        }

        let mut result = PreferencesResult { succeeded: 0, failed: 0 };
        if !apps.is_empty() {
            for (package, flags) in &change.overrides {
                Self::write_local_flags(&db, package, *flags)?;
            }
            result = self.sync_apps(&app, &phone, &apps, trigger).await?;
        }

        if !change.default_matching.is_empty() && self.config.quiet_by_default {
            self.clear_local(&db, &change.default_matching, effective_default.banner_part())?;
        }
        self.emit(NotificationsEvent::SettingsChanged { phone });
        Ok(result)
    }

    async fn sync_apps(
        &self,
        app: &AppLayer,
        phone: &DcgClient,
        apps: &[(String, AppFlags)],
        trigger: &str,
    ) -> Result<PreferencesResult, NotificationsError> {
        let record = app.phone_record().ok_or(NotificationsError::NoPhone)?;
        let batch = self.config.quiet_by_default
            && record.device_capability_negotiation.is_some_and(|n| n & BATCH_NOTIFICATION_SETTINGS != 0);

        if batch {
            let items: Vec<SyncSettingItem> = apps.iter().map(|(p, f)| SyncSettingItem::new(p, *f)).collect();
            let request = settings_request(
                |cv| settings::batch_body(&items, cv),
                true,
                trigger,
                names::SCENARIO_BATCH_UPDATE_SETTINGS,
            );

            let outcome = self.send_settings(app, phone, request).await?;
            let n = apps.len();
            return Ok(if accepted(&outcome) {
                PreferencesResult { succeeded: n, failed: 0 }
            } else {
                PreferencesResult { succeeded: 0, failed: n }
            });
        }

        let mut result = PreferencesResult { succeeded: 0, failed: 0 };
        for (package, flags) in apps {
            if accepted(&self.send_sync_setting(app, phone, package, *flags, trigger).await?) {
                result.succeeded += 1;
            } else {
                result.failed += 1;
            }
        }
        Ok(result)
    }

    /// Default-following apps drop their local rows; a row with an active mute stays, holding the
    /// default's banner bit.
    fn clear_local(&self, db: &FeatureDb, packages: &[String], default_banner: AppFlags) -> Result<(), StoreError> {
        let now_ticks = phonelink_core::time::ticks_from_unix_ms(self.now_ms());
        let mut keep = Vec::new();
        let mut delete = Vec::new();

        for package in packages {
            match store::setting(db, setting::LOCAL_GROUP, package)? {
                Some(mut row) if row.mute_expiration_ticks > now_ticks => {
                    row.value = default_banner.0.to_string();
                    keep.push(row);
                }
                _ => delete.push(package.clone()),
            }
        }

        store::put_settings(db, &keep)?;
        store::delete_settings(db, setting::LOCAL_GROUP, &delete)
    }
}
