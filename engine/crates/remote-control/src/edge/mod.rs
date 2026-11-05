//! The Mac's power and session actions as an OS edge: lock the screen, sleep, shut down, restart.
//!
//! The engine decides whether to act; the edge only acts and reports whether the request was made.
//! The tests use [`standins::RecordingPowerControl`], which never touches the machine; the CLI uses
//! [`RefusingPowerControl`].

use std::sync::Arc;

use tracing::{info, warn};

use phonelink_core::edge::EdgeError;

/// What the phone asks the Mac to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerAction {
    LockScreen,
    Sleep,
    ShutDown,
    Restart,
}

/// The Mac's power and session actions (macOS: the login session's lock, `IOKit` system sleep, an
/// Apple event to `loginwindow`).
///
/// Synchronous: the engine calls it off the async runtime. Each call
/// returns once the request is made — a shutdown or restart is not finished, and applications may
/// still cancel it — and fails when the request could not be made.
#[uniffi::export(with_foreign)]
pub trait PcPowerControl: Send + Sync {
    /// Locks the login session now, whatever the "require password" setting says.
    fn lock_screen(&self) -> Result<(), EdgeError>;
    /// Puts the Mac to sleep.
    fn sleep(&self) -> Result<(), EdgeError>;
    /// Shuts the Mac down the way the Apple menu's Shut Down does, without its confirmation.
    fn shut_down(&self) -> Result<(), EdgeError>;
    /// Restarts the Mac the way the Apple menu's Restart does, without its confirmation.
    fn restart(&self) -> Result<(), EdgeError>;
}

/// Runs `action` on `power` on the blocking pool; `true` when the edge reports the request made.
/// A failing or panicking edge is logged and is `false`; what the phone hears is the caller's.
pub(crate) async fn perform(power: Arc<dyn PcPowerControl>, action: PowerAction) -> bool {
    let outcome = tokio::task::spawn_blocking(move || match action {
        PowerAction::LockScreen => power.lock_screen(),
        PowerAction::Sleep => power.sleep(),
        PowerAction::ShutDown => power.shut_down(),
        PowerAction::Restart => power.restart(),
    })
    .await;

    match outcome {
        Ok(Ok(())) => {
            info!(?action, "power action requested from the Mac");
            true
        }
        Ok(Err(e)) => {
            warn!(?action, error = %e, "power action failed");
            false
        }
        Err(e) => {
            warn!(?action, error = %e, "power action edge panicked");
            false
        }
    }
}

/// The CLI's edge: it has no business powering the host down, so every action fails.
#[derive(Debug, Default)]
pub struct RefusingPowerControl;

impl RefusingPowerControl {
    fn refuse(what: &str) -> Result<(), EdgeError> {
        Err(EdgeError::Unexpected { message: format!("this host does not {what} on the phone's request") })
    }
}

impl PcPowerControl for RefusingPowerControl {
    fn lock_screen(&self) -> Result<(), EdgeError> {
        Self::refuse("lock its screen")
    }
    fn sleep(&self) -> Result<(), EdgeError> {
        Self::refuse("sleep")
    }
    fn shut_down(&self) -> Result<(), EdgeError> {
        Self::refuse("shut down")
    }
    fn restart(&self) -> Result<(), EdgeError> {
        Self::refuse("restart")
    }
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{PcPowerControl, PowerAction};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Records every action and acts on nothing; fails every action while `set_failing(true)`.
    #[derive(Debug, Default)]
    pub struct RecordingPowerControl {
        actions: Mutex<Vec<PowerAction>>,
        failing: AtomicBool,
    }

    impl RecordingPowerControl {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        #[cfg(feature = "test-support")]
        /// The actions asked for so far, in order (failed ones included).
        pub fn actions(&self) -> Vec<PowerAction> {
            self.actions.lock_or_panic().clone()
        }

        #[cfg(feature = "test-support")]
        pub fn set_failing(&self, failing: bool) {
            self.failing.store(failing, Ordering::SeqCst);
        }

        fn record(&self, action: PowerAction) -> Result<(), EdgeError> {
            self.actions.lock_or_panic().push(action);
            if self.failing.load(Ordering::SeqCst) {
                return Err(EdgeError::Unexpected { message: format!("{action:?} refused by the test") });
            }
            Ok(())
        }
    }

    impl PcPowerControl for RecordingPowerControl {
        fn lock_screen(&self) -> Result<(), EdgeError> {
            self.record(PowerAction::LockScreen)
        }
        fn sleep(&self) -> Result<(), EdgeError> {
            self.record(PowerAction::Sleep)
        }
        fn shut_down(&self) -> Result<(), EdgeError> {
            self.record(PowerAction::ShutDown)
        }
        fn restart(&self) -> Result<(), EdgeError> {
            self.record(PowerAction::Restart)
        }
    }
}

#[cfg(test)]
mod tests;
