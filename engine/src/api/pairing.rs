//! Pairing a phone to the signed-in Microsoft account by QR code or PIN, for the first-run window and
//! the *pair a new phone* actions.
//!
//! One session runs at a time; each change of what it shows arrives as
//! [`crate::edge::EngineEvent::Pairing`] with the session's number, and [`PairingApi::current`] reads
//! the last one. A session that ends in [`PairingResult::Success`] names a phone of
//! `PhoneApi::phones`, which the UI then selects.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::auth::pairing as session;

use super::{FeatureApiError, core_of};

/// How the user pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PairingFlow {
    /// Scan a QR code with the phone, then confirm a PIN.
    QrCode,
    /// Open a link on the phone and type a PIN into Link to Windows.
    ManualPin,
}

/// A code to show and when it stops working.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairingCode {
    /// The text to show — for a QR code, the exact text to encode.
    pub value: String,
    /// When it stops working, Unix seconds on this Mac's clock.
    pub expires_at: i64,
    /// Whole minutes it stays valid from when it arrived (at least 1).
    pub expires_in_minutes: u32,
}

/// Which PIN is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PairingPinKind {
    /// Confirms a scanned QR code on the phone; can be refreshed.
    Challenge,
    /// Typed into Link to Windows in the manual flow; cannot be refreshed.
    Pairing,
}

/// Why the shown PIN no longer works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PairingPinProblem {
    Expired,
    /// The phone entered another PIN.
    Wrong,
}

/// Why the last PIN refresh failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PairingRefreshFailure {
    NetworkUnavailable,
    ForbiddenDcgRequest,
    UnauthorizedRequest,
    MsaUnauthorizedRequest,
    TimeoutError,
    InternalError,
}

/// How a session ended.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum PairingResult {
    /// The phone `phone_id` joined the account and is one of its linked phones.
    Success {
        phone_id: String,
    },
    /// The QR code, the PIN or the session expired.
    SessionExpired,
    /// The phone signed in with another Microsoft account.
    AccountMismatched,
    /// The service answered something the session does not expect.
    ServiceError,
    /// The account's token was refused.
    TokenInvalid,
    NetworkUnavailable,
    TimedOut,
    Canceled,
    InternalError,
    /// No token for the account could be obtained.
    AccountTokenUnavailable,
    /// The session paired, but the phone is not in the account's device list.
    PhoneNotFound,
}

/// What a session shows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PairingStep {
    Creating {
        flow: PairingFlow,
    },
    /// Show `qr_code`; `manual_link` is what the manual instructions show.
    WaitingForScan {
        qr_code: PairingCode,
        manual_link: String,
    },
    WaitingForPin {
        pin: PairingCode,
        kind: PairingPinKind,
    },
    /// The shown PIN no longer works; a challenge PIN can be refreshed.
    PinRefresh {
        pin: PairingCode,
        kind: PairingPinKind,
        problem: PairingPinProblem,
        refreshing: bool,
        failure: Option<PairingRefreshFailure>,
    },
    /// The user grants permissions on the phone.
    WaitingForPermissions,
    /// The phone joined; this Mac is learning it.
    Completing,
    Ended {
        result: PairingResult,
    },
}

/// One step of session `session`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PairingUpdate {
    pub session: u64,
    pub step: PairingStep,
}

impl From<session::PairingFlow> for PairingFlow {
    fn from(f: session::PairingFlow) -> Self {
        match f {
            session::PairingFlow::QrCode => PairingFlow::QrCode,
            session::PairingFlow::ManualPin => PairingFlow::ManualPin,
        }
    }
}

impl From<PairingFlow> for session::PairingFlow {
    fn from(f: PairingFlow) -> Self {
        match f {
            PairingFlow::QrCode => session::PairingFlow::QrCode,
            PairingFlow::ManualPin => session::PairingFlow::ManualPin,
        }
    }
}

impl From<session::ShownCode> for PairingCode {
    fn from(c: session::ShownCode) -> Self {
        Self {
            value: c.value,
            expires_at: crate::time::unix_secs(c.expires_at),
            expires_in_minutes: c.expires_in_minutes,
        }
    }
}

impl From<session::PinKind> for PairingPinKind {
    fn from(k: session::PinKind) -> Self {
        match k {
            session::PinKind::Challenge => PairingPinKind::Challenge,
            session::PinKind::Pairing => PairingPinKind::Pairing,
        }
    }
}

impl From<session::PinProblem> for PairingPinProblem {
    fn from(p: session::PinProblem) -> Self {
        match p {
            session::PinProblem::Expired => PairingPinProblem::Expired,
            session::PinProblem::Wrong => PairingPinProblem::Wrong,
        }
    }
}

impl From<session::RefreshFailure> for PairingRefreshFailure {
    fn from(f: session::RefreshFailure) -> Self {
        use session::RefreshFailure as F;
        match f {
            F::NetworkUnavailable => Self::NetworkUnavailable,
            F::ForbiddenDcgRequest => Self::ForbiddenDcgRequest,
            F::UnauthorizedRequest => Self::UnauthorizedRequest,
            F::MsaUnauthorizedRequest => Self::MsaUnauthorizedRequest,
            F::TimeoutError => Self::TimeoutError,
            F::InternalError => Self::InternalError,
        }
    }
}

impl From<session::PairingResult> for PairingResult {
    fn from(r: session::PairingResult) -> Self {
        use session::PairingResult as R;
        match r {
            R::Success { phone_id } => Self::Success { phone_id },
            R::PairingSessionExpired => Self::SessionExpired,
            R::MsaTokenMismatched => Self::AccountMismatched,
            R::ServiceError => Self::ServiceError,
            R::MsaTokenInvalid => Self::TokenInvalid,
            R::NetworkUnavailable => Self::NetworkUnavailable,
            R::TimeoutError => Self::TimedOut,
            R::UserCanceled => Self::Canceled,
            R::InternalError => Self::InternalError,
            R::AccountTokenUnavailable => Self::AccountTokenUnavailable,
            R::PhoneNotFound => Self::PhoneNotFound,
        }
    }
}

impl From<session::PairingStep> for PairingStep {
    fn from(s: session::PairingStep) -> Self {
        use session::PairingStep as S;
        match s {
            S::Creating { flow } => Self::Creating { flow: flow.into() },
            S::WaitingForScan { qr_code, manual_link } => Self::WaitingForScan { qr_code: qr_code.into(), manual_link },
            S::WaitingForPin { pin, kind } => Self::WaitingForPin { pin: pin.into(), kind: kind.into() },
            S::PinRefresh { pin, kind, problem, refreshing, failure } => Self::PinRefresh {
                pin: pin.into(),
                kind: kind.into(),
                problem: problem.into(),
                refreshing,
                failure: failure.map(Into::into),
            },
            S::WaitingForPermissions => Self::WaitingForPermissions,
            S::Completing => Self::Completing,
            S::Ended(result) => Self::Ended { result: result.into() },
        }
    }
}

/// Pairing sessions of the signed-in account.
#[derive(uniffi::Object)]
pub struct PairingApi {
    engine: Arc<Engine>,
}

impl PairingApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }
}

#[uniffi::export]
impl PairingApi {
    /// Starts a session of `flow`, cancelling the one in progress, and returns its number. Needs a
    /// start that signed in and enrolled this Mac.
    pub fn start(&self, flow: PairingFlow) -> Result<u64, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        core.start_pairing(flow.into()).map_err(|e| FeatureApiError::NotAvailable { reason: e.to_string() })
    }

    /// Asks the session for a new challenge PIN (it answers only while it shows an expired or wrong
    /// one); `false` when no session runs.
    pub fn refresh_challenge_pin(&self) -> bool {
        core_of(&self.engine).is_ok_and(|core| core.pairing.refresh_challenge_pin())
    }

    /// Cancels the running session, if any; it ends with [`PairingResult::Canceled`].
    pub fn cancel(&self) {
        if let Ok(core) = core_of(&self.engine) {
            core.pairing.cancel();
        }
    }

    /// The last step of the current or last session.
    pub fn current(&self) -> Option<PairingUpdate> {
        let (session, step) = core_of(&self.engine).ok()?.pairing.last_step()?;
        Some(PairingUpdate { session, step: step.into() })
    }

    /// The sign-in name of the account a phone is paired to (its e-mail address or phone number),
    /// for the manual instructions.
    pub fn account_name(&self) -> Option<String> {
        core_of(&self.engine).ok()?.pairing.account_name()
    }
}
