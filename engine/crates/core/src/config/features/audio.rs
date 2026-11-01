//! The options and timers of the `audio` feature, at their defaults.

use std::time::Duration;

/// The feature's timers, at their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    /// The busy indicator after a command lasts until this long after its request finished (20 s).
    pub command_busy: Duration,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { command_busy: Duration::from_secs(20) }
    }
}
