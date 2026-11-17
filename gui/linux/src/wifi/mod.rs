//! Joining the phone's hotspot and following the computer's Wi-Fi network, through `NetworkManager`
//! on the system bus.

mod join;
mod watch;

use phonelink_hotspot::HotspotWifi;
use phonelink_hotspot::edge::WifiError;
use tracing::info;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath};

use crate::dbus::uncached;

#[cfg(test)]
mod tests;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_DEVICE_TYPE_WIFI: u32 = 2;
/// The first device state in which `NetworkManager` can scan and connect with the device; below it the
/// device is unmanaged or unavailable (as a device whose Wi-Fi is switched off is).
const NM_DEVICE_STATE_DISCONNECTED: u32 = 30;
/// What `NetworkManager` puts in an object-path property that names no object.
const NO_OBJECT: &str = "/";

/// The computer's Wi-Fi through `NetworkManager`.
pub struct NetworkManagerWifi {
    bus: Option<Connection>,
}

fn wifi_err(e: impl std::fmt::Display) -> WifiError {
    WifiError::Failed { reason: format!("NetworkManager: {e}") }
}

fn failed(reason: String) -> WifiError {
    WifiError::Failed { reason }
}

/// A `NetworkManager` proxy that reads every property from `NetworkManager` instead of zbus's cache: a
/// failed activation ends with its object removed, which a cached property would never show.
fn nm_proxy<'a, P>(bus: &Connection, path: P, interface: &'a str) -> zbus::Result<Proxy<'a>>
where
    P: TryInto<ObjectPath<'a>>,
    P::Error: Into<zbus::Error>,
{
    uncached(bus, NM, path, interface)
}

/// Wi-Fi can be used: switched on in `NetworkManager` and by the hardware switch, and the device ready
/// (a device whose Wi-Fi is switched off reads unavailable).
fn wifi_usable(enabled: bool, hardware_enabled: bool, device_state: u32) -> bool {
    enabled && hardware_enabled && device_state >= NM_DEVICE_STATE_DISCONNECTED
}

impl NetworkManagerWifi {
    pub fn new() -> Self {
        Self { bus: Connection::system().ok() }
    }

    fn bus(&self) -> Result<&Connection, WifiError> {
        self.bus.as_ref().ok_or(WifiError::Unavailable)
    }

    fn manager(&self) -> Result<Proxy<'static>, WifiError> {
        nm_proxy(self.bus()?, NM_PATH, NM).map_err(wifi_err)
    }

    /// The first Wi-Fi device `NetworkManager` lists.
    fn device(&self) -> Result<OwnedObjectPath, WifiError> {
        let bus = self.bus()?;
        let devices: Vec<OwnedObjectPath> = self.manager()?.call("GetDevices", &()).map_err(wifi_err)?;
        devices
            .into_iter()
            .find(|path| {
                nm_proxy(bus, path, NM_DEVICE)
                    .and_then(|device| device.get_property::<u32>("DeviceType"))
                    .is_ok_and(|kind| kind == NM_DEVICE_TYPE_WIFI)
            })
            .ok_or(WifiError::Unavailable)
    }

    /// `Unavailable` while Wi-Fi is switched off, in `NetworkManager` or by the hardware switch, or the
    /// device is not ready.
    fn ensure_usable(&self, device: &OwnedObjectPath) -> Result<(), WifiError> {
        let manager = self.manager()?;
        let enabled: bool = manager.get_property("WirelessEnabled").map_err(wifi_err)?;
        let hardware_enabled: bool = manager.get_property("WirelessHardwareEnabled").map_err(wifi_err)?;
        let device_state: u32 = nm_proxy(self.bus()?, device, NM_DEVICE)
            .and_then(|device| device.get_property("State"))
            .map_err(wifi_err)?;
        if wifi_usable(enabled, hardware_enabled, device_state) {
            return Ok(());
        }
        info!(is_enabled = enabled, is_hardware_enabled = hardware_enabled, device_state, "Wi-Fi is off or not ready");
        Err(WifiError::Unavailable)
    }

    fn deactivate(&self, active: &OwnedObjectPath) -> Result<(), WifiError> {
        self.manager()?.call::<_, _, ()>("DeactivateConnection", &(active,)).map_err(wifi_err)
    }
}

impl HotspotWifi for NetworkManagerWifi {
    fn has_interface(&self) -> bool {
        self.device().is_ok()
    }

    fn is_visible(&self, ssid: String) -> Result<bool, WifiError> {
        let device = self.device()?;
        self.ensure_usable(&device)?;
        Ok(self.scan_for(&device, &ssid)?.is_some())
    }

    fn join(&self, ssid: String, passphrase: String) -> Result<(), WifiError> {
        self.join_network(&ssid, &passphrase)
    }

    fn current_ssid(&self) -> Option<String> {
        let access_point: OwnedObjectPath = nm_proxy(self.bus.as_ref()?, self.device().ok()?, NM_WIRELESS)
            .ok()?
            .get_property("ActiveAccessPoint")
            .ok()?;
        (access_point.as_str() != NO_OBJECT).then(|| self.ssid_of(&access_point)).flatten()
    }

    /// Deactivates the Wi-Fi device's active connection. Unlike `Device.Disconnect`, which keeps the
    /// device from autoconnecting until the user connects it again, this lets `NetworkManager` return
    /// to the computer's usual network.
    fn leave(&self) -> Result<(), WifiError> {
        let active: OwnedObjectPath = nm_proxy(self.bus()?, self.device()?, NM_DEVICE)
            .and_then(|device| device.get_property("ActiveConnection"))
            .map_err(wifi_err)?;
        if active.as_str() == NO_OBJECT {
            return Ok(());
        }
        self.deactivate(&active)
    }
}
