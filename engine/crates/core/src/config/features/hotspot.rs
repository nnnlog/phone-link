//! The options and timers of the `hotspot` feature, at their defaults.

use std::time::Duration;

/// Instant hotspot's timers and limits (the pairing's are the shared
/// [`BluetoothConfig`](super::BluetoothConfig)).
#[derive(Debug, Clone)]
pub struct HotspotConfig {
    /// How long the connect waits for consent and the two Bluetooth permissions (60 s).
    pub consent_wait: Duration,
    /// The RFCOMM service lookup's bound (30 s).
    pub service_lookup_timeout: Duration,
    /// How long a sent request waits for the reply (10 s).
    pub reply_timeout: Duration,
    /// Attempts after the first for a transport failure (2).
    pub retries: u32,
    /// The wait before the second scan when the hotspot is not yet visible (5 s).
    pub not_visible_retry_delay: Duration,
}

impl Default for HotspotConfig {
    fn default() -> Self {
        Self {
            consent_wait: Duration::from_secs(60),
            service_lookup_timeout: Duration::from_secs(30),
            reply_timeout: Duration::from_secs(10),
            retries: 2,
            not_visible_retry_delay: Duration::from_secs(5),
        }
    }
}
