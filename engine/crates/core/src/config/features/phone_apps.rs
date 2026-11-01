//! The options and timers of the `phone_apps` feature, at their defaults.

use std::time::Duration;

/// The flags and timers of the phone apps, at their defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct PhoneAppsConfig {
    /// One window per app is turned off (default `false`): *hosted apps* never holds.
    pub separate_window: bool,
    /// The Office desktop-URI rewrite of recent entries (default `false`).
    pub office_desktop_uri_rewrite: bool,
    /// The "syncing" state shows for at most this long (`2` min).
    pub syncing_indicator_max: Duration,
    /// The recent list shows at most this many entries (`3`).
    pub recent_apps_max: usize,
}

impl Default for PhoneAppsConfig {
    fn default() -> Self {
        Self {
            separate_window: false,
            office_desktop_uri_rewrite: false,
            syncing_indicator_max: Duration::from_secs(120),
            recent_apps_max: 3,
        }
    }
}
