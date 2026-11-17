//! The internet path and the Wi-Fi device to report, from `NetworkManager` on the system bus.

use phonelink_device_status::edge::PcInternetPath;
use phonelink_engine::edge::EdgeError;
use zbus::blocking::Connection;
use zbus::names::BusName;
use zbus::zvariant::OwnedObjectPath;

use super::DesktopPcStatus;
use crate::dbus::{UNKNOWN_OBJECT, dbus_error_is, left_the_bus, name_owner, uncached};
use crate::edges::unexpected;

#[cfg(test)]
mod tests;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_DEVICE_TYPE_WIFI: u32 = 2;
/// `NM_802_11_MODE_INFRA`: a client of an access point, rather than a hotspot or an ad-hoc or mesh
/// node.
const NM_802_11_MODE_INFRA: u32 = 2;
/// `NM_STATE_CONNECTED_LOCAL`, the lowest of `NetworkManager`'s connected states.
const NM_STATE_CONNECTED_LOCAL: u32 = 50;
/// What `NetworkManager` puts in an object-path property that names no object.
const NO_OBJECT: &str = "/";

impl DesktopPcStatus {
    /// Runs `read` against `NetworkManager`'s own connection on the bus; `None` when `NetworkManager` is
    /// not on the bus, or left it during the read.
    fn read_network_manager<T>(
        &self,
        read: impl FnOnce(&Connection, BusName<'static>) -> zbus::Result<T>,
    ) -> Result<Option<T>, EdgeError> {
        let bus = self.bus()?;

        match name_owner(&bus, NM).and_then(|owner| owner.map(|owner| read(&bus, owner)).transpose()) {
            Err(e) if left_the_bus(&e) => Ok(None),
            Err(e) => {
                self.bus.after_error(&e);
                Err(unexpected(format!("NetworkManager: {e}")))
            }
            Ok(value) => Ok(value),
        }
    }

    /// `Other` while `NetworkManager` is not on the bus: the path is then not known to be any of the
    /// kinds the phone is told.
    pub(super) fn primary_internet_path(&self) -> Result<PcInternetPath, EdgeError> {
        let path = self.read_network_manager(|bus, nm| {
            let manager = uncached(bus, nm, NM_PATH, NM)?;
            let state: u32 = manager.get_property("State")?;
            let primary: OwnedObjectPath = manager.get_property("PrimaryConnection")?;
            // The manager mirrors the primary connection's `Type` as `PrimaryConnectionType`.
            let kind: String = manager.get_property("PrimaryConnectionType")?;
            Ok(internet_path_of(state, primary.as_str(), &kind))
        })?;
        Ok(path.unwrap_or(PcInternetPath::Other))
    }

    /// The kernel interface of the Wi-Fi device to report; `None` without `NetworkManager` on the bus or
    /// an associated Wi-Fi device.
    pub(super) fn reported_wifi_interface(&self) -> Result<Option<String>, EdgeError> {
        Ok(self.read_network_manager(associated_wifi_interface)?.flatten())
    }
}

/// The kernel interface of the Wi-Fi device to report: among the devices that are clients of an
/// access point, the one that carries the primary connection, else the first; `None` without one. A
/// device removed while it is read is left out.
fn associated_wifi_interface(bus: &Connection, nm: BusName<'static>) -> zbus::Result<Option<String>> {
    let manager = uncached(bus, nm.clone(), NM_PATH, NM)?;
    let primary: OwnedObjectPath = manager.get_property("PrimaryConnection")?;
    let devices: Vec<OwnedObjectPath> = manager.call("GetDevices", &())?;

    let mut associated = Vec::new();
    for device in &devices {
        match associated_wifi_device(bus, &nm, device) {
            Ok(Some(found)) => associated.push(found),
            Ok(None) => {}
            Err(e) if dbus_error_is(&e, &[UNKNOWN_OBJECT]) => {}
            Err(e) => return Err(e),
        }
    }

    Ok(reported_interface(&primary, &associated).map(str::to_owned))
}

/// The active connection and the kernel interface of `device` while it is a Wi-Fi device associated
/// with an access point as its client.
fn associated_wifi_device(
    bus: &Connection,
    nm: &BusName<'static>,
    device: &OwnedObjectPath,
) -> zbus::Result<Option<(OwnedObjectPath, String)>> {
    let general = uncached(bus, nm.clone(), device, NM_DEVICE)?;
    if general.get_property::<u32>("DeviceType")? != NM_DEVICE_TYPE_WIFI {
        return Ok(None);
    }
    let wireless = uncached(bus, nm.clone(), device, NM_WIRELESS)?;
    if wireless.get_property::<u32>("Mode")? != NM_802_11_MODE_INFRA {
        return Ok(None);
    }
    let access_point: OwnedObjectPath = wireless.get_property("ActiveAccessPoint")?;
    if access_point.as_str() == NO_OBJECT {
        return Ok(None);
    }
    Ok(Some((general.get_property("ActiveConnection")?, general.get_property("Interface")?)))
}

/// The interface to report among the associated Wi-Fi devices, each given as its `(active connection,
/// interface)`: the one that carries the primary connection, else the first.
fn reported_interface<'a>(primary: &OwnedObjectPath, associated: &'a [(OwnedObjectPath, String)]) -> Option<&'a str> {
    associated
        .iter()
        .find(|(connection, _)| connection == primary)
        .or_else(|| associated.first())
        .map(|(_, interface)| interface.as_str())
}

/// The internet path from `NetworkManager`'s `State`, `PrimaryConnection` and the primary connection's
/// type: none without a primary connection or below `NM_STATE_CONNECTED_LOCAL`. Only mobile broadband
/// is cellular; a phone's connection shared over Bluetooth is another kind.
fn internet_path_of(state: u32, primary: &str, connection_type: &str) -> PcInternetPath {
    if primary == NO_OBJECT || state < NM_STATE_CONNECTED_LOCAL {
        return PcInternetPath::Unavailable;
    }

    match connection_type {
        "802-11-wireless" => PcInternetPath::WiFi,
        "802-3-ethernet" => PcInternetPath::WiredEthernet,
        "gsm" | "cdma" => PcInternetPath::Cellular,
        _ => PcInternetPath::Other,
    }
}
