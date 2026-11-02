//! The options of the `calling` feature, at their defaults.

/// The options of call history, at their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallingConfig {
    /// A call-log metadata message asks for every id it lists, changed or not (default `true`), as
    /// Phone Link on Windows does; `false` asks only for the ids that are new or whose last-update
    /// time changed.
    pub request_every_listed_id: bool,
}

impl Default for CallingConfig {
    fn default() -> Self {
        Self { request_every_listed_id: true }
    }
}
