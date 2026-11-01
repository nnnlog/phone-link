//! The options and timers of the `notifications` feature, at their defaults.

use std::time::Duration;

/// Every timer, limit and flag of notifications, at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsConfig {
    /// Queued actions and replies older than this are dropped unsent.
    pub queue_expiry: Duration,
    /// The duplicate check remembers both hashes this long.
    pub dedupe_window: Duration,
    /// Only items whose event time is within this window alert.
    pub banner_recency: Duration,
    /// Per-package banner throttling (default on).
    pub toast_throttling: bool,
    /// The throttling threshold (5 alerts).
    pub throttling_threshold: u32,
    /// The throttling monitoring period (10 s).
    pub throttling_monitoring_period: Duration,
    /// The throttling cooldown (180 s).
    pub throttling_cooldown: Duration,
    /// Quiet by default (default off): the default banner preference of apps without a local
    /// setting, and settings trigger 27.
    pub quiet_by_default: bool,
    /// Mute-all lasts this long (a local-only setting).
    pub mute_all_duration: Duration,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            queue_expiry: Duration::from_secs(10 * 60),
            dedupe_window: Duration::from_secs(20),
            banner_recency: Duration::from_secs(5 * 60),
            toast_throttling: true,
            throttling_threshold: 5,
            throttling_monitoring_period: Duration::from_secs(10),
            throttling_cooldown: Duration::from_secs(180),
            quiet_by_default: false,
            mute_all_duration: Duration::from_secs(24 * 3600),
        }
    }
}
