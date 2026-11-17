//! Running a watcher on a thread of its own for as long as the app runs, started again with back-off.

use std::thread;
use std::time::{Duration, Instant};

use tracing::warn;

#[cfg(test)]
mod tests;

/// Runs `watch` on a thread named after `kind` for as long as the app runs, and starts it again with
/// back-off whenever it returns: its connection ended, or it failed.
pub(super) fn supervise(kind: &'static str, mut watch: impl FnMut() -> zbus::Result<()> + Send + 'static) {
    let spawned = thread::Builder::new().name(format!("{kind}-watch")).spawn(move || {
        let mut backoff = Backoff::default();
        loop {
            let started = Instant::now();
            let outcome = watch();
            let delay = backoff.after_run(started.elapsed());
            match outcome {
                Ok(()) => warn!(
                    kind,
                    delay_secs = delay.as_secs(),
                    "a system event watcher lost its bus connection; it starts again"
                ),
                Err(e) => {
                    warn!(kind, error = %e, delay_secs = delay.as_secs(), "a system event watcher failed; it starts again");
                }
            }
            thread::sleep(delay);
        }
    });
    if let Err(e) = spawned {
        warn!(kind, error = %e, "a system event watcher did not start");
    }
}

/// The wait before a watcher starts again: a second after its first failure, doubling with each
/// failure in a row up to a minute, and a second again once a run lasted a minute.
#[derive(Debug)]
struct Backoff {
    next: Duration,
}

impl Backoff {
    const FIRST: Duration = Duration::from_secs(1);
    const LONGEST: Duration = Duration::from_secs(60);
    /// A run at least this long was healthy: the failure that ended it starts the back-off over.
    const HEALTHY_RUN: Duration = Duration::from_secs(60);

    /// The wait after a run that lasted `ran_for`.
    fn after_run(&mut self, ran_for: Duration) -> Duration {
        if ran_for >= Self::HEALTHY_RUN {
            self.next = Self::FIRST;
        }
        let delay = self.next;
        self.next = (self.next * 2).min(Self::LONGEST);
        delay
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}
