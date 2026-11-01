//! What the feature shows and takes: its status, the notification and app-setting views, and the preference changes.

use super::{AppFlags, ConversationLine, ItemAction, OpenTarget};
use phonelink_core::types::DcgClient;
use std::time::Duration;

/// The feature's state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsStatus {
    pub phone: DcgClient,
    /// The phone's capability bits include `NotificationsListener` (`0x40`): the feature exists.
    pub capable: bool,
    /// Also `PhoneApps` (`0x8`): settings trigger 25 and per-app flags and mutes in the banner decision.
    pub app_list_supported: bool,
    /// "Show my phone's notifications" (default on).
    pub pc_permission: bool,
    /// The phone's notification-listener access (default off).
    pub device_permission: bool,
    /// The phone's sensitive-notification access (default off).
    pub sensitive_permission: bool,
    /// `NotificationsCapabilities` from the last connect.
    pub capabilities: i32,
    /// The sensitive-notification prompt is offered (`NotificationsCapabilities` has `8` and `4`).
    pub sensitive_prompt_offered: bool,
    /// A full sync is in flight.
    pub syncing: bool,
    pub last_updated_ms: Option<i64>,
    /// Mute-all is on until this time (Unix ms).
    pub muted_all_until_ms: Option<i64>,
    pub throttling_enabled: bool,
    /// A notification-permission request was answered *`DeviceNotSupported`*.
    pub low_ram_device: bool,
    /// A notification-permission request was answered *Success* or *Accepted*.
    pub setup_completed: bool,
    /// The first setup stage applies: Link to Windows on the phone needs the post-notification
    /// permission ([`post_notification_needed`]).
    pub post_notification_needed: bool,
}

/// One item of the notification list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationView {
    pub key: String,
    pub package_name: String,
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub body: String,
    pub conversation: Vec<ConversationLine>,
    /// `timestamp` when set, else `postTime` (Unix ms).
    pub time_ms: i64,
    pub pinned: bool,
    pub open_target: OpenTarget,
    /// `featureFlags` bit 2.
    pub launch_supported: bool,
    pub is_read_only_group_message: bool,
    /// The reply box: the first reply action.
    pub reply: Option<ItemAction>,
    /// Up to four buttons.
    pub actions: Vec<ItemAction>,
}

/// An app's notification setting as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSettingView {
    pub package_name: String,
    pub flags: AppFlags,
    /// The app is muted until this time (Unix ms).
    pub muted_until_ms: Option<i64>,
}

/// Per-app mute durations (a local-only setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuteDuration {
    OneHour,
    EightHours,
    OneDay,
}

impl MuteDuration {
    pub(super) fn duration(self) -> Duration {
        Duration::from_secs(match self {
            MuteDuration::OneHour => 3600,
            MuteDuration::EightHours => 8 * 3600,
            MuteDuration::OneDay => 24 * 3600,
        })
    }
}

/// Where a multi-app preferences change comes from (its trigger name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferencesSource {
    /// The preferences dialog of the settings view.
    SettingsDialog,
    /// The setup flow; its first save does not sync the apps that match the default.
    Setup,
}

/// A multi-app change of the setup / preferences dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferencesChange {
    pub source: PreferencesSource,
    /// Apps whose setting differs from the default.
    pub overrides: Vec<(String, AppFlags)>,
    /// Apps that follow the default.
    pub default_matching: Vec<String>,
    /// The default the user chose, when they chose one in this change.
    pub new_default: Option<AppFlags>,
}

/// How many apps of a preferences change reached the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreferencesResult {
    pub succeeded: usize,
    pub failed: usize,
}
