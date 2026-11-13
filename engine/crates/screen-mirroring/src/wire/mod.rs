//! The scopes and JSON bodies of the mirroring workflows: what the PC writes, byte for byte where the
//! Phone Link on Windows writes them so, and what it reads from the phone.
//!
//! Every body is UTF-8 JSON; the phone ignores whitespace and keys it does not read.

mod from_phone;
mod object_writer;
mod to_phone;

#[cfg(feature = "test-support")]
pub use from_phone::NotificationResult;
pub use from_phone::{
    Body, ClipData, DisconnectingReason, LockedState, ParseError, parse_clip_data, parse_disconnecting,
    parse_drag_cancel, parse_drag_complete, parse_drag_request_item, parse_drag_start_phone, parse_locked_state,
    parse_notification_result, parse_stop_workflow,
};
use object_writer::write_string;
pub use object_writer::{Escaping, Obj};
#[cfg(feature = "test-support")]
pub use to_phone::LayoutDisplay;
pub use to_phone::{
    ClipItem, ConfigValues, FocusableState, InputSourceKind, KEYBOARD_SETTINGS_INTENT, always_expanded,
    app_focusable_state, app_resumable, audio_pc_streaming_allowed, audio_set_volume, black_screen, config,
    disconnecting_result, drag_cancel, drag_drop_pc, drag_request_item, drag_request_ownership, drag_start_pc,
    empty_nearby, empty_orchestrator, launch_app, launch_custom_intent, launch_notification_v1, launch_notification_v2,
    launch_phone_screen_app, nearby_request_setting, nearby_send_pin_code, permission_transaction,
    request_move_to_vd_result, request_permission, request_screen_layout, request_session_result, workflow_body,
};

/// The package name of the phone-screen window.
pub const PHONE_SCREEN_PACKAGE: &str = "yourphone.phonescreen";

/// Workflow names (`/workflow` `workflow`).
pub mod workflow {
    pub const LAUNCHER: &str = "launcher";
    pub const PERMISSION: &str = "permission";
    pub const REMOTE: &str = "remote";
    pub const AUDIO: &str = "audio";
    pub const NEARBY: &str = "nearby";
    pub const EARLY_LAUNCH: &str = "earlyLaunch";
}

/// Channel property keys of the phone's announcements.
pub mod property {
    pub const WORKFLOW: &str = "workflow";
    pub const TYPE: &str = "type";
    pub const ORCHESTRATOR: &str = "orchestrator";
    pub const PACKAGE_NAME: &str = "packageName";
    pub const SESSION_ID: &str = "sessionId";
}

pub mod scope {
    pub const WORKFLOW: &str = "/workflow";
    pub const STOP_WORKFLOW: &str = "/stopWorkflow";

    pub const REQUEST_PERMISSION: &str = "/permission/requestPermission";
    pub const PERMISSION_STATUS: &str = "/permission/permissionStatus";
    pub const CHECK_PERMISSION: &str = "/permission/checkPermission";
    pub const SHOW_PERMISSION: &str = "/permission/showPermission";
    pub const LAUNCH_SETTINGS: &str = "/permission/launchSettings";
    pub const CHECK_PERMISSION_RESULT: &str = "/permission/checkPermissionResult";
    pub const SHOW_PERMISSION_RESULT: &str = "/permission/showPermissionResult";
    pub const LAUNCH_SETTINGS_RESULT: &str = "/permission/launchSettingsResult";

    pub const CONFIG: &str = "/mirror/phone/config";
    pub const ALWAYS_EXPANDED: &str = "/mirror/phone/alwaysInExpandedLayoutChanged";
    pub const LAUNCH_APP: &str = "/mirror/phone/launchApp";
    pub const LAUNCH_PHONE_SCREEN_APP: &str = "/mirror/phone/launchPhoneScreenApp";
    pub const APP_LAUNCH_RESULT: &str = "/mirror/phone/appLaunchResult";
    pub const REQUEST_SESSION: &str = "/mirror/phone/requestSession";
    pub const REQUEST_PHONE_SCREEN_SESSION: &str = "/mirror/phone/requestPhoneScreenSession";
    pub const REQUEST_SESSION_RESULT: &str = "/mirror/phone/requestSessionResult";
    pub const SET_LOCKED_STATE: &str = "/mirror/phone/setLockedState";
    pub const WAKE_SCREEN: &str = "/mirror/phone/wakeScreen";
    pub const REQUEST_BLACK_SCREEN_STATE: &str = "/mirror/phone/requestBlackScreenState";
    pub const BLACK_SCREEN: &str = "/mirror/phone/blackScreen";
    pub const APP_RESUMABLE: &str = "/mirror/phone/appResumable";
    pub const APP_RESUMABLE_RESULT: &str = "/mirror/phone/appResumableResult";
    pub const LAUNCH_NOTIFICATION: &str = "/mirror/phone/launchNotification";
    pub const LAUNCH_NOTIFICATION_RESULT: &str = "/mirror/phone/launchNotificationResult";
    pub const LAUNCH_CUSTOM_INTENT: &str = "/mirror/phone/launchCustomIntent";
    pub const ACCESSIBILITY_SETTINGS: &str = "/mirror/phone/accessibilitySettings";

    pub const APP_READY: &str = "/mirror/phone/appReady";
    pub const DEVICE_RESOLUTION_CHANGED: &str = "/mirror/phone/deviceResolutionChanged";
    pub const ORIENTATION_CHANGED: &str = "/mirror/phone/orientationChanged";
    pub const POSTURE: &str = "/mirror/phone/posture";
    pub const NAVIGATION_MODE: &str = "/mirror/phone/navigationMode";
    pub const SECURE_CONTENT_CHANGED: &str = "/mirror/phone/secureContentChanged";
    pub const SCREEN_LAYOUT_STATUS: &str = "/mirror/phone/screenLayoutStatus";
    pub const REQUEST_SCREEN_LAYOUT: &str = "/mirror/phone/requestScreenLayout";
    pub const BRING_TO_FOREGROUND: &str = "/mirror/phone/bringToForeground";
    pub const REQUEST_MOVE_TO_VD: &str = "/mirror/phone/requestMoveToVD";
    pub const REQUEST_MOVE_TO_VD_RESULT: &str = "/mirror/phone/requestMoveToVDResult";
    pub const DISCONNECTING: &str = "/mirror/phone/disconnecting";
    pub const DISCONNECTING_RESULT: &str = "/mirror/phone/disconnectingResult";
    pub const APP_FOCUSABLE_STATE: &str = "/mirror/phone/appFocusableState";
    pub const ACCESSIBILITY_EVENT: &str = "/mirror/phone/accessibilityEvent";

    pub const DRAG_START_PC: &str = "/drag/startPc";
    pub const DRAG_DROP_PC: &str = "/drag/dropPc";
    pub const DRAG_REQUEST_ITEM: &str = "/drag/requestItem";
    pub const DRAG_REQUEST_OWNERSHIP: &str = "/drag/requestOwnership";
    pub const DRAG_CANCEL: &str = "/drag/cancel";
    pub const DRAG_START_PHONE: &str = "/drag/startPhone";
    pub const DRAG_CLIP_DATA: &str = "/drag/clipData";
    pub const DRAG_COMPLETE: &str = "/drag/complete";

    pub const AUDIO_PHONE_CONFIG: &str = "/audio/phoneConfig";
    pub const AUDIO_PC_STREAMING_ALLOWED: &str = "/audio/pcStreamingAllowed";
    pub const AUDIO_STREAM_READY: &str = "/audio/streamReady";
    pub const AUDIO_SET_VOLUME: &str = "/audio/setVolume";
    pub const AUDIO_VOLUME_CHANGED: &str = "/audio/volumeChanged";
    pub const AUDIO_SILENCE_STATE_CHANGED: &str = "/audio/silenceStateChanged";
    pub const AUDIO_CAPTURE_ALLOWED: &str = "/audio/captureAllowed";

    pub const NEARBY_REQUEST_SETTING: &str = "/nearby/requestSetting";
    pub const NEARBY_REQUEST_PIN_CODE: &str = "/nearby/requestPinCode";
    pub const NEARBY_SEND_PIN_CODE: &str = "/nearby/sendPinCode";
    pub const NEARBY_SKIP_UNLOCK: &str = "/nearby/skipUnlockNotification";
    pub const NEARBY_SKIP_VERIFICATION: &str = "/nearby/skipVerificationNotification";
    pub const NEARBY_REQUEST_SETTING_RESULT: &str = "/nearby/requestSettingResult";
    pub const NEARBY_DEVICE_LOCK: &str = "/nearby/deviceLock";
    pub const NEARBY_VERIFICATION_RESULT: &str = "/nearby/verificationResult";
    pub const NEARBY_WRONG_PIN_CODE: &str = "/nearby/wrongPinCode";
    pub const NEARBY_SKIP_UNLOCK_RESULT: &str = "/nearby/skipUnlockResult";
    pub const NEARBY_SKIP_VERIFICATION_RESULT: &str = "/nearby/skipVerificationResult";
}

/// The orchestrator scopes the PC handles.
pub const ORCHESTRATOR_INBOUND: &[&str] = &[
    scope::APP_LAUNCH_RESULT,
    scope::REQUEST_SESSION,
    scope::REQUEST_PHONE_SCREEN_SESSION,
    scope::SET_LOCKED_STATE,
    scope::REQUEST_BLACK_SCREEN_STATE,
    scope::APP_RESUMABLE_RESULT,
    scope::LAUNCH_NOTIFICATION_RESULT,
    scope::ACCESSIBILITY_SETTINGS,
];

/// The early-launch channel's inbound scopes.
pub const EARLY_LAUNCH_INBOUND: &[&str] =
    &[scope::REQUEST_SESSION, scope::REQUEST_PHONE_SCREEN_SESSION, scope::LAUNCH_NOTIFICATION_RESULT];

pub const PERMISSION_INBOUND: &[&str] = &[
    scope::PERMISSION_STATUS,
    scope::CHECK_PERMISSION_RESULT,
    scope::SHOW_PERMISSION_RESULT,
    scope::LAUNCH_SETTINGS_RESULT,
];

pub const PER_APP_INBOUND: &[&str] = &[
    scope::APP_READY,
    scope::DEVICE_RESOLUTION_CHANGED,
    scope::ORIENTATION_CHANGED,
    scope::POSTURE,
    scope::NAVIGATION_MODE,
    scope::SECURE_CONTENT_CHANGED,
    scope::SCREEN_LAYOUT_STATUS,
    scope::BRING_TO_FOREGROUND,
    scope::REQUEST_MOVE_TO_VD,
    scope::DISCONNECTING,
    scope::ACCESSIBILITY_EVENT,
    scope::DRAG_REQUEST_ITEM,
    scope::DRAG_CANCEL,
    scope::DRAG_START_PHONE,
    scope::DRAG_CLIP_DATA,
    scope::DRAG_COMPLETE,
];

pub const AUDIO_INBOUND: &[&str] = &[
    scope::AUDIO_PHONE_CONFIG,
    scope::AUDIO_STREAM_READY,
    scope::AUDIO_VOLUME_CHANGED,
    scope::AUDIO_SILENCE_STATE_CHANGED,
    scope::AUDIO_CAPTURE_ALLOWED,
];

pub const NEARBY_INBOUND: &[&str] = &[
    scope::NEARBY_REQUEST_SETTING_RESULT,
    scope::NEARBY_DEVICE_LOCK,
    scope::NEARBY_VERIFICATION_RESULT,
    scope::NEARBY_WRONG_PIN_CODE,
    scope::NEARBY_SKIP_UNLOCK_RESULT,
    scope::NEARBY_SKIP_VERIFICATION_RESULT,
];

/// `launchApp` flags.
pub mod launch_flags {
    pub const BYPASS_ANDROID_LAUNCH_CHECKS: i64 = 1;
    pub const IS_PHONE_SCREEN: i64 = 2;
    pub const IS_TASK_TRANSFER_LAUNCH: i64 = 4;
}

/// `ScreenLayout`.
pub mod screen_layout {
    pub const DEFAULT: i32 = 0;
    pub const COMPACT: i32 = 1;
    pub const EXPANDED: i32 = 2;
}

#[cfg(test)]
mod tests;
