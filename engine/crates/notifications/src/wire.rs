//! Vocabulary of the notifications feature: routes, `ValueSet` keys, operation codes, settings
//! identifiers, capability flags and the trace-state names the app sends.

/// Routes of the PC's requests (notifications and per-app notification settings).
pub mod route {
    pub const NOTIFICATIONS: &str = "/notifications";
    pub const SETTINGS: &str = "/settings";
}

/// The `contentType` of notification batches and of `/notifications` requests.
pub const CONTENT_TYPE: &str = "notifications";
/// The `contentType` of `/settings` requests (no PC processor).
pub const SETTINGS_CONTENT_TYPE: &str = "settings";

/// `ValueSet` keys of this feature.
pub mod key {
    // Inbound batches (content message, batch announcement, reply).
    pub const NOTIFICATION_KEYS: &str = "notificationKeys";
    pub const OPERATIONS: &str = "operations";
    pub const NOTIFICATIONS: &str = "notifications";
    pub const ITEM_COUNT: &str = "itemCount";
    pub const MESSAGE: &str = "message";
    // `/notifications` request keys.
    pub const OPERATION: &str = "operation";
    pub const KEY: &str = "key";
    pub const ACTION_INDEX: &str = "actionIndex";
    pub const INLINE_REPLY_MESSAGE: &str = "inlineReplyMessage";
    pub const NOTIFICATION_KEYS_COUNT: &str = "notificationKeysCount";
    pub const POST_TIMES: &str = "postTimes";
    /// Sent `true` with the connect's reconcile keys: the phone may answer whether it finished
    /// reconciling them before replying.
    pub const SUPPORTS_NOTIFICATION_SYNC_COMPLETION: &str = "supportsNotificationSyncCompletion";
    /// Connect response: `true` when the phone finished reconciling the connect's keys.
    pub const NOTIFICATION_SYNC_COMPLETED: &str = "notificationSyncCompleted";
    // `/settings` requests and responses (per-app notification settings).
    pub const TRIGGER_TYPE: &str = "triggerType";
    pub const SETTING_KEY: &str = "settingKey";
    pub const SETTING_VALUE: &str = "settingValue";
    pub const SETTING_TYPE: &str = "settingType";
    pub const SETTING_GROUP_ID: &str = "settingGroupId";
    pub const SETTING_MUTE_EXPIRATION_TIMESTAMP: &str = "settingMuteExpirationTimestamp";
    pub const SETTINGS: &str = "settings";
    pub const SAVED_SETTING: &str = "savedSetting";
    pub const SYNC_SETTING: &str = "syncSetting";
    // The connect message.
    pub const AUDIO_INFO_SYNC_ENABLED: &str = "AudioInfoSyncEnabled";
    pub const PERMISSIONS: &str = "permissions";
    pub const CAPABILITIES: &str = "capabilities";
    /// `permissions.notifications` and `capabilities.notifications`.
    pub const NOTIFICATIONS_ENTRY: &str = "notifications";
}

/// `PhoneNotificationOperationType` (operation codes — complete).
pub mod operation {
    pub const UNKNOWN: i32 = 0;
    pub const NEW_NOTIFICATION: i32 = 1;
    pub const REMOVE_NOTIFICATION: i32 = 2;
    pub const EXISTING_NOTIFICATION: i32 = 3;
    pub const CLEAR_ALL_NOTIFICATIONS: i32 = 4;
    pub const LAUNCH_NOTIFICATION: i32 = 5;
    pub const MEDIA_COMMAND: i32 = 6;
    pub const SESSION_CHANGE: i32 = 7;
}

/// `SettingsRequestTriggerType`; `0` Unknown is never sent.
pub mod trigger_type {
    pub const SYNC_SETTINGS: i32 = 25;
    pub const SETTING_CHANGED: i32 = 26;
    pub const BATCH_SETTINGS_CHANGED: i32 = 27;
}

/// Settings identifiers (per-app notification settings).
pub mod setting {
    /// The phone-side "sync this app" switch.
    pub const SYNC_GROUP: &str = "NotificationSyncSetting";
    /// PC-only banner flags and mutes.
    pub const LOCAL_GROUP: &str = "NotificationLocalSetting";
    /// `settingType` of a sync value (`PropertyType.Boolean`).
    pub const TYPE_BOOLEAN: i32 = 11;
    /// `settingType` of a flags value (`PropertyType.UInt32`).
    pub const TYPE_FLAGS: i32 = 5;
}

/// `NotificationsCapabilities` (from the connect message).
pub mod capability {
    /// iOS only: stored with the rest, never acted on (the ANCS paths are not implemented).
    pub const ANCS_CONNECTION_CAPABLE: i32 = 1;
    /// The phone reconciles from the `connect` keys.
    pub const ECR_HANDLES_NOTIFICATIONS: i32 = 2;
    pub const PROFILE_COMPUTER_ROLE_AVAILABLE: i32 = 4;
    pub const SENSITIVE_NOTIFICATIONS_ENABLED: i32 = 8;
}

/// `deviceCapabilityNegotiation` bit `BatchNotificationSettings` (settings trigger 27).
pub const BATCH_NOTIFICATION_SETTINGS: i32 = 8;

/// The `ypTriggerId` / `ypScenarioId` names the app sends (the settings triggers are Phone Link's
/// names).
pub mod names {
    pub const SCENARIO_NOTIFICATIONS: &str = "Notifications";
    pub const TRIGGER_DISMISS: &str = "NotificationsDismiss";
    pub const TRIGGER_CLEAR_ALL: &str = "NotificationsClearAll";
    pub const SCENARIO_CONNECT_FULL_SYNC: &str = "ConnectToDeviceNotifications";
    pub const SCENARIO_FULL_SYNC: &str = "SyncAllNotifications";
    pub const TRIGGER_PERMISSIONS_CHANGED: &str = "NotificationsPermissionsChanged";
    pub const TRIGGER_SENSITIVE_PERMISSIONS_CHANGED: &str = "NotificationsSensitivePermissionsChanged";
    pub const TRIGGER_FEED_ACTION: &str = "NotificationsFeedAction";
    pub const TRIGGER_FEED_INLINE_REPLY: &str = "NotificationsFeedInlineReply";
    pub const TRIGGER_TOAST_INLINE_REPLY: &str = "NotificationsToastInlineReply";
    pub const TRIGGER_PERMISSION_REQUIRED: &str = "PermissionRequired";
    pub const TRIGGER_CHECK_PERMISSION_STATUS: &str = "CheckPermissionStatus";
    pub const TRIGGER_SENSITIVE_PERMISSION: &str = "LaunchNotificationsSensitivePermission";
    pub const SCENARIO_SENSITIVE_PERMISSION: &str = "NotificationsSensitivePermission";
    pub const SCENARIO_SYNC_ALL_SETTINGS: &str = "SyncAllSettings";
    pub const SCENARIO_UPDATE_SETTING: &str = "UpdateSetting";
    pub const SCENARIO_BATCH_UPDATE_SETTINGS: &str = "BatchUpdateSettings";
    /// Settings trigger 25: the notification settings view opens.
    pub const TRIGGER_SETTINGS_VIEW_OPENED: &str = "InitializeNotificationsSettings";
    /// Settings trigger 26 from a list item's context-menu on/off toggle.
    pub const TRIGGER_CONTEXT_MENU_TOGGLE: &str = "NotificationSettingChangedFromContextMenu";
    /// Settings trigger 26 from the undo button of the "turned off for this app" notice.
    pub const TRIGGER_UNDO_TURN_OFF: &str = "UpdateNotificationsSettingsValue";
    /// Settings trigger 26 (and full sync (d)) from the settings list.
    pub const TRIGGER_SETTINGS_LIST: &str = "NotificationSettingChanged";
    /// Settings trigger 27 / 26s from the preferences dialog in the settings view.
    pub const TRIGGER_PREFERENCES_DIALOG: &str = "QbDDialogApply";
    /// Settings trigger 27 / 26s from the setup flow.
    pub const TRIGGER_PREFERENCES_SETUP: &str = "QbDBatchSave";
}

/// Values the queue record stores in its external id 2.
pub const INLINE_REPLY_EXTERNAL_ID: &str = "inline_reply";
