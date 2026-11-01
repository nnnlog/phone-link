//! The PC's own status for the phone's `/remotedevicestatus` request (phone → PC): the macOS sources
//! as an OS edge, and the `DeviceStatusResponseMessage` the engine builds from them.
//!
//! The Wi-Fi bar mapping stays in Rust.

use tracing::warn;

use phonelink_core::edge::EdgeError;
use phonelink_proto::device_info::v1::{
    DeviceStatus, DeviceStatusResponseMessage, DeviceStatusResponseStatus, NetworkType,
};

/// The Mac's internal battery, as `IOKit`'s power-source description reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct PcBattery {
    /// `kIOPSCurrentCapacityKey`.
    pub current_capacity: i32,
    /// `kIOPSMaxCapacityKey`.
    pub max_capacity: i32,
    /// `kIOPSPowerSourceStateKey` is `kIOPSACPowerValue` (charging, or idle on external power).
    pub on_ac_power: bool,
}

/// The kind of the Mac's current internet path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PcInternetPath {
    /// No satisfied path.
    Unavailable,
    /// Satisfied over Wi-Fi (`usesInterfaceType(.wifi)`).
    WiFi,
    /// Satisfied over cellular (`usesInterfaceType(.cellular)`).
    Cellular,
    /// Satisfied over wired Ethernet (`usesInterfaceType(.wiredEthernet)`).
    WiredEthernet,
    /// Satisfied over any other interface.
    Other,
}

/// Where the PC's status comes from (macOS: `IOKit` power sources, `ProcessInfo`, `NWPathMonitor`,
/// `CoreWLAN`). Synchronous: the receiver calls it off the async runtime.
#[uniffi::export(with_foreign)]
pub trait PcStatusSource: Send + Sync {
    /// The internal battery (macOS: `IOPSCopyPowerSourcesInfo` / `IOPSCopyPowerSourcesList` and the
    /// description of the source whose `kIOPSTypeKey` is `kIOPSInternalBatteryType`); `None` when the
    /// Mac has none.
    fn internal_battery(&self) -> Result<Option<PcBattery>, EdgeError>;
    /// Low Power Mode is on (macOS: `ProcessInfo.processInfo.isLowPowerModeEnabled`).
    fn low_power_mode(&self) -> Result<bool, EdgeError>;
    /// The current path of an `NWPathMonitor` (satisfied or not, and its interface type).
    fn internet_path(&self) -> Result<PcInternetPath, EdgeError>;
    /// The RSSI, in dBm, of the connected Wi-Fi network (macOS: `CoreWLAN`
    /// `CWWiFiClient.shared().interface()?.rssiValue()`); `None` when none is available (no
    /// interface, not associated, or the value is withheld).
    fn wifi_rssi(&self) -> Result<Option<i32>, EdgeError>;
}

/// `CoreWLAN`'s RSSI as the `0…4` bars of `wifi_signal_level`: `0` at ≤ −100 dBm, `4` at ≥ −55 dBm,
/// otherwise `⌊(rssi + 100) × 4 ⁄ 45⌋`.
pub fn rssi_to_bars(rssi_dbm: i32) -> i32 {
    if rssi_dbm <= -100 {
        0
    } else if rssi_dbm >= -55 {
        4
    } else {
        (rssi_dbm + 100) * 4 / 45
    }
}

/// The battery's current capacity as an integer percentage of its maximum (rounded down, within
/// `0…100`); `None` when the maximum is not positive, which counts as no battery.
fn battery_percent(battery: &PcBattery) -> Option<i32> {
    if battery.max_capacity <= 0 {
        return None;
    }
    let percent = i64::from(battery.current_capacity.max(0)) * 100 / i64::from(battery.max_capacity);
    Some(percent.min(100) as i32)
}

/// The PC's `DeviceStatus`.
///
/// Battery fields only with a battery (`battery_percent = -1` without one), the network type of the
/// internet path (`WIFI` only with an RSSI, with its bars), every other field unset.
pub fn read_pc_status(source: &dyn PcStatusSource) -> Result<DeviceStatus, EdgeError> {
    let mut info = DeviceStatus::default();
    match source.internal_battery()?.as_ref().and_then(|b| battery_percent(b).map(|p| (b, p))) {
        Some((battery, percent)) => {
            info.battery_percent = percent;
            info.battery_charging = battery.on_ac_power;
            info.battery_power_save = source.low_power_mode()?;
        }
        None => info.battery_percent = -1,
    }

    match source.internet_path()? {
        PcInternetPath::WiFi => {
            if let Some(rssi) = source.wifi_rssi()? {
                info.set_network_type(NetworkType::Wifi);
                info.wifi_signal_level = rssi_to_bars(rssi);
            }
        }
        PcInternetPath::Cellular => info.set_network_type(NetworkType::Cellular),
        PcInternetPath::WiredEthernet => info.set_network_type(NetworkType::Ethernet),
        PcInternetPath::Unavailable | PcInternetPath::Other => {}
    }
    Ok(info)
}

/// The answer to `/remotedevicestatus`: `OK` with the PC's status, or `UNKNOWN_ERROR` without `info`
/// when reading it fails.
pub fn pc_status_response(source: &dyn PcStatusSource) -> DeviceStatusResponseMessage {
    match read_pc_status(source) {
        Ok(info) => DeviceStatusResponseMessage { status: DeviceStatusResponseStatus::Ok as i32, info: Some(info) },
        Err(e) => {
            warn!(error = %e, "PC status not read; answered UNKNOWN_ERROR");
            unknown_error_response()
        }
    }
}

pub(crate) fn unknown_error_response() -> DeviceStatusResponseMessage {
    DeviceStatusResponseMessage { status: DeviceStatusResponseStatus::UnknownError as i32, info: None }
}

/// Stand-ins for this module's OS edges that the headless CLI runs with.
pub mod standins {
    use super::{PcBattery, PcInternetPath, PcStatusSource};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Mutex;

    /// What [`FixedPcStatus`] reports.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct PcState {
        pub battery: Option<PcBattery>,
        pub low_power_mode: bool,
        pub path: PcInternetPath,
        pub wifi_rssi: Option<i32>,
        /// Every read fails.
        pub failing: bool,
    }

    impl Default for PcState {
        /// A desktop Mac on wired Ethernet.
        fn default() -> Self {
            Self {
                battery: None,
                low_power_mode: false,
                path: PcInternetPath::WiredEthernet,
                wifi_rssi: None,
                failing: false,
            }
        }
    }

    /// A PC status the test sets.
    #[derive(Debug, Default)]
    pub struct FixedPcStatus(Mutex<PcState>);

    impl FixedPcStatus {
        /// A PC status edge that reports `state` until [`FixedPcStatus::set`] changes it.
        pub fn new(state: PcState) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self(Mutex::new(state)))
        }

        /// Changes the reported state with `f`.
        pub fn set(&self, f: impl FnOnce(&mut PcState)) {
            f(&mut self.0.lock_or_panic());
        }

        fn read<T>(&self, f: impl FnOnce(&PcState) -> T) -> Result<T, EdgeError> {
            let state = self.0.lock_or_panic();
            if state.failing {
                return Err(EdgeError::Unexpected { message: "PC status unavailable".into() });
            }
            Ok(f(&state))
        }
    }

    impl PcStatusSource for FixedPcStatus {
        fn internal_battery(&self) -> Result<Option<PcBattery>, EdgeError> {
            self.read(|s| s.battery)
        }
        fn low_power_mode(&self) -> Result<bool, EdgeError> {
            self.read(|s| s.low_power_mode)
        }
        fn internet_path(&self) -> Result<PcInternetPath, EdgeError> {
            self.read(|s| s.path)
        }
        fn wifi_rssi(&self) -> Result<Option<i32>, EdgeError> {
            self.read(|s| s.wifi_rssi)
        }
    }
}

#[cfg(test)]
mod tests;
