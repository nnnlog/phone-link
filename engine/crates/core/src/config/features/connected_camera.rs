//! The options and timers of the `connected_camera` feature, at their defaults.

use std::time::Duration;

/// Every option and timer of the feature, at its default.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectedCameraConfig {
    /// A stream that needs the phone's permissions asks the phone to prompt for them (default on).
    pub auto_prompt: bool,
    /// Several phones may be cameras at once (default off: one at a time).
    pub multiple_cameras: bool,
    /// The wait for the phone's context handle (`120` s).
    pub handle_timeout: Duration,
    /// The pause after each handled client start or stop (`500` ms).
    pub client_signal_pause: Duration,
    /// How often the network quality is read while streaming (`10` s).
    pub network_poll: Duration,
    /// The packet-loss rate from which the network counts as poor (`0.5`).
    pub low_loss_rate: f64,
}

impl Default for ConnectedCameraConfig {
    fn default() -> Self {
        Self {
            auto_prompt: true,
            multiple_cameras: false,
            handle_timeout: Duration::from_secs(120),
            client_signal_pause: Duration::from_millis(500),
            network_poll: Duration::from_secs(10),
            low_loss_rate: 0.5,
        }
    }
}
