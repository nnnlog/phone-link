//! A failure that recurs with every event or item, logged when it starts rather than each time.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether a recurring failure is ongoing, so that its log line is written once until it clears.
#[derive(Debug, Default)]
pub struct OngoingFailure(AtomicBool);

impl OngoingFailure {
    /// Marks the failure as ongoing; `true` the first time since it last cleared.
    pub fn starts(&self) -> bool {
        !self.0.swap(true, Ordering::Relaxed)
    }

    /// Marks the failure as cleared.
    pub fn clear(&self) {
        self.0.store(false, Ordering::Relaxed);
    }
}
