//! The options and timers of the `remote_snipping` feature, at their defaults.

use std::time::Duration;

/// Every option and timer of the feature, at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSnippingConfig {
    /// The feature option (default on): off means no subscription, no request, no notification.
    pub enabled: bool,
    /// A new photo is notified while its last-modified time is less than this before now (`120` s).
    pub recent_window: Duration,
    /// At most one notification attempt per this interval, over every phone (`1` s).
    pub rate_limit: Duration,
    /// A notification within this time of the phone's previous pop-up is delivered silently (`7` s).
    pub silent_window: Duration,
    /// A posted notification is withdrawn after this long (`5` min).
    pub expiry: Duration,
}

impl Default for RemoteSnippingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            recent_window: Duration::from_secs(120),
            rate_limit: Duration::from_secs(1),
            silent_window: Duration::from_secs(7),
            expiry: Duration::from_secs(300),
        }
    }
}
