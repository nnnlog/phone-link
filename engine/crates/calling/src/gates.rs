//! The gates over the phone record that call history reads.
//!
//! | Gate | Rule |
//! |---|---|
//! | [`is_android`] | the phone's platform is Android (call history has no iOS path here) |
//! | [`calls_capable`] | capability bit `Calls` (`0x10`) |
//! | [`call_logs_capable`] | capability bit `CallLogs` (`0x100`) |
//! | [`call_log_permission`] | the phone's `CallLogs` permission (`0x100`), not held until reported |
//! | [`post_notification_needed`] | an Android 13+ phone holds neither its post-notification nor its auto-launch permission |
//!
//! Phone Link on Windows also needs its OS to offer Bluetooth calling before it shows or stores call
//! history; this Mac has no such requirement.

use phonelink_app::PhoneRecord;
use phonelink_app::consts::{capability, permission, platform};

pub fn is_android(record: &PhoneRecord) -> bool {
    record.platform == platform::ANDROID
}

pub fn calls_capable(record: &PhoneRecord) -> bool {
    record.has_capability(capability::CALLS)
}

pub fn call_logs_capable(record: &PhoneRecord) -> bool {
    record.has_capability(capability::CALL_LOGS)
}

pub fn call_log_permission(record: &PhoneRecord) -> bool {
    record.permission_or(permission::CALL_LOGS, false)
}

/// The phone must allow Link to Windows to post notifications before it is asked for its call log.
///
/// Android 13 or later, its `PostNotifications` permission reported missing (unreported counts as
/// granted) and its `AutoLaunch` permission not held.
pub fn post_notification_needed(record: &PhoneRecord) -> bool {
    record.post_notification_ask_supported()
        && !record.permission_or(permission::POST_NOTIFICATIONS, true)
        && !record.permission_or(permission::AUTO_LAUNCH, false)
}

/// The calls view shows for this phone: Android, with the `Calls` capability.
pub fn calls_view_supported(record: &PhoneRecord) -> bool {
    is_android(record) && calls_capable(record)
}

/// The phone shares its call history: the calls view shows and the phone has `CallLogs`.
pub fn history_supported(record: &PhoneRecord) -> bool {
    calls_view_supported(record) && call_logs_capable(record)
}
