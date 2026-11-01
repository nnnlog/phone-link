//! QR/PIN account pairing: joins a phone to the Microsoft account this PC is signed in with, through
//! the `DevicePairingProxy` service's pairing sessions.
//!
//! The PC creates a session, shows its QR code (or, in the manual flow, its PIN), polls the session
//! while the user finishes on the phone in Link to Windows, and learns the phone's `dcgClientId`
//! once the session reports `DEVICE_PAIRED`. No key, certificate or token passes through the
//! session: the PC then trusts the phone through a forced device-list refresh ([`PhoneLookup`]).
//!
//! One session runs at a time ([`PairingManager`]); starting one cancels the one in progress, and
//! cancelling is local (the service has no cancel call). The session lives in memory only. Every
//! change of what the user sees is reported to the session's observer as a [`PairingStep`].
//!
//! Where Phone Link on Windows retries a malformed reply (a body that is not a session object, a
//! missing or unknown `state`, an unparsable date) at once and without end, a session here ends:
//! `InternalError` for a body that is not a JSON object, `ServiceError` for the rest.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use tracing::debug;

use super::AuthService;
use super::dcg_rest::DcgRest;
use super::store::AccountState;
use phonelink_core::config::PairingConfig;
use phonelink_core::edge::{Clock, HostInfo, Reachability};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::TraceContext;

mod session;
mod view;
mod wire;

#[cfg(feature = "test-support")]
pub use session::PairingCalls;
use session::{Cause, Command, Ending, RestCalls};
pub use session::{PairingObserver, PairingStartError, PhoneLookup, Session};
pub use view::{PairingResult, PairingStep, PinKind, PinProblem, RefreshFailure, ShownCode};
#[cfg(feature = "test-support")]
pub use wire::OPTIONAL_PERMISSIONS;
pub use wire::{PairingFlow, ServiceState, SessionResponse, create_request_body};

// ------------------------------------------------------------------------------------------------
// The manager
// ------------------------------------------------------------------------------------------------

/// The running session: its number, its commands, and its cancel.
struct Running {
    number: u64,
    commands: mpsc::UnboundedSender<Command>,
    cancel: Option<oneshot::Sender<()>>,
}

/// Runs the account's pairing sessions, one at a time, over the primary identity's account.
pub struct PairingManager {
    auth: Arc<AuthService>,
    /// The REST client with the pairing service's own policy.
    rest: DcgRest,
    config: PairingConfig,
    clock: Arc<dyn Clock>,
    reachability: Arc<dyn Reachability>,
    host: Arc<dyn HostInfo>,
    state: Mutex<ManagerState>,
}

#[derive(Default)]
struct ManagerState {
    numbers: u64,
    running: Option<Running>,
    /// The last step of the last session, with its number.
    last: Option<(u64, PairingStep)>,
}

impl PairingManager {
    pub fn new(
        auth: Arc<AuthService>,
        config: PairingConfig,
        clock: Arc<dyn Clock>,
        reachability: Arc<dyn Reachability>,
        host: Arc<dyn HostInfo>,
    ) -> Arc<Self> {
        let rest = auth.rest().clone().with_policy(config.rest.clone());
        Arc::new(Self { auth, rest, config, clock, reachability, host, state: Mutex::new(ManagerState::default()) })
    }

    /// The last step of the current or last session, with the session's number.
    pub fn last_step(&self) -> Option<(u64, PairingStep)> {
        self.state.lock_or_panic().last.clone()
    }

    /// The sign-in name of the account a session pairs the phone to.
    pub fn account_name(&self) -> Option<String> {
        self.auth.msa().account_sign_in_name()
    }

    /// Starts a session of `flow`, cancelling the one in progress first, and returns its number.
    /// Every step goes to `observer`. The PC must be signed in and enrolled. The account's token is
    /// fetched once, for every call of the session; without one the session ends
    /// (`AccountTokenUnavailable`) and sends nothing.
    pub fn start(
        self: &Arc<Self>,
        flow: PairingFlow,
        lookup: Arc<dyn PhoneLookup>,
        observer: PairingObserver,
    ) -> Result<u64, PairingStartError> {
        let env = self.auth.environment().ok_or(PairingStartError::NotSignedIn)?;
        if !self.auth.account_info().is_some_and(|a| a.state == AccountState::Enrolled) {
            return Err(PairingStartError::NotSignedIn);
        }

        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let number = {
            let mut st = self.state.lock_or_panic();
            if let Some(previous) = st.running.take() {
                debug!(sequence = previous.number, "a new session cancels the one in progress");
                if let Some(cancel) = previous.cancel {
                    let _ = cancel.send(());
                }
            }
            st.numbers += 1;
            let number = st.numbers;
            st.running = Some(Running { number, commands: commands_tx, cancel: Some(cancel_tx) });
            number
        };

        let me = self.clone();
        let emit = {
            let me = me.clone();
            let observer = observer.clone();
            move |step: PairingStep| {
                {
                    let mut st = me.state.lock_or_panic();
                    if matches!(step, PairingStep::Ended(_)) && st.running.as_ref().is_some_and(|r| r.number == number)
                    {
                        st.running = None;
                    }
                    if st.last.as_ref().is_none_or(|(n, _)| *n <= number) {
                        st.last = Some((number, step.clone()));
                    }
                }
                observer(number, step);
            }
        };

        tokio::spawn(async move {
            let msa = match me.auth.msa().access_token(env).await {
                Ok(token) => token,
                Err(e) => {
                    let ending = Ending::new(PairingResult::AccountTokenUnavailable, Cause::Token(e));
                    ending.log();
                    emit(PairingStep::Ended(ending.result));
                    return;
                }
            };

            let region = me.host.region_code().map(|r| r.to_ascii_uppercase()).filter(|r| !r.is_empty());
            let calls = Arc::new(RestCalls {
                rest: me.rest.clone(),
                env,
                msa,
                region,
                trace: TraceContext::new_root(Vec::new()),
            });
            let session = Session {
                config: me.config.clone(),
                flow,
                calls,
                lookup,
                clock: me.clock.clone(),
                reachability: me.reachability.clone(),
                emit: Box::new(emit.clone()),
            };

            tokio::select! {
                _ = session.run(commands_rx) => {}
                _ = cancel_rx => {
                    Ending::from(PairingResult::UserCanceled).log();
                    emit(PairingStep::Ended(PairingResult::UserCanceled));
                }
            }
        });
        Ok(number)
    }

    /// Asks the running session for a new challenge PIN; `false` when no session runs. A session not
    /// waiting for a refresh ignores it.
    pub fn refresh_challenge_pin(&self) -> bool {
        let st = self.state.lock_or_panic();
        st.running.as_ref().is_some_and(|r| r.commands.send(Command::Refresh).is_ok())
    }

    /// Cancels the running session, if any (local only: the service has no cancel call).
    pub fn cancel(&self) {
        let running = self.state.lock_or_panic().running.as_mut().and_then(|r| r.cancel.take());
        if let Some(cancel) = running {
            let _ = cancel.send(());
        }
    }
}

#[cfg(feature = "test-support")]
/// Runs one session with the given parts (tests and embedders): `emit` gets every step; send on the
/// returned sender to ask for a challenge-PIN refresh; drop it to cancel a session waiting for one.
pub fn run_session(session: Session) -> (mpsc::UnboundedSender<()>, tokio::task::JoinHandle<PairingResult>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            if commands_tx.send(Command::Refresh).is_err() {
                break;
            }
        }
    });
    (tx, tokio::spawn(session.run(commands_rx)))
}
