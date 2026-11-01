//! The notifications feature for the UI ([`crate::features::notifications`]).
//!
//! It covers the phone's notification list and what can be done with it, the per-app settings, the
//! PC switch and mutes, and the phone-side permission prompts.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::notifications::{
    self as feature, AppFlags, AppSettingView, ConversationLine, ItemAction, MuteDuration, NotificationView,
    NotificationsError, NotificationsEvent, NotificationsStatus, OpenTarget, PreferencesChange, PreferencesSource,
};

use super::{FeatureApiError, RequestOutcome, core_of, count, features_of};

impl From<NotificationsError> for FeatureApiError {
    fn from(e: NotificationsError) -> Self {
        match e {
            NotificationsError::NoPhone => FeatureApiError::NoPhone,
            NotificationsError::AppGone => FeatureApiError::NotStarted,
            NotificationsError::UnknownNotification => FeatureApiError::UnknownItem { what: "notification".into() },
            NotificationsError::UnknownAction => FeatureApiError::UnknownItem { what: "notification action".into() },
            NotificationsError::NotAvailable(reason) => FeatureApiError::NotAvailable { reason: reason.into() },
            NotificationsError::Store(e) => e.into(),
            NotificationsError::App(e) => e.into(),
        }
    }
}

/// Where a click on a notification takes the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationOpenTarget {
    Notifications,
    Messaging,
    Calls,
}

impl From<OpenTarget> for NotificationOpenTarget {
    fn from(t: OpenTarget) -> Self {
        match t {
            OpenTarget::Notifications => NotificationOpenTarget::Notifications,
            OpenTarget::Messaging => NotificationOpenTarget::Messaging,
            OpenTarget::Calls => NotificationOpenTarget::Calls,
        }
    }
}

/// One line of a messaging-style notification: a sender heading or a message.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationLine {
    Sender { name: String },
    Message { text: String },
}

/// A button (or the reply box) of a notification.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationButton {
    pub name: String,
    pub inline_reply: bool,
    /// What `press_action` / `reply` pass back.
    pub index: i32,
    /// The entry's position among the notification's actions.
    pub position: u32,
}

impl From<&ItemAction> for NotificationButton {
    fn from(a: &ItemAction) -> Self {
        Self { name: a.name.clone(), inline_reply: a.inline_reply, index: a.index, position: count(a.position) }
    }
}

/// One item of the phone's notification list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneNotification {
    pub key: String,
    pub package_name: String,
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub body: String,
    /// A messaging-style conversation, empty for other notifications.
    pub conversation: Vec<NotificationLine>,
    /// Unix ms.
    pub time_ms: i64,
    pub pinned: bool,
    pub open_target: NotificationOpenTarget,
    pub launch_supported: bool,
    pub is_read_only_group_message: bool,
    /// The reply box: the first reply action.
    pub reply: Option<NotificationButton>,
    /// Up to four buttons.
    pub actions: Vec<NotificationButton>,
}

impl From<&NotificationView> for PhoneNotification {
    fn from(v: &NotificationView) -> Self {
        Self {
            key: v.key.clone(),
            package_name: v.package_name.clone(),
            app_name: v.app_name.clone(),
            title: v.title.clone(),
            subtitle: v.subtitle.clone(),
            body: v.body.clone(),
            conversation: v
                .conversation
                .iter()
                .map(|l| match l {
                    ConversationLine::Sender(name) => NotificationLine::Sender { name: name.clone() },
                    ConversationLine::Message(text) => NotificationLine::Message { text: text.clone() },
                })
                .collect(),
            time_ms: v.time_ms,
            pinned: v.pinned,
            open_target: v.open_target.into(),
            launch_supported: v.launch_supported,
            is_read_only_group_message: v.is_read_only_group_message,
            reply: v.reply.as_ref().map(Into::into),
            actions: v.actions.iter().map(Into::into).collect(),
        }
    }
}

/// The feature's state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationsState {
    /// The phone has the notifications listener: the feature exists.
    pub capable: bool,
    /// The phone reports its app list: per-app settings apply.
    pub app_list_supported: bool,
    /// "Show notifications from my phone" (default on).
    pub pc_permission: bool,
    /// The phone's notification access.
    pub device_permission: bool,
    /// The phone's sensitive-notification access.
    pub sensitive_permission: bool,
    /// The sensitive-notification prompt is offered.
    pub sensitive_prompt_offered: bool,
    pub syncing: bool,
    pub last_updated_ms: Option<i64>,
    /// Every banner is muted until this time (Unix ms).
    pub muted_all_until_ms: Option<i64>,
    pub throttling_enabled: bool,
    /// The phone answered the access request *device not supported* (low-RAM phone).
    pub low_ram_device: bool,
    pub setup_completed: bool,
    /// The first setup stage: Link to Windows on the phone (Android 13 or later) needs the
    /// permission to post notifications, which the user grants on the phone.
    pub post_notification_needed: bool,
}

impl From<&NotificationsStatus> for NotificationsState {
    fn from(s: &NotificationsStatus) -> Self {
        Self {
            capable: s.capable,
            app_list_supported: s.app_list_supported,
            pc_permission: s.pc_permission,
            device_permission: s.device_permission,
            sensitive_permission: s.sensitive_permission,
            sensitive_prompt_offered: s.sensitive_prompt_offered,
            syncing: s.syncing,
            last_updated_ms: s.last_updated_ms,
            muted_all_until_ms: s.muted_all_until_ms,
            throttling_enabled: s.throttling_enabled,
            low_ram_device: s.low_ram_device,
            setup_completed: s.setup_completed,
            post_notification_needed: s.post_notification_needed,
        }
    }
}

/// An app's notification flags: `feed` is "show in the list" (sent to the phone), `banner` is "show
/// banners" (this Mac only); `other_bits` keeps any further bits the phone reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct AppNotificationFlags {
    pub feed: bool,
    pub banner: bool,
    pub other_bits: u32,
}

impl From<AppFlags> for AppNotificationFlags {
    fn from(f: AppFlags) -> Self {
        Self { feed: f.feed(), banner: f.banner(), other_bits: f.0 & !AppFlags::ALL.0 }
    }
}

impl From<AppNotificationFlags> for AppFlags {
    fn from(f: AppNotificationFlags) -> Self {
        AppFlags(f.other_bits & !AppFlags::ALL.0).with_feed(f.feed).with_banner(f.banner)
    }
}

/// An app's setting as the settings view shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppNotificationSetting {
    pub package_name: String,
    pub flags: AppNotificationFlags,
    /// Banners muted until this time (Unix ms).
    pub muted_until_ms: Option<i64>,
}

impl From<&AppSettingView> for AppNotificationSetting {
    fn from(v: &AppSettingView) -> Self {
        Self { package_name: v.package_name.clone(), flags: v.flags.into(), muted_until_ms: v.muted_until_ms }
    }
}

/// How long [`NotificationsApi::mute_app`] mutes an app's banners: one hour, eight hours or a day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationMuteDuration {
    OneHour,
    EightHours,
    OneDay,
}

impl From<NotificationMuteDuration> for MuteDuration {
    fn from(d: NotificationMuteDuration) -> Self {
        match d {
            NotificationMuteDuration::OneHour => MuteDuration::OneHour,
            NotificationMuteDuration::EightHours => MuteDuration::EightHours,
            NotificationMuteDuration::OneDay => MuteDuration::OneDay,
        }
    }
}

/// Which dialog a [`NotificationPreferencesChange`] comes from: the settings or the first setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationPreferencesSource {
    SettingsDialog,
    Setup,
}

/// One app whose setting differs from the default in a preferences change.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AppNotificationOverride {
    pub package_name: String,
    pub flags: AppNotificationFlags,
}

/// A multi-app change of the setup or preferences dialog.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationPreferencesChange {
    pub source: NotificationPreferencesSource,
    pub overrides: Vec<AppNotificationOverride>,
    /// Apps that follow the default.
    pub default_matching: Vec<String>,
    /// The default the user chose in this change, if any.
    pub new_default: Option<AppNotificationFlags>,
}

impl From<NotificationPreferencesChange> for PreferencesChange {
    fn from(c: NotificationPreferencesChange) -> Self {
        PreferencesChange {
            source: match c.source {
                NotificationPreferencesSource::SettingsDialog => PreferencesSource::SettingsDialog,
                NotificationPreferencesSource::Setup => PreferencesSource::Setup,
            },
            overrides: c.overrides.into_iter().map(|o| (o.package_name, o.flags.into())).collect(),
            default_matching: c.default_matching,
            new_default: c.new_default.map(Into::into),
        }
    }
}

/// How many apps of a preferences change reached the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct NotificationPreferencesResult {
    pub succeeded: u32,
    pub failed: u32,
}

/// What changed, for the UI to re-read.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationsChange {
    ListChanged,
    StateChanged,
    SettingsChanged,
    /// A queued action or reply reached its final status; `outcome` is `None` when it was dropped
    /// unsent after the queue expiry.
    ActionFinished {
        key: String,
        reply: bool,
        outcome: Option<RequestOutcome>,
    },
    /// The user clicked a posted notification.
    OpenRequested {
        key: String,
        target: NotificationOpenTarget,
    },
}

impl From<&NotificationsEvent> for NotificationsChange {
    fn from(e: &NotificationsEvent) -> Self {
        match e {
            NotificationsEvent::ListChanged { .. } => NotificationsChange::ListChanged,
            NotificationsEvent::StatusChanged { .. } => NotificationsChange::StateChanged,
            NotificationsEvent::SettingsChanged { .. } => NotificationsChange::SettingsChanged,
            NotificationsEvent::ActionFinished { key, reply, outcome, .. } => NotificationsChange::ActionFinished {
                key: key.clone(),
                reply: *reply,
                outcome: outcome.as_ref().map(Into::into),
            },
            NotificationsEvent::OpenRequested { key, target, .. } => {
                NotificationsChange::OpenRequested { key: key.clone(), target: (*target).into() }
            }
        }
    }
}

/// The notifications feature of the selected phone.
#[derive(uniffi::Object)]
pub struct NotificationsApi {
    engine: Arc<Engine>,
}

impl NotificationsApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<Arc<feature::Notifications>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        Ok(features_of(&core)?.notifications.clone())
    }

    /// Runs `f` on the feature inside the engine's runtime (the foreign side calls on its own threads).
    fn run<T>(
        &self,
        f: impl FnOnce(&feature::Notifications) -> Result<T, NotificationsError>,
    ) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.notifications.clone();
        Ok(f(&feature)?)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl NotificationsApi {
    /// The selected phone's notification access, this Mac's settings for it, and the sync state.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<NotificationsState, FeatureApiError> {
        self.run(|f| f.status()).map(|s| (&s).into())
    }

    /// The phone's notifications, newest first.
    pub fn list(&self) -> Result<Vec<PhoneNotification>, FeatureApiError> {
        self.run(|f| f.list()).map(|v| v.iter().map(Into::into).collect())
    }

    /// Dismisses one notification here and on the phone.
    pub async fn dismiss(&self, key: String) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.dismiss(&key).await?.into())
    }

    /// Clears every unpinned notification; `None` when there was nothing to clear.
    pub async fn clear_all(&self) -> Result<Option<RequestOutcome>, FeatureApiError> {
        Ok(self.feature()?.clear_all().await?.map(Into::into))
    }

    /// Pins or unpins a notification (this Mac only).
    pub fn set_pinned(&self, key: String, pinned: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_pinned(&key, pinned))
    }

    /// Presses a notification's button.
    pub fn press_action(&self, key: String, action_index: i32) -> Result<(), FeatureApiError> {
        self.run(|f| f.press_action(&key, action_index))
    }

    /// Sends a reply from a notification's reply box.
    pub fn reply(&self, key: String, action_index: i32, text: String) -> Result<(), FeatureApiError> {
        self.run(|f| f.reply(&key, action_index, &text))
    }

    /// The settings view opened (it asks the phone for its app list when supported).
    pub async fn open_settings_view(&self) -> Result<Option<RequestOutcome>, FeatureApiError> {
        Ok(self.feature()?.open_settings_view().await?.map(Into::into))
    }

    /// The notification setting of the app `package_name`, and until when its banners are muted.
    pub fn app_setting(&self, package_name: String) -> Result<AppNotificationSetting, FeatureApiError> {
        self.run(|f| f.app_setting(&package_name)).map(|s| (&s).into())
    }

    /// Turns an app's notifications off (or on) from a list item's context menu.
    pub async fn toggle_app_from_list(
        &self,
        package_name: String,
        enabled: bool,
    ) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.toggle_app_from_list(&package_name, enabled).await?.into())
    }

    /// Undoes `toggle_app_from_list` by re-applying the app's previous flags.
    pub async fn undo_app_turn_off(
        &self,
        package_name: String,
        previous: AppNotificationFlags,
    ) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.undo_app_turn_off(&package_name, previous.into()).await?.into())
    }

    /// A change in the settings list.
    pub async fn change_app_setting(
        &self,
        package_name: String,
        flags: AppNotificationFlags,
    ) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.change_app_setting(&package_name, flags.into()).await?.into())
    }

    /// Changes only an app's banner flag (this Mac only).
    pub fn set_app_local_setting(
        &self,
        package_name: String,
        flags: AppNotificationFlags,
    ) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_app_local_setting(&package_name, flags.into()))
    }

    /// Saves several apps' settings from the setup or settings dialog and sends them to the phone.
    ///
    /// It answers after the phone has answered, with how many apps' settings reached it.
    pub async fn apply_preferences(
        &self,
        change: NotificationPreferencesChange,
    ) -> Result<NotificationPreferencesResult, FeatureApiError> {
        let r = self.feature()?.apply_preferences(change.into()).await?;
        Ok(NotificationPreferencesResult { succeeded: count(r.succeeded), failed: count(r.failed) })
    }

    /// "Show notifications from my phone".
    pub fn set_pc_permission(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_pc_permission(on))
    }

    /// Mutes the banners of the app `package_name` for `duration` (this Mac only).
    pub fn mute_app(&self, package_name: String, duration: NotificationMuteDuration) -> Result<(), FeatureApiError> {
        self.run(|f| f.mute_app(&package_name, duration.into()))
    }

    /// Ends a mute of the app `package_name`'s banners (this Mac only).
    pub fn unmute_app(&self, package_name: String) -> Result<(), FeatureApiError> {
        self.run(|f| f.unmute_app(&package_name))
    }

    /// Mutes every banner for 24 hours, or ends that.
    pub fn set_mute_all(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_mute_all(on))
    }

    /// "Pause apps that send too many notifications".
    pub fn set_throttling_enabled(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_throttling_enabled(on))
    }

    /// Asks the phone for notification access.
    pub async fn request_access(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_access().await?.into())
    }

    /// Asks the phone for access to sensitive notification content.
    pub async fn request_sensitive_access(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_sensitive_access().await?.into())
    }
}

#[cfg(test)]
mod tests;
