//! Mirroring's status codes and what an error window shows for each.

use phonelink_nano::connect::AttemptError;
use phonelink_nano::connection::CloseReason;

/// Status codes.
pub mod code {
    pub const GENERIC: i32 = 0;
    pub const LAUNCH_FAILED: i32 = 32;
    pub const OVERHEATED: i32 = 33;
    pub const APP_DOES_NOT_EXIST: i32 = 34;
    pub const PC_PERMISSION: i32 = 35;
    pub const LAUNCH_TIMEOUT: i32 = 37;
    pub const POWER_SAVING_MODE: i32 = 38;
    pub const APP_UNAVAILABLE: i32 = 39;
    pub const MESSAGE_CHANNEL_FAILED: i32 = 48;
    pub const MESSAGE_CHANNEL_TIMED_OUT: i32 = 49;
    pub const DISCONNECTED_UNKNOWN: i32 = 80;
    pub const LOST_CONNECTION: i32 = 81;
    pub const DEVICE_LOCKED: i32 = 82;
    pub const DEVICE_SWITCHED: i32 = 83;
    pub const WINDOWS_CLOSED: i32 = 84;
    pub const PEER_ENDED: i32 = 85;
    pub const EXPIRED: i32 = 86;
    pub const METERED_ALTERNATE: i32 = 96;
    pub const CONNECTION_FAILED: i32 = 256;
    pub const CONNECTION_TIMED_OUT: i32 = 257;
    pub const DIRECT_CONNECTION_TIMED_OUT: i32 = 267;
    pub const CONNECTION_CANCELED: i32 = 268;
    pub const DIRECT_CONNECTION_UNKNOWN: i32 = 270;
    pub const HOSTNAME_RESOLUTION: i32 = 273;
    pub const TRANSPORT_NONE: i32 = 274;
    pub const SEND_TIMED_OUT: i32 = 603;
    pub const DEVICE_UNREACHABLE: i32 = 606;
    pub const NETWORK_UNAVAILABLE: i32 = 608;
    pub const WEB_SERVICE_ERROR: i32 = 609;
    pub const METERED_CONNECTION: i32 = 808;
    pub const ALL_FEATURES_DISABLED: i32 = 813;
    pub const INVALID_REQUEST: i32 = 814;
}

/// What an error window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    AppNotAvailable,
    Overheated,
    PowerSaving,
    LaunchFailed,
    PermissionOff,
    Disconnected,
    TimedOut,
    CannotConnectDirectly,
    PhoneUnreachable,
    MeteredConnection,
    FeaturesDisabled,
    UpdateNeeded,
    ServiceIssue,
    Generic,
}

/// The button an error window offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorAction {
    /// Close the window.
    Close,
    /// Re-run the set-up for every window.
    Retry,
    /// Re-run the set-up for every window ("Connect").
    Connect,
    /// Turn "allow apps" on and retry.
    AllowAndRetry,
    /// Launch the window's app again (`launchApp` for this window).
    Relaunch,
}

pub fn error_kind(code: i32) -> (ErrorKind, ErrorAction) {
    use ErrorAction as A;
    use ErrorKind as K;
    match code {
        code::APP_DOES_NOT_EXIST | code::APP_UNAVAILABLE => (K::AppNotAvailable, A::Close),
        code::OVERHEATED => (K::Overheated, A::Close),
        code::POWER_SAVING_MODE => (K::PowerSaving, A::Relaunch),
        code::LAUNCH_FAILED => (K::LaunchFailed, A::Retry),
        code::PC_PERMISSION => (K::PermissionOff, A::AllowAndRetry),
        80 | 81 | 82 | 85 => (K::Disconnected, A::Connect),
        257 | 603 => (K::TimedOut, A::Connect),
        267 | 608 => (K::CannotConnectDirectly, A::Retry),
        606 => (K::PhoneUnreachable, A::Connect),
        808 | 96 => (K::MeteredConnection, A::Retry),
        813 => (K::FeaturesDisabled, A::Connect),
        814 => (K::UpdateNeeded, A::Connect),
        609 | 611 | 617 | 619 | 620 => (K::ServiceIssue, A::Retry),
        _ => (K::Generic, A::Retry),
    }
}

/// The status of an `appLaunchResult` that is not `success`, `trueTouchWarning` or
/// `duplicateLaunch`.
pub fn launch_result_code(result: &str) -> i32 {
    match result {
        "appNotExist" => code::APP_DOES_NOT_EXIST,
        "overheated" => code::OVERHEATED,
        "powerSavingMode" => code::POWER_SAVING_MODE,
        "appUnavailable" => code::APP_UNAVAILABLE,
        _ => code::LAUNCH_FAILED,
    }
}

/// Why the mirroring connection did not open: the status code the windows show, and what failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the mirroring connection did not open (status {code}): {cause}")]
pub struct ConnectFailure {
    pub code: i32,
    pub cause: String,
}

/// The status a failed connection attempt ends the start with.
pub fn attempt_status(e: &AttemptError) -> i32 {
    match e {
        AttemptError::Rejected { result, .. } => 800 + result,
        AttemptError::Transport { status } => *status,
        AttemptError::Connection(reason) => match reason {
            CloseReason::TimeoutExceeded => code::DIRECT_CONNECTION_TIMED_OUT,
            CloseReason::UserCanceled => code::CONNECTION_CANCELED,
            CloseReason::None => code::TRANSPORT_NONE,
            CloseReason::Unknown(_) => code::DIRECT_CONNECTION_UNKNOWN,
            CloseReason::HostnameResolutionError => code::HOSTNAME_RESOLUTION,
            _ => code::CONNECTION_FAILED,
        },
        AttemptError::Timeout => code::CONNECTION_TIMED_OUT,
        _ => code::CONNECTION_FAILED,
    }
}

/// A set-up failure's code as the windows show it: 808 is 96 for a phone without ICE support.
pub fn setup_code(code: i32, ice_support: bool) -> i32 {
    if code == code::METERED_CONNECTION && !ice_support { code::METERED_ALTERNATE } else { code }
}

/// The disconnection code of a connection that closed with `reason`; `requested` is the code the PC
/// closed it with, `mac_locked` whether the Mac's session is locked.
pub fn close_code(reason: &CloseReason, requested: Option<i32>, mac_locked: bool) -> i32 {
    match reason {
        CloseReason::UserEnded if requested.is_some() => requested.expect("checked"),
        CloseReason::KeepAliveTimeout => code::LOST_CONNECTION,
        CloseReason::PeerEnded if mac_locked => code::DEVICE_LOCKED,
        CloseReason::PeerEnded => code::PEER_ENDED,
        _ => code::DISCONNECTED_UNKNOWN,
    }
}

#[cfg(test)]
mod tests;
