//! This computer's own status for the phone: the battery from sysfs, low power mode from
//! power-profiles-daemon, the internet path from `NetworkManager`, and the Wi-Fi signal from the kernel
//! for the device `NetworkManager` reports.

mod battery;
mod network_manager;
mod power_profiles;
mod wifi_signal;

use std::path::Path;

use battery::system_battery;
use phonelink_device_status::edge::{PcBattery, PcInternetPath, PcStatusSource};
use phonelink_engine::edge::EdgeError;
use wifi_signal::station_signal;
use zbus::blocking::Connection;

use crate::dbus::{Bus, LazyBus};
use crate::edges::unexpected;

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// This computer's own status for the phone, read on each request.
pub struct DesktopPcStatus {
    /// The system bus, connected on first use; a failed connect is tried again on the next read, and a
    /// lost connection is replaced.
    bus: LazyBus,
}

impl DesktopPcStatus {
    pub fn new() -> Self {
        Self { bus: LazyBus::new(Bus::System, None) }
    }

    fn bus(&self) -> Result<Connection, EdgeError> {
        self.bus.connection().map_err(|e| unexpected(format!("no system bus: {e}")))
    }
}

impl PcStatusSource for DesktopPcStatus {
    fn internal_battery(&self) -> Result<Option<PcBattery>, EdgeError> {
        system_battery(Path::new(POWER_SUPPLY)).map_err(|e| unexpected(format!("power supplies: {e}")))
    }

    fn low_power_mode(&self) -> Result<bool, EdgeError> {
        self.power_saver_on()
    }

    fn internet_path(&self) -> Result<PcInternetPath, EdgeError> {
        self.primary_internet_path()
    }

    /// The signal the kernel measures from the access point of the Wi-Fi device `NetworkManager`
    /// reports; `None` without `NetworkManager` on the bus or an associated Wi-Fi device.
    fn wifi_rssi(&self) -> Result<Option<i32>, EdgeError> {
        let Some(interface) = self.reported_wifi_interface()? else {
            return Ok(None);
        };
        station_signal(&interface).map_err(|e| unexpected(format!("the Wi-Fi signal of {interface}: {e}")))
    }
}
