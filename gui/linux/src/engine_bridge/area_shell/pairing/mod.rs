//! Pairing a phone to the signed-in Microsoft account by QR code or PIN, as the pairing view shows it:
//! its pages, from the steps of the engine's session. The QR code is the entry; "Pair Manually" shows
//! the manual instructions, whose "Continue" starts a PIN session in place of the QR one.
//!
//! Two limits are the view's own: a page still loading its code after [`LOADING_LIMIT`] fails and the
//! session is cancelled; a wait for the phone's permissions longer than [`PERMISSIONS_LIMIT`] shows the
//! error page (the session goes on until the next start or cancel).

use std::time::Duration;

use phonelink_engine::api::pairing::{
    PairingCode, PairingFlow, PairingPinKind, PairingPinProblem, PairingRefreshFailure, PairingResult, PairingStep,
    PairingUpdate,
};
use serde_json::{Value, json};

use crate::l10n::{fill, translate};

pub(super) const LOADING_LIMIT: Duration = Duration::from_secs(35);
pub(super) const PERMISSIONS_LIMIT: Duration = Duration::from_secs(180);

/// Why the error page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Failure {
    /// The QR code, PIN or session expired; a new one can be requested.
    Expired,
    /// No code came within the loading limit.
    LoadingTookTooLong,
    /// The phone's permissions did not come within the permissions limit.
    PermissionsTookTooLong,
    /// The session could not start: why, as the user reads it.
    CouldNotStart(String),
    Ended(PairingResult),
}

impl Failure {
    /// What the error page says.
    fn message(&self) -> String {
        let text = match self {
            Self::Expired | Self::Ended(PairingResult::SessionExpired) => {
                "The code expired. Get a new one to try again."
            }
            Self::LoadingTookTooLong => "Phone Link couldn't get a code. Check your internet connection and try again.",
            Self::PermissionsTookTooLong => "This is taking longer than expected. Check your phone, then try again.",
            Self::CouldNotStart(reason) => return reason.clone(),
            Self::Ended(PairingResult::AccountMismatched) => {
                "Your phone is signed in to a different Microsoft account. Sign in to Link to Windows with the account you use on this computer."
            }
            Self::Ended(PairingResult::TokenInvalid | PairingResult::AccountTokenUnavailable) => {
                "Phone Link couldn't use your Microsoft account. Sign out and sign in again, then try again."
            }
            Self::Ended(PairingResult::NetworkUnavailable) => {
                "Phone Link couldn't reach Microsoft. Check your internet connection and try again."
            }
            Self::Ended(PairingResult::TimedOut) => "Microsoft didn't answer in time. Try again.",
            Self::Ended(PairingResult::PhoneNotFound) => {
                "Your phone joined your account, but Phone Link couldn't find it yet. Try again in a moment."
            }
            Self::Ended(
                PairingResult::ServiceError
                | PairingResult::InternalError
                | PairingResult::Success { .. }
                | PairingResult::Canceled,
            ) => "Pairing didn't finish. Try again.",
        };
        translate(text).to_owned()
    }
}

/// Why a new PIN could not be had.
fn refresh_message(failure: PairingRefreshFailure) -> &'static str {
    translate(match failure {
        PairingRefreshFailure::NetworkUnavailable => "Check your internet connection and try again.",
        PairingRefreshFailure::TimeoutError => "Microsoft didn't answer in time. Try again.",
        PairingRefreshFailure::UnauthorizedRequest | PairingRefreshFailure::MsaUnauthorizedRequest => {
            "Phone Link couldn't use your Microsoft account."
        }
        PairingRefreshFailure::ForbiddenDcgRequest | PairingRefreshFailure::InternalError => {
            "Phone Link couldn't get a new PIN."
        }
    })
}

/// A page of the pairing view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Page {
    /// No session yet.
    Idle,
    /// A session is being created.
    Loading,
    /// Show this QR code; `manual_link` is for the manual instructions.
    QrCode {
        code: PairingCode,
        manual_link: String,
    },
    /// How to pair by hand: open `link` on the phone and sign in with `account_name`.
    ManualInstructions {
        link: String,
        account_name: Option<String>,
    },
    Pin {
        code: PairingCode,
        kind: PairingPinKind,
    },
    /// The shown PIN expired or was wrong; a challenge PIN can be refreshed.
    PinProblem {
        code: PairingCode,
        kind: PairingPinKind,
        problem: PairingPinProblem,
        refreshing: bool,
        failure: Option<PairingRefreshFailure>,
    },
    WaitingForPermissions,
    /// The phone joined; this computer is learning it.
    Completing,
    Paired,
    Failed(Failure),
}

/// A limit that runs out after `after`, unless the page changed: then [`Pairing::limit_reached`] with
/// `generation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Timer {
    pub(super) after: Duration,
    pub(super) generation: u64,
}

/// What the bridge does after the pairing changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Effects {
    /// A limit to start.
    pub(super) timer: Option<Timer>,
    /// The session paired the phone with this id: select it.
    pub(super) paired: Option<String>,
    /// Cancel the engine's session.
    pub(super) cancel_session: bool,
}

/// The pairing view's state: the page, and the session it follows.
#[derive(Debug)]
pub(super) struct Pairing {
    page: Page,
    /// The flow of the current or last session.
    flow: PairingFlow,
    session: Option<u64>,
    /// The QR session's last code and manual link.
    last_scan: Option<(PairingCode, String)>,
    /// Counts the pages shown: a limit applies only while its page is still the one shown.
    generation: u64,
}

impl Default for Pairing {
    fn default() -> Self {
        Self { page: Page::Idle, flow: PairingFlow::QrCode, session: None, last_scan: None, generation: 0 }
    }
}

impl Pairing {
    #[cfg(test)]
    pub(super) fn page(&self) -> &Page {
        &self.page
    }

    /// The flow of the current or last session, which "Try Again" starts anew.
    pub(super) fn flow(&self) -> PairingFlow {
        self.flow
    }

    /// A session of `flow` was asked for: `started` holds its number, or why it did not start.
    pub(super) fn started(&mut self, flow: PairingFlow, started: Result<u64, String>) -> Effects {
        self.flow = flow;
        self.last_scan = None;
        match started {
            Ok(session) => {
                self.session = Some(session);
                self.show(Page::Loading)
            }
            Err(reason) => {
                self.session = None;
                self.show(Page::Failed(Failure::CouldNotStart(reason)))
            }
        }
    }

    /// From the QR page: how to pair by hand, with the QR session's manual link. The QR session goes
    /// on until "Continue". Nothing changes before the QR session has a code.
    pub(super) fn show_manual(&mut self, account_name: Option<String>) -> Effects {
        let Some((_, link)) = &self.last_scan else { return Effects::default() };
        let link = link.clone();
        self.show(Page::ManualInstructions { link, account_name })
    }

    /// From the manual instructions: back to the running QR session's code; `None` when there is none
    /// and a new QR session must start.
    pub(super) fn back_to_qr(&mut self) -> Option<Effects> {
        let showing_manual = matches!(self.page, Page::ManualInstructions { .. });
        if self.session.is_none() || self.flow != PairingFlow::QrCode || !showing_manual {
            return None;
        }
        let (code, manual_link) = self.last_scan.clone()?;
        Some(self.show(Page::QrCode { code, manual_link }))
    }

    /// A new challenge PIN can be asked for: an expired or wrong one shows and none is on its way.
    pub(super) fn can_refresh_pin(&self) -> bool {
        matches!(self.page, Page::PinProblem { kind: PairingPinKind::Challenge, refreshing: false, .. })
    }

    /// The engine took the request for a new PIN.
    pub(super) fn refreshing(&mut self) {
        if let Page::PinProblem { refreshing, .. } = &mut self.page {
            *refreshing = true;
        }
    }

    /// The user left pairing.
    pub(super) fn cancel(&mut self) {
        self.generation += 1;
        self.session = None;
        self.page = Page::Idle;
    }

    /// A step of the engine's session; steps of other sessions are not this view's.
    pub(super) fn apply(&mut self, update: PairingUpdate) -> Effects {
        if Some(update.session) != self.session {
            return Effects::default();
        }
        match update.step {
            PairingStep::Creating { .. } => self.show(Page::Loading),
            PairingStep::WaitingForScan { qr_code, manual_link } => {
                self.last_scan = Some((qr_code.clone(), manual_link.clone()));
                // The manual instructions stay up while the QR session goes on.
                if matches!(self.page, Page::ManualInstructions { .. }) {
                    return Effects::default();
                }
                self.show(Page::QrCode { code: qr_code, manual_link })
            }
            PairingStep::WaitingForPin { pin, kind } => self.show(Page::Pin { code: pin, kind }),
            PairingStep::PinRefresh { pin, kind, problem, refreshing, failure } => {
                self.show(Page::PinProblem { code: pin, kind, problem, refreshing, failure })
            }
            PairingStep::WaitingForPermissions if self.page == Page::WaitingForPermissions => Effects::default(),
            PairingStep::WaitingForPermissions => self.show(Page::WaitingForPermissions),
            PairingStep::Completing => self.show(Page::Completing),
            PairingStep::Ended { result } => {
                self.session = None;
                self.ended(result)
            }
        }
    }

    fn ended(&mut self, result: PairingResult) -> Effects {
        match result {
            PairingResult::Success { phone_id } => Effects { paired: Some(phone_id), ..self.show(Page::Paired) },
            // A cancel this view asked for has already set the page.
            PairingResult::Canceled if matches!(self.page, Page::Failed(_)) => Effects::default(),
            PairingResult::Canceled => self.show(Page::Idle),
            PairingResult::SessionExpired => self.show(Page::Failed(Failure::Expired)),
            other => self.show(Page::Failed(Failure::Ended(other))),
        }
    }

    /// The limit started for `generation` ran out.
    pub(super) fn limit_reached(&mut self, generation: u64) -> Effects {
        if generation != self.generation {
            return Effects::default();
        }
        match self.page {
            Page::Loading => {
                tracing::warn!("the pairing code was not received in time; the session is cancelled");
                self.session = None;
                self.page = Page::Failed(Failure::LoadingTookTooLong);
                Effects { cancel_session: true, ..Effects::default() }
            }
            Page::WaitingForPermissions => {
                tracing::warn!("the phone's permissions were not granted in time; pairing shows the failure");
                self.page = Page::Failed(Failure::PermissionsTookTooLong);
                Effects::default()
            }
            _ => Effects::default(),
        }
    }

    /// Shows `page` and starts the limit that page has, if any.
    fn show(&mut self, page: Page) -> Effects {
        self.generation += 1;
        let limit = match page {
            Page::Loading => Some(LOADING_LIMIT),
            Page::WaitingForPermissions => Some(PERMISSIONS_LIMIT),
            _ => None,
        };
        self.page = page;
        Effects { timer: limit.map(|after| Timer { after, generation: self.generation }), ..Effects::default() }
    }

    /// What the pairing view shows, as JSON.
    pub(super) fn json(&self) -> Value {
        let flow = match self.flow {
            PairingFlow::QrCode => "qrCode",
            PairingFlow::ManualPin => "manualPin",
        };
        let kind = |kind: &PairingPinKind| match kind {
            PairingPinKind::Challenge => "challenge",
            PairingPinKind::Pairing => "pairing",
        };
        let mut value = match &self.page {
            Page::Idle => json!({ "page": "idle" }),
            Page::Loading => json!({ "page": "loading" }),
            Page::QrCode { code, .. } => json!({ "page": "qrCode", "code": code.value, "expiresAt": code.expires_at }),
            Page::ManualInstructions { link, account_name } => json!({
                "page": "manualInstructions",
                "link": link,
                "signInWith": match account_name {
                    Some(name) => fill(translate("Sign in with %@."), std::slice::from_ref(name)),
                    None => translate("Sign in with the Microsoft account you use on this computer.").to_owned(),
                },
            }),
            Page::Pin { code, kind: pin_kind } => json!({
                "page": "pin",
                "code": code.value,
                "expiresAt": code.expires_at,
                "pinKind": kind(pin_kind),
            }),
            Page::PinProblem { code, kind: pin_kind, problem, refreshing, failure } => json!({
                "page": "pinProblem",
                "code": code.value,
                "pinKind": kind(pin_kind),
                "problem": match problem {
                    PairingPinProblem::Expired => "expired",
                    PairingPinProblem::Wrong => "wrong",
                },
                "refreshing": refreshing,
                "message": failure.map(refresh_message),
            }),
            Page::WaitingForPermissions => json!({ "page": "waitingForPermissions" }),
            Page::Completing => json!({ "page": "completing" }),
            Page::Paired => json!({ "page": "paired" }),
            Page::Failed(failure) => json!({
                "page": "failed",
                "message": failure.message(),
                "expired": matches!(failure, Failure::Expired | Failure::Ended(PairingResult::SessionExpired)),
            }),
        };
        value["flow"] = Value::from(flow);
        value
    }
}

#[cfg(test)]
mod tests;
