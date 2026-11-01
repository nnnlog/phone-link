//! The options and timers of the `screen_mirroring` feature, at their defaults.

use std::time::Duration;

/// Every option and timer of the feature, at its default.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenMirroringConfig {
    /// `config.maxMainDisplayFps` 60 (default off: 30 for the phone screen only, else no cap).
    pub max_fps_60: bool,
    /// A phone app's secure content replaces its window (default off: logged only).
    pub secure_content: bool,
    pub link: LinkTimers,
    /// A window loading or unstable this long shows error 37; the phone connecting or unstable this
    /// long goes to error 0 (`100` s).
    pub watchdog: Duration,
    /// Video starts only when the window is ready within this (`10` s).
    pub window_wait: Duration,
    /// The wait for a notification launch's result (`10` s).
    pub notification_result: Duration,
    /// The `nearby` and `earlyLaunch` channels must open within this (`30` s).
    pub channel_open: Duration,
    /// The nearby verification code expires after this (`10` min).
    pub pin_code_expiry: Duration,
    /// The audio frame length until the phone says otherwise (`40` ms).
    pub frame_length_ms: u32,
}

impl Default for ScreenMirroringConfig {
    fn default() -> Self {
        Self {
            max_fps_60: false,
            secure_content: false,
            link: LinkTimers::default(),
            watchdog: Duration::from_secs(100),
            window_wait: Duration::from_secs(10),
            notification_result: Duration::from_secs(10),
            channel_open: Duration::from_secs(30),
            pin_code_expiry: Duration::from_secs(600),
            frame_length_ms: 40,
        }
    }
}

// ------------------------------------------------------------------------------------------------
// The workflows on one connection
// ------------------------------------------------------------------------------------------------

/// The timers of the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkTimers {
    /// The whole connection attempt, `launcher` included (`90` s → 257).
    pub connect: Duration,
    /// With no workflow live, the connection closes after this (`120` s).
    pub idle_release: Duration,
    /// The `launcher` channel closed: the connection closes after this (`1` s).
    pub launcher_closed: Duration,
    /// A start on a new connection waits this long after closing an idle one (`500` ms).
    pub new_connection_pause: Duration,
}

impl Default for LinkTimers {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(90),
            idle_release: Duration::from_secs(120),
            launcher_closed: Duration::from_secs(1),
            new_connection_pause: Duration::from_millis(500),
        }
    }
}
