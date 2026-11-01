//! What the user sees of a pairing session.

use super::PairingFlow;
use std::time::SystemTime;

/// A code the PC shows (the QR code's link, a PIN) and when it stops working, on the PC's clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownCode {
    pub value: String,
    pub expires_at: SystemTime,
    /// The expiry as whole minutes from when the response arrived, rounded (at least 1).
    pub expires_in_minutes: u32,
}

/// Which PIN the PC shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
    /// The QR flow's `challengePIN`, shown once the QR code was scanned; it can be refreshed.
    Challenge,
    /// The manual flow's `pairingPIN`, typed into Link to Windows; it cannot be refreshed.
    Pairing,
}

/// Why the shown PIN no longer works.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinProblem {
    /// `CHALLENGE_PIN_EXPIRED`.
    Expired,
    /// `CHALLENGE_PIN_VALIDATE_FAILED`: the phone entered another PIN.
    Wrong,
}

/// Why a challenge-PIN refresh failed (the session waits for another).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshFailure {
    NetworkUnavailable,
    ForbiddenDcgRequest,
    UnauthorizedRequest,
    MsaUnauthorizedRequest,
    TimeoutError,
    InternalError,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingResult {
    /// `DEVICE_PAIRED`, and the phone is one of the account's linked phones now.
    Success {
        phone_id: String,
    },
    /// The QR code, the PIN or the session expired.
    PairingSessionExpired,
    /// `PAIRING_ACCOUNT_MISMATCHED`: the phone signed in with another Microsoft account.
    MsaTokenMismatched,
    /// The service answered a state the session does not accept there (`UnexpectedServiceState`).
    ServiceError,
    /// The account's token was refused.
    MsaTokenInvalid,
    NetworkUnavailable,
    /// A step's bound elapsed.
    TimeoutError,
    UserCanceled,
    InternalError,
    /// No token for the account could be obtained; nothing was sent.
    AccountTokenUnavailable,
    /// The session paired, but the phone is not in the account's device list.
    PhoneNotFound,
}

impl PairingResult {
    /// The variant's name, without the phone's id (for logs).
    pub fn name(&self) -> &'static str {
        match self {
            PairingResult::Success { .. } => "Success",
            PairingResult::PairingSessionExpired => "PairingSessionExpired",
            PairingResult::MsaTokenMismatched => "MsaTokenMismatched",
            PairingResult::ServiceError => "ServiceError",
            PairingResult::MsaTokenInvalid => "MsaTokenInvalid",
            PairingResult::NetworkUnavailable => "NetworkUnavailable",
            PairingResult::TimeoutError => "TimeoutError",
            PairingResult::UserCanceled => "UserCanceled",
            PairingResult::InternalError => "InternalError",
            PairingResult::AccountTokenUnavailable => "AccountTokenUnavailable",
            PairingResult::PhoneNotFound => "PhoneNotFound",
        }
    }
}

/// What the session shows the user now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingStep {
    /// The session is being created.
    Creating {
        flow: PairingFlow,
    },
    /// The QR flow: show `qr_code` (its value is the exact text to encode); `manual_link` is the link
    /// the manual instructions show.
    WaitingForScan {
        qr_code: ShownCode,
        manual_link: String,
    },
    /// Show `pin` until the phone confirms it.
    WaitingForPin {
        pin: ShownCode,
        kind: PinKind,
    },
    /// The shown PIN no longer works; a refresh (QR flow only) gets a new one. `refreshing` while a
    /// refresh runs; `failure` is the last refresh's.
    PinRefresh {
        pin: ShownCode,
        kind: PinKind,
        problem: PinProblem,
        refreshing: bool,
        failure: Option<RefreshFailure>,
    },
    /// The phone confirmed; the user grants permissions on the phone.
    WaitingForPermissions,
    /// The session paired; the PC learns the phone.
    Completing,
    Ended(PairingResult),
}
