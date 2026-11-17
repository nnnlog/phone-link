//! Calls run on a thread of their own against a deadline, for the `BlueZ` calls that can take longer
//! than their caller waits.

use std::sync::mpsc;
use std::thread;
use std::time::Instant;

/// How a call run against a deadline ended.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Bounded<T> {
    Finished(T),
    /// The deadline came first; the call's thread ends with the call, within the method timeout.
    DeadlinePassed,
}

/// Why a call run against a deadline has no outcome.
///
/// The spawn error travels as its text, so that the outcome of a call compares (`Eq`).
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum BoundedCallError {
    /// The call's thread did not start.
    #[error("the {name} thread did not start: {reason}")]
    NotStarted { name: &'static str, reason: String },
    /// The call's thread ended without sending the call's result: the call panicked.
    #[error("the {name} thread ended without a result")]
    NoResult { name: &'static str },
}

/// Runs `call` on a thread named `name` and waits for its result until `deadline`.
pub(super) fn call_until<T: Send + 'static>(
    deadline: Instant,
    name: &'static str,
    call: impl FnOnce() -> T + Send + 'static,
) -> Result<Bounded<T>, BoundedCallError> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            // The caller gave up when nobody receives.
            let _ = sender.send(call());
        })
        .map_err(|e| BoundedCallError::NotStarted { name, reason: e.to_string() })?;

    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => Ok(Bounded::Finished(result)),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(Bounded::DeadlinePassed),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(BoundedCallError::NoResult { name }),
    }
}
