//! The options of the `calling` feature, at their defaults.

use std::time::Duration;

/// The options of call history and call control, at their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallingConfig {
    /// A call-log metadata message asks for every id it lists, changed or not (default `true`), as
    /// Phone Link on Windows does; `false` asks only for the ids that are new or whose last-update
    /// time changed.
    pub request_every_listed_id: bool,
    /// How long opening the hands-free channel to the phone waits (15 s).
    pub control_open_timeout: Duration,
    /// How long a hands-free command waits for the phone's final result (10 s).
    pub control_command_timeout: Duration,
    /// How long one read of the hands-free channel waits before it reads again (1 s).
    pub control_read_poll: Duration,
    /// How long ending a held call waits for it to resume first (3 s).
    pub control_end_held_wait: Duration,
}

impl Default for CallingConfig {
    fn default() -> Self {
        Self {
            request_every_listed_id: true,
            control_open_timeout: Duration::from_secs(15),
            control_command_timeout: Duration::from_secs(10),
            control_read_poll: Duration::from_secs(1),
            control_end_held_wait: Duration::from_secs(3),
        }
    }
}
