//! The options and timers of the `bluetooth` feature, at their defaults.

use std::time::Duration;

/// The pairing's timers.
#[derive(Debug, Clone)]
pub struct BluetoothConfig {
    /// The Bluetooth and `BluetoothAdvertise` `/permission` requests' timeout (45 s).
    pub permission_timeout: Duration,
    /// The Bluetooth discovery's bound (30 s).
    pub discovery_timeout: Duration,
    /// How long a service-record refresh waits for the records to include the service (15 s).
    pub service_refresh_timeout: Duration,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self {
            permission_timeout: Duration::from_secs(45),
            discovery_timeout: Duration::from_secs(30),
            service_refresh_timeout: Duration::from_secs(15),
        }
    }
}
