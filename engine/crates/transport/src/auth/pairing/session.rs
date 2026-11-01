//! The calls, the phone lookup and one pairing session.

use super::{
    PairingFlow, PairingResult, PairingStep, PinKind, PinProblem, RefreshFailure, ServiceState, SessionResponse,
    ShownCode, create_request_body,
};
use crate::auth::dcg_rest::{DcgRest, RestError, ServiceRequestStatus};
use crate::auth::msa::MsaError;
use async_trait::async_trait;
use phonelink_core::config::PairingConfig;
use phonelink_core::edge::{Clock, Reachability};
use phonelink_core::types::{Environment, TraceContext};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// The four calls of a session, each returning the 2xx reply's body.
#[async_trait]
pub trait PairingCalls: Send + Sync {
    async fn create(&self, flow: PairingFlow) -> Result<String, RestError>;
    async fn get(&self, session_id: &str) -> Result<String, RestError>;
    async fn complete(&self, session_id: &str) -> Result<String, RestError>;
    async fn refresh_challenge_pin(&self, session_id: &str) -> Result<String, RestError>;
}

/// Learns the paired phone: refreshes the account's device list from the cloud and says whether
/// `phone_id` is one of its linked phones now.
#[async_trait]
pub trait PhoneLookup: Send + Sync {
    async fn learn_phone(&self, phone_id: &str) -> bool;
}

/// Receives every step of a session, tagged with the session's number.
pub type PairingObserver = Arc<dyn Fn(u64, PairingStep) + Send + Sync>;

/// The session's calls over the DCG REST client: one MSA token for every call, the trace context
/// of the session, and `clientRegionName` on the create call.
pub(super) struct RestCalls {
    pub(super) rest: DcgRest,
    pub(super) env: Environment,
    pub(super) msa: String,
    pub(super) region: Option<String>,
    pub(super) trace: TraceContext,
}

#[async_trait]
impl PairingCalls for RestCalls {
    async fn create(&self, flow: PairingFlow) -> Result<String, RestError> {
        self.rest
            .create_pairing_session(self.env, &self.msa, self.region.as_deref(), create_request_body(flow), &self.trace)
            .await
    }

    async fn get(&self, session_id: &str) -> Result<String, RestError> {
        self.rest.pairing_session(self.env, &self.msa, session_id, &self.trace).await
    }

    async fn complete(&self, session_id: &str) -> Result<String, RestError> {
        self.rest.complete_pairing_session(self.env, &self.msa, session_id, &self.trace).await
    }

    async fn refresh_challenge_pin(&self, session_id: &str) -> Result<String, RestError> {
        self.rest.refresh_challenge_pin(self.env, &self.msa, session_id, &self.trace).await
    }
}

/// Why a session could not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingStartError {
    /// The PC is not signed in and enrolled in an account yet.
    #[error("not signed in to an account")]
    NotSignedIn,
}

/// How a session ends: its result, and what made it end when that is not the user, the clock or
/// the pairing itself — the cause the session's end line reports.
pub(super) struct Ending {
    pub(super) result: PairingResult,
    cause: Option<Cause>,
}

impl Ending {
    pub(super) fn new(result: PairingResult, cause: Cause) -> Self {
        Self { result, cause: Some(cause) }
    }

    /// The session's end line: `info` for a pairing, an expiry or a cancel, `warn` with the cause
    /// for any other end.
    pub(super) fn log(&self) {
        let outcome = self.result.name();
        match (&self.result, &self.cause) {
            (PairingResult::Success { phone_id }, _) => {
                info!(outcome, phone = phone_id.as_str(), "pairing session ended")
            }
            (_, None) => info!(outcome, "pairing session ended"),
            (_, Some(cause)) => {
                warn!(outcome, reason = %cause, raw_state = cause.raw_state(), "pairing session ended");
            }
        }
    }
}

impl From<PairingResult> for Ending {
    fn from(result: PairingResult) -> Self {
        Self { result, cause: None }
    }
}

impl From<(PairingResult, Cause)> for Ending {
    fn from((result, cause): (PairingResult, Cause)) -> Self {
        Self::new(result, cause)
    }
}

/// What made a session, or a challenge-PIN refresh, fail.
#[derive(Debug, thiserror::Error)]
pub(super) enum Cause {
    /// A reply without a usable value of the key (its wire name).
    #[error("the reply has no usable {0}")]
    Field(&'static str),
    /// A state the step does not accept.
    #[error("state {} is not accepted here", .0.wire())]
    State(ServiceState),
    /// A `state` that is missing, not a string, or none the session knows (the string as sent).
    #[error("the reply's state is missing or unknown")]
    UnknownState(Option<String>),
    #[error("the reply is not a JSON object ({body_len} bytes)")]
    NotAnObject { body_len: usize },
    /// A step's bound elapsed.
    #[error("no reply within {0:?}")]
    TimedOut(Duration),
    /// The step's last call failed.
    #[error("{0}")]
    Call(#[source] RestError),
    /// No token for the account; nothing was sent.
    #[error("no token for the account: {0}")]
    Token(#[source] MsaError),
    #[error("the paired phone is not in the account's device list")]
    NotListed,
}

impl Cause {
    /// The `state` string as the service sent it, when it is one the session does not know.
    fn raw_state(&self) -> Option<&str> {
        match self {
            Cause::UnknownState(raw) => raw.as_deref(),
            _ => None,
        }
    }
}

/// A reply without a usable value of `key`: the session ends with `ServiceError`.
fn missing(key: &'static str) -> Ending {
    Ending::new(PairingResult::ServiceError, Cause::Field(key))
}

/// A step attempt's failure: a call that failed (retried when recoverable) or a reply that ends the
/// session.
pub(super) enum StepFailure {
    Call(RestError),
    End(Ending),
}

impl From<(PairingResult, Cause)> for StepFailure {
    fn from(ending: (PairingResult, Cause)) -> Self {
        StepFailure::End(ending.into())
    }
}

/// The session's phase.
#[derive(Debug, Clone)]
pub(super) enum Phase {
    Scan,
    Pin { pin: ShownCode, kind: PinKind },
    Complete,
    Permissions,
    Refresh { pin: ShownCode, kind: PinKind, problem: PinProblem },
}

/// Runs one session: everything but its start and its cancel.
pub struct Session {
    pub config: PairingConfig,
    pub flow: PairingFlow,
    pub calls: Arc<dyn PairingCalls>,
    pub lookup: Arc<dyn PhoneLookup>,
    pub clock: Arc<dyn Clock>,
    pub reachability: Arc<dyn Reachability>,
    pub emit: Box<dyn Fn(PairingStep) + Send + Sync>,
}

/// A user command to a running session.
pub(super) enum Command {
    Refresh,
}

impl Session {
    /// Runs the session to its end, reporting each step, and returns how it ended (also reported as
    /// the last step). `commands` carries the user's refresh requests.
    pub(super) async fn run(self, mut commands: mpsc::UnboundedReceiver<Command>) -> PairingResult {
        let ending = self.drive(&mut commands).await;
        ending.log();
        (self.emit)(PairingStep::Ended(ending.result.clone()));
        ending.result
    }

    async fn drive(&self, commands: &mut mpsc::UnboundedReceiver<Command>) -> Ending {
        let (session_id, session_expiry, mut phase) = match self.create_session().await {
            Ok(created) => created,
            Err(result) => return result,
        };

        loop {
            let next = match phase {
                Phase::Refresh { pin, kind, problem } => {
                    self.wait_for_refresh(&session_id, session_expiry, pin, kind, problem, commands).await
                }
                Phase::Complete => self.complete_step(&session_id).await,
                polled => self.poll_step(&session_id, polled).await,
            };
            phase = match next {
                Ok(phase) => phase,
                Err(result) => return result,
            };
        }
    }

    /// Creates the session and shows its first code: the session id, its expiry on the PC's clock
    /// and the first phase, or how the session ends.
    async fn create_session(&self) -> Result<(String, SystemTime, Phase), Ending> {
        (self.emit)(PairingStep::Creating { flow: self.flow });

        let config = &self.config;
        let created = self
            .attempts(config.create_attempts, config.create_timeout, |_| async {
                let body = self.calls.create(self.flow).await.map_err(StepFailure::Call)?;
                Ok(SessionResponse::parse_reply(&body)?)
            })
            .await?;

        let now = self.clock.now();
        let session_id =
            created.text("pairingSessionId").map(str::to_string).ok_or_else(|| missing("pairingSessionId"))?;
        let session_expiry =
            created.local_expiry("pairingSessionExpireTime", now).ok_or_else(|| missing("pairingSessionExpireTime"))?;
        let phase = self.first_phase(&created, now)?;

        info!(flow = self.flow.wire(), "pairing session created");
        Ok((session_id, session_expiry, phase))
    }

    /// The phase a created session starts in, after its code is shown, or how the session ends.
    fn first_phase(&self, created: &SessionResponse, now: SystemTime) -> Result<Phase, Ending> {
        match (self.flow, created.state) {
            (PairingFlow::QrCode, ServiceState::PendingPairingLinkVisit) => {
                let qr_code = created
                    .shown_code("pairingLink", "pairingLinkExpireTime", now)
                    .ok_or_else(|| missing("pairingLink"))?;
                let manual_link = created
                    .text("pairingLinkForSignInPINFlow")
                    .map(str::to_string)
                    .ok_or_else(|| missing("pairingLinkForSignInPINFlow"))?;
                (self.emit)(PairingStep::WaitingForScan { qr_code, manual_link });
                Ok(Phase::Scan)
            }
            (PairingFlow::ManualPin, ServiceState::PendingPairingPinValidate) => {
                let pin = created
                    .shown_code("pairingPIN", "pairingPINExpireTime", now)
                    .ok_or_else(|| missing("pairingPIN"))?;
                (self.emit)(PairingStep::WaitingForPin { pin: pin.clone(), kind: PinKind::Pairing });
                Ok(Phase::Pin { pin, kind: PinKind::Pairing })
            }
            (_, state) => Err(unexpected(state)),
        }
    }

    /// The `Complete` phase: completes the session (retried as a poll) and moves on to the
    /// permissions, or ends the session.
    async fn complete_step(&self, session_id: &str) -> Result<Phase, Ending> {
        let config = &self.config;
        let response = self
            .attempts(config.poll_step_attempts, config.poll_step_timeout, |attempt| async move {
                // A failed `Complete` is retried as a poll.
                let body =
                    if attempt == 0 { self.calls.complete(session_id).await } else { self.calls.get(session_id).await };
                self.step_reply(body, |s| s == ServiceState::PendingPairingPermissionApprove).await
            })
            .await?;

        match response.state {
            ServiceState::PendingPairingPermissionApprove => {
                (self.emit)(PairingStep::WaitingForPermissions);
                Ok(Phase::Permissions)
            }
            ServiceState::DevicePaired => Err(self.complete_pairing(&response).await),
            ServiceState::PairingSessionExpired => Err(PairingResult::PairingSessionExpired.into()),
            other => Err(unexpected(other)),
        }
    }

    /// A polled phase (scan, PIN, permissions): one poll, keeping the phase while the service state
    /// does, else the phase the response moves it to, or how the session ends.
    async fn poll_step(&self, session_id: &str, polled: Phase) -> Result<Phase, Ending> {
        let config = &self.config;
        let keeps = |state: ServiceState| match &polled {
            Phase::Scan => state == ServiceState::PendingPairingLinkVisit,
            Phase::Pin { .. } => {
                matches!(state, ServiceState::PendingPairingPinValidate | ServiceState::PendingChallengePinValidate)
            }
            Phase::Permissions => state == ServiceState::PendingPairingPermissionApprove,
            Phase::Complete | Phase::Refresh { .. } => false,
        };

        let response = self
            .attempts(config.poll_step_attempts, config.poll_step_timeout, |_| async move {
                self.step_reply(self.calls.get(session_id).await, keeps).await
            })
            .await?;

        if keeps(response.state) { Ok(polled) } else { self.next_phase(polled, &response).await }
    }

    /// The phase a poll response moves `phase` to (a response that keeps the phase is handled by
    /// the caller), or how the session ends.
    async fn next_phase(&self, phase: Phase, response: &SessionResponse) -> Result<Phase, Ending> {
        let now = self.clock.now();
        let state = response.state;

        match state {
            ServiceState::DevicePaired => return Err(self.complete_pairing(response).await),
            ServiceState::PendingPairingPermissionApprove => {
                (self.emit)(PairingStep::WaitingForPermissions);
                return Ok(Phase::Permissions);
            }
            _ => {}
        }

        match (phase, state) {
            (Phase::Scan, ServiceState::PendingChallengePinValidate) => {
                let pin = response
                    .shown_code("challengePIN", "challengePINExpireTime", now)
                    .ok_or_else(|| missing("challengePIN"))?;
                (self.emit)(PairingStep::WaitingForPin { pin: pin.clone(), kind: PinKind::Challenge });
                Ok(Phase::Pin { pin, kind: PinKind::Challenge })
            }
            (Phase::Scan, ServiceState::ChallengePinValidateFailed) => {
                let pin = response
                    .shown_code("challengePIN", "challengePINExpireTime", now)
                    .ok_or_else(|| missing("challengePIN"))?;
                Ok(Phase::Refresh { pin, kind: PinKind::Challenge, problem: PinProblem::Wrong })
            }
            (Phase::Scan, ServiceState::PairingLinkExpired | ServiceState::PairingSessionExpired) => {
                Err(PairingResult::PairingSessionExpired.into())
            }
            (Phase::Pin { .. }, ServiceState::ReadyForComplete) => {
                (self.emit)(PairingStep::WaitingForPermissions);
                Ok(Phase::Complete)
            }
            (
                Phase::Pin { pin, kind },
                ServiceState::ChallengePinExpired | ServiceState::ChallengePinValidateFailed,
            ) => {
                let problem =
                    if state == ServiceState::ChallengePinExpired { PinProblem::Expired } else { PinProblem::Wrong };
                Ok(Phase::Refresh { pin, kind, problem })
            }
            (Phase::Pin { .. }, ServiceState::PairingPinExpired | ServiceState::PairingSessionExpired) => {
                Err(PairingResult::PairingSessionExpired.into())
            }
            (Phase::Permissions, ServiceState::PairingSessionExpired) => {
                Err(PairingResult::PairingSessionExpired.into())
            }
            (_, other) => Err(unexpected(other)),
        }
    }

    /// The *PIN refresh* phase: no polling; it waits for the session's expiry or the user's refresh.
    /// A refresh that succeeds resumes polling with its PIN; one that fails waits again.
    async fn wait_for_refresh(
        &self,
        session_id: &str,
        session_expiry: SystemTime,
        mut pin: ShownCode,
        kind: PinKind,
        problem: PinProblem,
        commands: &mut mpsc::UnboundedReceiver<Command>,
    ) -> Result<Phase, Ending> {
        let mut failure = None;

        loop {
            let remaining = session_expiry.duration_since(self.clock.now()).unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                return Err(PairingResult::PairingSessionExpired.into());
            }

            (self.emit)(PairingStep::PinRefresh { pin: pin.clone(), kind, problem, refreshing: false, failure });
            let command = tokio::select! {
                _ = tokio::time::sleep(remaining) => return Err(PairingResult::PairingSessionExpired.into()),
                command = commands.recv() => command,
            };
            match command {
                Some(Command::Refresh) if kind == PinKind::Challenge => {}
                Some(Command::Refresh) => {
                    debug!("the manual flow's PIN cannot be refreshed");
                    continue;
                }
                // The session's owner is gone: it was cancelled.
                None => return Err(PairingResult::UserCanceled.into()),
            }

            (self.emit)(PairingStep::PinRefresh { pin: pin.clone(), kind, problem, refreshing: true, failure });
            match self.refresh(session_id).await {
                Ok(new_pin) => {
                    pin = new_pin;
                    (self.emit)(PairingStep::WaitingForPin { pin: pin.clone(), kind });
                    return Ok(Phase::Pin { pin, kind });
                }
                Err((f, cause)) => {
                    warn!(
                        refresh_outcome = ?f,
                        reason = %cause,
                        raw_state = cause.raw_state(),
                        "challenge-PIN refresh failed; the session waits for another"
                    );
                    failure = Some(f);
                }
            }
        }
    }

    /// One `RefreshChallengePIN`, bounded: its new challenge PIN, or why not and what made it fail.
    async fn refresh(&self, session_id: &str) -> Result<ShownCode, (RefreshFailure, Cause)> {
        let call = self.calls.refresh_challenge_pin(session_id);
        let body = match tokio::time::timeout(self.config.refresh_timeout, call).await {
            Err(_) => return Err((RefreshFailure::TimeoutError, Cause::TimedOut(self.config.refresh_timeout))),
            Ok(Err(e)) => return Err((self.refresh_failure(&e), Cause::Call(e))),
            Ok(Ok(body)) => body,
        };

        let response =
            SessionResponse::parse_reply(&body).map_err(|(_, cause)| (RefreshFailure::InternalError, cause))?;
        if response.state != ServiceState::PendingChallengePinValidate {
            return Err((RefreshFailure::InternalError, Cause::State(response.state)));
        }

        response
            .shown_code("challengePIN", "challengePINExpireTime", self.clock.now())
            .ok_or((RefreshFailure::InternalError, Cause::Field("challengePIN")))
    }

    fn refresh_failure(&self, e: &RestError) -> RefreshFailure {
        use ServiceRequestStatus as S;
        if !self.reachability.is_internet_available() {
            return RefreshFailure::NetworkUnavailable;
        }

        match e.status() {
            Some(S::BadRequest | S::ForbiddenDcgRequest) => RefreshFailure::ForbiddenDcgRequest,
            Some(S::InternetError) => RefreshFailure::NetworkUnavailable,
            Some(S::UnauthorizedRequest | S::StrongAuthUnauthorizedRequest) => RefreshFailure::UnauthorizedRequest,
            Some(S::MsaUnauthorizedRequest) => RefreshFailure::MsaUnauthorizedRequest,
            _ => RefreshFailure::InternalError,
        }
    }

    /// A step's reply: parsed, and when `wait` holds for its state the poll interval waited out (part
    /// of the step, so its bound covers it).
    async fn step_reply(
        &self,
        body: Result<String, RestError>,
        wait: impl Fn(ServiceState) -> bool,
    ) -> Result<SessionResponse, StepFailure> {
        let response = SessionResponse::parse_reply(&body.map_err(StepFailure::Call)?)?;
        if wait(response.state) {
            tokio::time::sleep(self.config.poll_interval).await;
        }
        Ok(response)
    }

    /// Runs `step` up to `max` times, each bounded by `bound`: a recoverable call failure is retried
    /// at once; the bound elapsing ends the session (`TimeoutError`), as does any other failure.
    async fn attempts<'a, F, Fut>(&'a self, max: u32, bound: Duration, mut step: F) -> Result<SessionResponse, Ending>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<SessionResponse, StepFailure>> + 'a,
    {
        let mut attempt = 0;
        loop {
            match tokio::time::timeout(bound, step(attempt)).await {
                Err(_) => return Err(Ending::new(PairingResult::TimeoutError, Cause::TimedOut(bound))),
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(StepFailure::End(ending))) => return Err(ending),
                Ok(Err(StepFailure::Call(e))) => {
                    attempt += 1;
                    if attempt < max && self.recoverable(&e) {
                        warn!(error = %e, attempt, "pairing call failed; retrying");
                        continue;
                    }
                    return Err(Ending::new(self.final_result(&e), Cause::Call(e)));
                }
            }
        }
    }

    /// An internet error, a 401 other than categories 1 and 4, 429, or 5xx; anything else only while
    /// the PC has no internet connection — except a cancelled request, a 401 of category 1 or 4 and
    /// an open circuit, which never are.
    fn recoverable(&self, e: &RestError) -> bool {
        use ServiceRequestStatus as S;
        match e.status() {
            Some(
                S::InternetError
                | S::UnauthorizedRequest
                | S::TooManyRequests
                | S::ServiceError
                | S::AccessTokenRetrievalError,
            ) => true,
            Some(S::Cancelled | S::MsaUnauthorizedRequest | S::StrongAuthUnauthorizedRequest | S::BrokenCircuit) => {
                false
            }
            _ => !self.reachability.is_internet_available(),
        }
    }

    fn final_result(&self, e: &RestError) -> PairingResult {
        use ServiceRequestStatus as S;
        match e.status() {
            Some(
                S::AccessTokenRetrievalError
                | S::MsaUnauthorizedRequest
                | S::UnauthorizedRequest
                | S::StrongAuthUnauthorizedRequest
                | S::MsaTokenUserInteractionRequired,
            ) => PairingResult::MsaTokenInvalid,
            Some(S::InternetError) => PairingResult::NetworkUnavailable,
            _ if !self.reachability.is_internet_available() => PairingResult::NetworkUnavailable,
            _ => PairingResult::InternalError,
        }
    }

    /// `DEVICE_PAIRED`: the phone's id must be given and the refreshed device list must hold it.
    async fn complete_pairing(&self, response: &SessionResponse) -> Ending {
        (self.emit)(PairingStep::Completing);
        let Some(phone_id) = response.text("partnerDcgClientId") else {
            return Ending::new(PairingResult::PhoneNotFound, Cause::Field("partnerDcgClientId"));
        };

        if self.lookup.learn_phone(phone_id).await {
            PairingResult::Success { phone_id: phone_id.to_string() }.into()
        } else {
            Ending::new(PairingResult::PhoneNotFound, Cause::NotListed)
        }
    }
}

/// A state the current phase does not accept: `MsaTokenMismatched` for `PAIRING_ACCOUNT_MISMATCHED`,
/// else `ServiceError`.
pub(super) fn unexpected(state: ServiceState) -> Ending {
    let result = if state == ServiceState::PairingAccountMismatched {
        PairingResult::MsaTokenMismatched
    } else {
        PairingResult::ServiceError
    };

    Ending::new(result, Cause::State(state))
}
