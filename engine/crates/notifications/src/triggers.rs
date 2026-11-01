//! The PC's direct requests and their triggers, and the permission requests: full syncs (a)–(d) and
//! their "syncing" state, dismiss and clear all, the silent permission check that follows dismiss,
//! clear all and full syncs (b) and (c), the permission prompts and their outcome mapping, and the
//! watcher of the App layer's events that fires (b) and (c) on every qualifying permission change
//! (Phone Link on Windows fires them only while its notification list is open).

use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;
use tracing::{debug, error, info, warn};

use phonelink_app::consts::permission;
use phonelink_app::{AppEvent, AppLayer, AppOutcome, RequestOptions};
use phonelink_core::types::DcgClient;

use super::outbound::NotificationsRequest;
use super::store::{self, value as stored};
use super::wire::{capability, names};
use super::{
    Inner, NotificationsError, NotificationsEvent, PermissionState, device_permission, post_notification_needed,
    sensitive_permission,
};
use phonelink_core::sync::MutexExt;

/// Why a full sync (operation 3) is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullSync {
    /// (a) a connect built while the stored capabilities lack `EcrHandlesNotifications`.
    Connect { trigger: String },
    /// (b) the PC or device permission changed and both are on.
    PermissionsChanged,
    /// (c) the sensitive-notification permission became on while both are on.
    SensitivePermissionChanged,
    /// (d) a settings-list change whose trigger-26 request ended *Unknown*.
    SettingsList,
}

impl FullSync {
    fn names(&self) -> (&str, &'static str) {
        match self {
            FullSync::Connect { trigger } => (trigger, names::SCENARIO_CONNECT_FULL_SYNC),
            FullSync::PermissionsChanged => (names::TRIGGER_PERMISSIONS_CHANGED, names::SCENARIO_FULL_SYNC),
            FullSync::SensitivePermissionChanged => {
                (names::TRIGGER_SENSITIVE_PERMISSIONS_CHANGED, names::SCENARIO_FULL_SYNC)
            }
            FullSync::SettingsList => (names::TRIGGER_SETTINGS_LIST, names::SCENARIO_FULL_SYNC),
        }
    }

    /// (a) and (d) record "last updated" on *Success*.
    fn records_last_updated(&self) -> bool {
        matches!(self, FullSync::Connect { .. } | FullSync::SettingsList)
    }

    /// (b) and (c) are followed by the silent permission check.
    fn checks_permission_after(&self) -> bool {
        matches!(self, FullSync::PermissionsChanged | FullSync::SensitivePermissionChanged)
    }
}

impl Inner {
    /// Marks a full sync in flight ("syncing"); with `only_when_idle` it refuses while one is.
    pub(crate) fn begin_full_sync(&self, phone: &DcgClient, only_when_idle: bool) -> bool {
        let (started, first) = self.with_phone(phone, |p| {
            if only_when_idle && p.full_syncs > 0 {
                return (false, false);
            }
            p.full_syncs += 1;
            (true, p.full_syncs == 1)
        });
        if first {
            self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() });
        }
        started
    }

    fn end_full_sync(&self, phone: &DcgClient) {
        let idle = self.with_phone(phone, |p| {
            p.full_syncs = p.full_syncs.saturating_sub(1);
            p.full_syncs == 0
        });
        if idle {
            self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() });
        }
    }

    /// Sends a full sync whose [`Inner::begin_full_sync`] succeeded: every stored key, newest
    /// first, with its `postTime`.
    pub(crate) async fn run_full_sync(
        &self,
        phone: &DcgClient,
        kind: FullSync,
    ) -> Result<AppOutcome, NotificationsError> {
        info!(?kind, "notifications full sync started");
        let sent = self.send_full_sync(phone, &kind).await;
        self.end_full_sync(phone);
        let outcome = sent?;

        // A failed outcome is its caller's: the app's, or `spawn_full_sync`'s.
        if outcome.is_success() {
            info!(?kind, "notifications full sync finished");
        }

        if outcome.is_success() && kind.records_last_updated() {
            self.set_last_updated(phone);
        }

        if kind.checks_permission_after() && outcome != AppOutcome::NeedsPermission {
            self.check_permission_silently(phone).await;
        }
        Ok(outcome)
    }

    async fn send_full_sync(&self, phone: &DcgClient, kind: &FullSync) -> Result<AppOutcome, NotificationsError> {
        let app = self.app()?;
        if !app.selected_phone().is_some_and(|p| p.is_same_client(phone)) {
            return Err(NotificationsError::NoPhone);
        }

        let (keys, post_times) = store::keys_and_post_times(&*self.db(phone)?)?;
        let (trigger, scenario) = kind.names();
        let request = NotificationsRequest::full_sync(keys, post_times).app_request(trigger, scenario);

        let response = app.request(request).await?;
        if !response.outcome.is_success() {
            debug!(?kind, outcome = ?response.outcome, result = ?response.result, "notifications full sync refused");
        }
        Ok(response.outcome)
    }

    /// Runs a full sync whose [`Inner::begin_full_sync`] succeeded on the runtime, without waiting.
    pub(crate) fn spawn_full_sync(self: &Arc<Self>, phone: DcgClient, kind: FullSync) {
        let this = self.clone();
        self.runtime.spawn(async move {
            match this.run_full_sync(&phone, kind).await {
                Ok(outcome) if outcome.is_success() => {}
                Ok(outcome) => warn!(?outcome, "notifications full sync refused by the phone"),
                Err(e) => warn!(error = %e, "notifications full sync not sent"),
            }
        });
    }

    // --------------------------------------------------------------------------------------------
    // The permission watcher: full syncs (b) and (c)
    // --------------------------------------------------------------------------------------------

    /// Watches the App layer's events: each permission change, in order (full syncs (b) and (c)),
    /// and record changes (a newly selected phone, changed capability bits).
    pub(crate) fn spawn_event_watcher(&self, app: &Arc<AppLayer>) {
        let mut events = app.subscribe_events();
        let weak = self.weak_self.clone();
        let shutdown = self.shutdown.clone();

        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = events.recv() => event,
                };

                let Some(inner) = weak.upgrade() else { break };
                let Some(app) = inner.app.upgrade() else { break };

                match event {
                    Ok(AppEvent::PermissionsChanged { phone, bits }) => inner.on_permission_bits(&phone, bits),
                    Ok(AppEvent::PhoneRecordChanged { .. }) => inner.observe_selection(&app),
                    Ok(_) => {}
                    Err(RecvError::Lagged(n)) => {
                        warn!(skipped = n, "notifications missed App events; re-reading the phone state");
                        inner.observe_selection(&app);
                        inner.resync_permissions(&app);
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        });
    }

    /// A newly selected phone starts draining its action queue and gets its permission inputs
    /// recorded; a change of its capability bits or of the post-notification setup stage (its
    /// permissions or OS version) is announced.
    pub(crate) fn observe_selection(&self, app: &AppLayer) {
        let Some(phone) = app.selected_phone() else { return };

        let newly_selected = {
            let mut selected = self.selected.lock_or_panic();
            let changed = !selected.as_ref().is_some_and(|s| s.is_same_client(&phone));
            *selected = Some(phone.clone());
            changed
        };

        if newly_selected {
            if let Some(this) = self.arc() {
                this.wake_queue(&phone);
            }
        }

        let Some(record) = app.phone_record().filter(|r| r.phone().is_same_client(&phone)) else { return };
        if self.with_phone(&phone, |p| p.permissions.is_none()) {
            self.first_look(&phone, device_permission(&record), sensitive_permission(&record));
        }

        let bits = record.capability_bits;
        let stage = post_notification_needed(&record);
        let changed = self.with_phone(&phone, |p| {
            let bits_changed = p.capability_bits.replace(bits).is_some_and(|b| b != bits);
            let stage_changed = p.post_notification_needed.replace(stage).is_some_and(|s| s != stage);
            bits_changed || stage_changed
        });

        if changed {
            self.emit(NotificationsEvent::StatusChanged { phone });
        }
    }

    /// Records the permission inputs of a phone seen for the first time; no transition.
    fn first_look(&self, phone: &DcgClient, device: bool, sensitive: bool) {
        let pc = match self.db(phone).and_then(|db| Self::pc_permission(&db)) {
            Ok(pc) => pc,
            Err(e) => {
                error!(error = %e, "notifications PC permission not read; permission changes are not compared");
                return;
            }
        };

        self.with_phone(phone, |p| {
            p.permissions.get_or_insert(PermissionState { pc, device, sensitive });
        });
    }

    /// One `PermissionsChanged` event: the device and sensitive permissions are its bits `0x40` and
    /// `0x80000000` (a bit no writer has set is clear: both default to off).
    fn on_permission_bits(&self, phone: &DcgClient, bits: u32) {
        let device = bits & permission::NOTIFICATIONS_SYNC != 0;
        let sensitive = bits & permission::NOTIFICATIONS_SENSITIVE != 0;
        if self.with_phone(phone, |p| p.permissions.is_none()) {
            self.first_look(phone, device, sensitive);
            return;
        }
        self.permission_transition(phone, |s| PermissionState { device, sensitive, ..s });
    }

    /// After missed events: the record's current values, as one change.
    fn resync_permissions(&self, app: &AppLayer) {
        let Some(record) = app.phone_record() else { return };
        let (device, sensitive) = (device_permission(&record), sensitive_permission(&record));
        self.permission_transition(&record.phone(), |s| PermissionState { device, sensitive, ..s });
    }

    /// Applies one change of the inputs: (b) when the PC or device permission changed and both are
    /// on, unless a full sync is in flight; (c) when the sensitive permission became on while both
    /// are on.
    fn permission_transition(&self, phone: &DcgClient, change: impl FnOnce(PermissionState) -> PermissionState) {
        let changed = self.with_phone(phone, |p| {
            let before = p.permissions?;
            let after = change(before);
            p.permissions = Some(after);
            Some((before, after))
        });

        let Some((before, after)) = changed else { return };
        if before == after {
            return;
        }

        self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() });
        let Some(this) = self.arc() else { return };

        let both = after.pc && after.device;
        if both && (before.pc != after.pc || before.device != after.device) {
            if self.begin_full_sync(phone, true) {
                this.spawn_full_sync(phone.clone(), FullSync::PermissionsChanged);
            } else {
                debug!("notifications full sync (b) skipped; a full sync is in flight");
            }
        }

        if both && !before.sensitive && after.sensitive {
            self.begin_full_sync(phone, false);
            this.spawn_full_sync(phone.clone(), FullSync::SensitivePermissionChanged);
        }
    }

    // --------------------------------------------------------------------------------------------
    // Permission requests
    // --------------------------------------------------------------------------------------------

    /// The outcome mapping of a notification-permission request: *Success* / *Accepted* ⇒ device
    /// permission on (and setup done); *Denied* / *`PermanentlyDenied`* / *`NeedsPermission`* ⇒ off;
    /// *`DeviceNotSupported`* ⇒ the phone is a low-RAM device; anything else ⇒ unchanged.
    fn apply_permission_outcome(&self, app: &AppLayer, phone: &DcgClient, outcome: &AppOutcome) {
        if !app.selected_phone().is_some_and(|p| p.is_same_client(phone)) {
            return;
        }

        let stored = match outcome {
            AppOutcome::Success | AppOutcome::Accepted => {
                app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, true));
                Some(stored::SETUP_COMPLETED)
            }
            AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission => {
                app.update_phone_record(|r| r.set_permission(permission::NOTIFICATIONS_SYNC, false));
                None
            }
            AppOutcome::DeviceNotSupported => Some(stored::LOW_RAM_DEVICE),
            _ => None,
        };

        if let Some(name) = stored {
            match self.db(phone).and_then(|db| store::set_value(&db, name, 1)) {
                Ok(()) => self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() }),
                Err(e) => error!(error = %e, "notification permission outcome not stored"),
            }
        }
    }

    /// The silent check: `/permission` 64, `silent: true`, not user-initiated.
    pub(crate) async fn check_permission_silently(&self, phone: &DcgClient) -> Option<AppOutcome> {
        let app = self.app().ok()?;
        if !app.selected_phone().is_some_and(|p| p.is_same_client(phone)) {
            return None;
        }

        let options =
            RequestOptions::default().named(names::TRIGGER_CHECK_PERMISSION_STATUS, names::SCENARIO_NOTIFICATIONS);
        let request = app.permission_request(permission::NOTIFICATIONS_SYNC, true, options);

        match app.request(request).await {
            Ok(response) => {
                self.apply_permission_outcome(&app, phone, &response.outcome);
                Some(response.outcome)
            }
            Err(e) => {
                warn!(error = %e, "silent notification permission check not sent");
                None
            }
        }
    }

    fn spawn_silent_check(self: &Arc<Self>, phone: DcgClient) {
        let this = self.clone();
        self.runtime.spawn(async move {
            this.check_permission_silently(&phone).await;
        });
    }

    /// The grant-access prompt: `/permission` 64, `silent: false`, user-initiated.
    pub(crate) async fn request_access(&self) -> Result<AppOutcome, NotificationsError> {
        let (app, phone) = self.selected()?;
        let options =
            RequestOptions::user_initiated().named(names::TRIGGER_PERMISSION_REQUIRED, names::SCENARIO_NOTIFICATIONS);
        let outcome =
            app.request(app.permission_request(permission::NOTIFICATIONS_SYNC, false, options)).await?.outcome;
        self.apply_permission_outcome(&app, &phone, &outcome);
        Ok(outcome)
    }

    /// The sensitive-notification prompt, offered when `NotificationsCapabilities` has `8` and `4`.
    pub(crate) async fn request_sensitive_access(&self) -> Result<AppOutcome, NotificationsError> {
        let (app, phone) = self.selected()?;
        let wanted = capability::SENSITIVE_NOTIFICATIONS_ENABLED | capability::PROFILE_COMPUTER_ROLE_AVAILABLE;
        if Self::capabilities(&*self.db(&phone)?)? & wanted != wanted {
            return Err(NotificationsError::NotAvailable("the sensitive-notification prompt is not offered"));
        }

        let options = RequestOptions::user_initiated()
            .named(names::TRIGGER_SENSITIVE_PERMISSION, names::SCENARIO_SENSITIVE_PERMISSION);
        let request = app.permission_request(permission::NOTIFICATIONS_SENSITIVE, false, options);
        Ok(app.request(request).await?.outcome)
    }

    // --------------------------------------------------------------------------------------------
    // Dismiss, clear all, pin, PC permission
    // --------------------------------------------------------------------------------------------

    /// Operation 2: the row is deleted and its notification withdrawn before sending.
    pub(crate) async fn dismiss(self: &Arc<Self>, key: &str) -> Result<AppOutcome, NotificationsError> {
        let (app, phone) = self.selected()?;
        let deleted = store::delete_rows(&*self.db(&phone)?, &[key.to_string()])?;

        if !deleted.is_empty() {
            self.withdraw(&phone, deleted.into_iter().map(|r| r.key));
            self.emit(NotificationsEvent::ListChanged { phone: phone.clone() });
        }

        let request =
            NotificationsRequest::dismiss(key).app_request(names::TRIGGER_DISMISS, names::SCENARIO_NOTIFICATIONS);
        let outcome = app.request(request).await?.outcome;

        if outcome != AppOutcome::NeedsPermission {
            self.spawn_silent_check(phone);
        }
        Ok(outcome)
    }

    /// Operation 4 with the cleared items that are still stored; nothing when none is. On
    /// *Success* their rows, queued requests and notifications go.
    pub(crate) async fn clear_all(self: &Arc<Self>) -> Result<Option<AppOutcome>, NotificationsError> {
        let (app, phone) = self.selected()?;
        let cleared = self.clearable_keys(&phone)?;
        let db = self.db(&phone)?;

        let mut still_stored = Vec::new();
        for k in &cleared {
            if store::row_by_key(&db, k)?.is_some() {
                still_stored.push(k.clone());
            }
        }

        if still_stored.is_empty() {
            debug!("clear all not sent; no cleared notification is still stored");
            return Ok(None);
        }

        let request = NotificationsRequest::clear_all(still_stored)
            .app_request(names::TRIGGER_CLEAR_ALL, names::SCENARIO_NOTIFICATIONS);
        let outcome = app.request(request).await?.outcome;

        if outcome.is_success() {
            let deleted = store::delete_rows(&db, &cleared)?;
            store::drop_queued(&db, &cleared)?;
            self.withdraw(&phone, cleared);
            if !deleted.is_empty() {
                self.emit(NotificationsEvent::ListChanged { phone: phone.clone() });
            }
        }

        if outcome != AppOutcome::NeedsPermission {
            self.spawn_silent_check(phone);
        }
        Ok(Some(outcome))
    }

    pub(crate) fn set_pinned(&self, key: &str, pinned: bool) -> Result<(), NotificationsError> {
        let (_, phone) = self.selected()?;
        let state = if pinned { store::pinned::PINNED } else { store::pinned::UNPINNED };
        if !store::set_state(&*self.db(&phone)?, key, state)? {
            return Err(NotificationsError::UnknownNotification);
        }
        self.emit(NotificationsEvent::ListChanged { phone });
        Ok(())
    }

    /// The PC permission switch; a change is compared like a phone-reported one (full sync (b)).
    pub(crate) fn set_pc_permission(self: &Arc<Self>, on: bool) -> Result<(), NotificationsError> {
        let (app, phone) = self.selected()?;
        let db = self.db(&phone)?;
        let before = Self::pc_permission(&db)?;
        store::set_value(&db, stored::PC_PERMISSION, i64::from(on))?;

        if before != on {
            info!(enabled = on, "notifications on this Mac switched");
        }

        if self.with_phone(&phone, |p| p.permissions.is_none()) {
            if let Some(record) = app.phone_record().filter(|r| r.phone().is_same_client(&phone)) {
                self.first_look(&phone, device_permission(&record), sensitive_permission(&record));
            }
            if before != on {
                self.emit(NotificationsEvent::StatusChanged { phone });
            }
        } else {
            self.permission_transition(&phone, |s| PermissionState { pc: on, ..s });
        }
        Ok(())
    }
}
