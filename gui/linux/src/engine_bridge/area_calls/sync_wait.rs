//! The wait for calls or contacts to come up to date, while the sections show them as updating.

use std::time::Duration;

/// Calls or contacts that are not up to date show as updating for at most this long.
pub(super) const SYNC_WAIT: Duration = Duration::from_secs(120);

/// The wait for calls or contacts to come up to date.
///
/// While they are not and the wait has not run out, the section shows them as updating.
#[derive(Default)]
pub(super) struct SyncWait {
    generation: u64,
    running: bool,
    pub(super) over: bool,
    up_to_date: Option<bool>,
}

impl SyncWait {
    /// The sync state was read: `Some(generation)` when a wait starts, for [`SyncWait::expire`].
    ///
    /// With `restart`, data that falls behind again starts a new wait.
    pub(super) fn observe(&mut self, up_to_date: bool, restart: bool) -> Option<u64> {
        if restart && self.up_to_date == Some(true) && !up_to_date {
            self.generation += 1;
            self.running = false;
            self.over = false;
        }

        self.up_to_date = Some(up_to_date);
        if up_to_date || self.running || self.over {
            return None;
        }

        self.running = true;
        Some(self.generation)
    }

    /// The wait `generation` ran out; `false` when a newer one replaced it.
    pub(super) fn expire(&mut self, generation: u64) -> bool {
        if generation != self.generation || !self.running {
            return false;
        }
        self.running = false;
        self.over = true;
        true
    }
}

/// Which wait ran out.
#[derive(Debug, Clone, Copy)]
pub(super) enum Synced {
    Calls,
    Contacts,
}
