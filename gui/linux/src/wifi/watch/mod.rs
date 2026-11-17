//! Following the network the computer is on: `NetworkManager`'s property changes of its Wi-Fi
//! devices.

use std::collections::HashMap;
use std::thread;

use tracing::{info, warn};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::{ObjectPath, OwnedValue};

use super::{NM, NM_DEVICE, NM_DEVICE_TYPE_WIFI, NM_WIRELESS, NetworkManagerWifi, nm_proxy};
use crate::dbus::PROPERTIES;

#[cfg(test)]
mod tests;

const NM_DEVICES_PATH: &str = "/org/freedesktop/NetworkManager/Devices";

/// What a property change of a `NetworkManager` device says about the network the computer is on.
#[derive(Debug, PartialEq, Eq)]
enum DeviceChange {
    /// A Wi-Fi device moved to another network, or to none.
    AccessPoint,
    /// A device's connection came or went (a Wi-Fi device's is a change of network).
    Connection,
    Other,
}

fn device_change(interface: &str, changed: &HashMap<String, OwnedValue>) -> DeviceChange {
    match interface {
        NM_WIRELESS if changed.contains_key("ActiveAccessPoint") => DeviceChange::AccessPoint,
        NM_DEVICE if changed.contains_key("ActiveConnection") => DeviceChange::Connection,
        _ => DeviceChange::Other,
    }
}

/// The device at `path` is a Wi-Fi device.
fn is_wifi_device(bus: &Connection, path: &ObjectPath<'_>) -> bool {
    match nm_proxy(bus, path.to_owned(), NM_DEVICE).and_then(|device| device.get_property::<u32>("DeviceType")) {
        Ok(kind) => kind == NM_DEVICE_TYPE_WIFI,
        Err(e) => {
            info!(error = %e, "a device whose connection changed was not read");
            false
        }
    }
}

/// A Wi-Fi device joined, left or changed networks, from one of `NetworkManager`'s property changes.
fn network_changed(bus: &Connection, message: &Message) -> bool {
    let header = message.header();
    let Some(path) = header.path() else { return false };
    let (interface, changed, _invalidated): (String, HashMap<String, OwnedValue>, Vec<String>) =
        match message.body().deserialize() {
            Ok(body) => body,
            Err(e) => {
                warn!(error = %e, "a NetworkManager property change did not read");
                return false;
            }
        };

    match device_change(&interface, &changed) {
        DeviceChange::AccessPoint => true,
        DeviceChange::Connection => is_wifi_device(bus, path),
        DeviceChange::Other => false,
    }
}

impl NetworkManagerWifi {
    /// Calls `changed` on a thread of its own whenever a Wi-Fi device joins, leaves or changes
    /// networks, so what depends on the network the computer is on is read again.
    pub fn watch_network(&self, changed: impl Fn() + Send + 'static) {
        let Some(bus) = self.bus.clone() else {
            warn!("no system bus; Wi-Fi network changes are not followed");
            return;
        };

        let signals = Connection::system().and_then(|watch| {
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .sender(NM)?
                .interface(PROPERTIES)?
                .member("PropertiesChanged")?
                .path_namespace(NM_DEVICES_PATH)?
                .build();
            MessageIterator::for_match_rule(rule, &watch, None)
        });
        let signals = match signals {
            Ok(signals) => signals,
            Err(e) => {
                warn!(error = %e, "Wi-Fi network changes are not followed");
                return;
            }
        };

        let spawned = thread::Builder::new().name("wifi-watch".to_owned()).spawn(move || {
            for message in signals {
                match message {
                    Ok(message) => {
                        if network_changed(&bus, &message) {
                            changed();
                        }
                    }
                    Err(e) => warn!(error = %e, "a NetworkManager signal did not read"),
                }
            }
            warn!("Wi-Fi network changes are no longer followed");
        });
        if let Err(e) = spawned {
            warn!(error = %e, "Wi-Fi network changes are not followed");
        }
    }
}
