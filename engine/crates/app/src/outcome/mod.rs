//! What a feature sees of an App request: the mapping of a response or failure to an outcome, which
//! every feature uses.

use phonelink_core::seams::{ConnectionFailure, PlatformRequestStatus};
use phonelink_core::types::{DcgSendStatus, HubSendResult, OpenStatus, WakeStatus};
use phonelink_proto::platform_sdk::v1::PlatformDisconnectRequestReason;

use super::consts::result_code as rc;

/// The outcome of one App request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppOutcome {
    /// `result` 0, 10, 11, 16; or `3` with equal contract majors.
    Success,
    /// `result` 1; or a response whose payload could not be read (not a `ValueSet`).
    Failure,
    /// `result` 3 and the phone's contract major is lower than the PC's.
    PhoneContractIncompatible,
    /// `result` 3 and the phone's contract major is higher than the PC's.
    PcContractIncompatible,
    Accepted,
    Denied,
    PermanentlyDenied,
    NeedsPermission,
    NeedsInitialPermission,
    MeteredConnection,
    MeteredSyncDisabled,
    AllFeaturesDisabled,
    InvalidRequest,
    DatabaseFailure,
    DeviceNotSupported,
    AppLaunched,
    TwoPhoneMode,
    NonDefaultProfile,
    /// `result` 2, 17, 19, 22, missing, any other; every platform rejection.
    Unknown,
    /// The send, the response wait or the connection wait timed out.
    TimedOut,
    /// No network.
    InternetError,
    /// Canceled — by a local gate (nothing sent; [`super::AppResponse::local_gate`] says which) or
    /// by the caller.
    Canceled,
    /// The connection to the phone reported a failure while the request waited for it.
    ConnectionFailed(ConnectionFailure),
    /// The fragment layer could not deliver the request; the cause is kept.
    SendFailed(DcgSendStatus),
    /// A `/DeviceResourceManager` request sent through this layer ([`super::DrmAppRequest`]) was
    /// delivered but did not succeed.
    ResourceFailed(ResourceFailure),
}

/// Why a `/DeviceResourceManager` request sent through the App layer failed after it was delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceFailure {
    /// The phone answered `RESOURCE_HANDLER_NOT_REGISTERED`, rejected the route as not subscribed, or
    /// answered with a body that is not a `DeviceResourceManagerResponseMessage`.
    PayloadError,
    /// Rejected `AllSyncDisabled`: Link to Windows is turned off on the phone.
    RemoteAllSyncDisabled,
    /// Rejected `MeteredSyncDisabled`.
    RemoteMeteredSyncDisabled,
    /// Rejected `SyncNotAllowed`.
    RemoteSyncNotAllowed,
    /// Rejected `VersionTooLow` or `VersionTooHigh`.
    NotAllowed,
}

impl AppOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, AppOutcome::Success)
    }

    /// The text of what failed, for a send whose hub open or wake failed with one.
    pub fn cause(&self) -> Option<&str> {
        match self {
            AppOutcome::SendFailed(status) => status.cause(),
            _ => None,
        }
    }

    /// The send or the connection failed with *LTW disabled* — a wake that failed with LTW
    /// disabled, a request that waited for a connection the phone had ended with reason
    /// `ALL_SYNC_DISABLED`, or a `/DeviceResourceManager` request the phone rejected
    /// `AllSyncDisabled` (feeds the "all features disabled" flag).
    pub fn is_ltw_disabled(&self) -> bool {
        matches!(
            self,
            AppOutcome::ConnectionFailed(ConnectionFailure::Wake(WakeStatus::LtwDisabled))
                | AppOutcome::SendFailed(DcgSendStatus::WakeFailed { status: WakeStatus::LtwDisabled, .. })
                | AppOutcome::ResourceFailed(ResourceFailure::RemoteAllSyncDisabled)
        ) || matches!(self, AppOutcome::ConnectionFailed(ConnectionFailure::SessionEnded { reason })
            if *reason == PlatformDisconnectRequestReason::AllSyncDisabled as i32)
    }

    /// *Metered sync disabled*: `result` 12, a request that waited for a connection the phone had
    /// ended with reason `METERED_SYNC_DISABLED`, or a `/DeviceResourceManager` request the phone
    /// rejected `MeteredSyncDisabled`.
    pub fn is_metered_sync_disabled(&self) -> bool {
        matches!(
            self,
            AppOutcome::MeteredSyncDisabled | AppOutcome::ResourceFailed(ResourceFailure::RemoteMeteredSyncDisabled)
        ) || matches!(self, AppOutcome::ConnectionFailed(ConnectionFailure::SessionEnded { reason })
                if *reason == PlatformDisconnectRequestReason::MeteredSyncDisabled as i32)
    }
}

/// Maps a response's `result` code. `pc_contract` is the PC's contract version (`3.99`);
/// `response_contract` the response's (`None` reads `0`). Only the integer parts are compared.
pub fn outcome_for_result(result: i32, pc_contract: f64, response_contract: Option<f64>) -> AppOutcome {
    match result {
        rc::SUCCESS | rc::CANCELED_DISCONNECTED | rc::CANCELED_COALESCED | rc::FEATURE_FLAG_DISABLED => {
            AppOutcome::Success
        }
        rc::FAILURE => AppOutcome::Failure,
        rc::INCOMPATIBLE_VERSIONS => {
            let pc = pc_contract.trunc();
            let phone = response_contract.unwrap_or(0.0).trunc();
            if phone == pc {
                AppOutcome::Success
            } else if phone < pc {
                AppOutcome::PhoneContractIncompatible
            } else {
                AppOutcome::PcContractIncompatible
            }
        }
        rc::USER_ACCEPTED => AppOutcome::Accepted,
        rc::USER_DENIED => AppOutcome::Denied,
        rc::USER_PERMANENTLY_DENIED => AppOutcome::PermanentlyDenied,
        rc::NEEDS_PERMISSION => AppOutcome::NeedsPermission,
        rc::NEEDS_INITIAL_PERMISSION => AppOutcome::NeedsInitialPermission,
        rc::METERED_CONNECTION => AppOutcome::MeteredConnection,
        rc::METERED_CONNECTION_SYNC_DISABLED => AppOutcome::MeteredSyncDisabled,
        rc::ALL_FEATURES_DISABLED => AppOutcome::AllFeaturesDisabled,
        rc::INVALID_REQUEST => AppOutcome::InvalidRequest,
        rc::DATABASE_OPERATION_FAILED => AppOutcome::DatabaseFailure,
        rc::DEVICE_NOT_SUPPORTED => AppOutcome::DeviceNotSupported,
        rc::APP_LAUNCHED => AppOutcome::AppLaunched,
        rc::TWO_PHONE_MODE_SYNC_DISABLED => AppOutcome::TwoPhoneMode,
        rc::NON_DEFAULT_USER_PROFILE => AppOutcome::NonDefaultProfile,
        _ => AppOutcome::Unknown,
    }
}

/// A platform status other than `Success`: rejections are reported as *unknown* (the reason is not
/// passed on); send failures by cause.
pub fn outcome_for_platform_status(status: &PlatformRequestStatus) -> AppOutcome {
    match status {
        PlatformRequestStatus::Success => AppOutcome::Unknown, // a success without a message
        PlatformRequestStatus::Canceled => AppOutcome::Canceled,
        PlatformRequestStatus::ResponseTimedOut => AppOutcome::TimedOut,
        PlatformRequestStatus::RequestSendFailed(cause) => outcome_for_send_failure(cause),
        PlatformRequestStatus::NoResponse
        | PlatformRequestStatus::RemoteSyncNotAllowed
        | PlatformRequestStatus::RemoteVersionTooLow
        | PlatformRequestStatus::RemoteVersionTooHigh
        | PlatformRequestStatus::RemoteAllSyncDisabled
        | PlatformRequestStatus::RemoteMeteredSyncDisabled
        | PlatformRequestStatus::RemoteNotSubscribedToRoute => AppOutcome::Unknown,
    }
}

/// *send failed → by cause*: no network → internet error; no ack → timed out.
pub fn outcome_for_send_failure(cause: &DcgSendStatus) -> AppOutcome {
    match cause {
        DcgSendStatus::HubSendFailed { result: HubSendResult::InternetError, .. }
        | DcgSendStatus::WakeFailed {
            status: WakeStatus::InternetError | WakeStatus::TransportInternetError, ..
        } => AppOutcome::InternetError,
        DcgSendStatus::TimedOut => AppOutcome::TimedOut,
        DcgSendStatus::Canceled => AppOutcome::Canceled,
        other => AppOutcome::SendFailed(other.clone()),
    }
}

/// A connection failure reported while the request waited for the connection.
pub fn outcome_for_connection_failure(failure: ConnectionFailure) -> AppOutcome {
    match failure {
        ConnectionFailure::Hub(OpenStatus::InternetError)
        | ConnectionFailure::Wake(WakeStatus::InternetError | WakeStatus::TransportInternetError) => {
            AppOutcome::InternetError
        }
        other => AppOutcome::ConnectionFailed(other),
    }
}

#[cfg(test)]
mod tests;
