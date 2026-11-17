//! Reading `BlueZ`'s objects: the property values the app uses, the adapter in use, the devices on it,
//! the phone among them, and the messaging proximity state they make.

use std::collections::{HashMap, HashSet};

use phonelink_messaging::ProximityState;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use super::bluez::{ADAPTER, DEVICE};
use super::names::names_match;

pub(super) type Properties = HashMap<String, OwnedValue>;
pub(super) type Objects = HashMap<OwnedObjectPath, HashMap<String, Properties>>;

pub(super) fn bool_of(properties: &Properties, name: &str) -> bool {
    properties.get(name).and_then(|value| value.downcast_ref::<bool>().ok()).unwrap_or(false)
}

pub(super) fn string_of(properties: &Properties, name: &str) -> Option<String> {
    properties.get(name).and_then(|value| value.downcast_ref::<&str>().ok()).map(str::to_owned)
}

fn strings_of(properties: &Properties, name: &str) -> Vec<String> {
    properties
        .get(name)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_default()
}

fn path_of(properties: &Properties, name: &str) -> Option<String> {
    properties.get(name).and_then(|value| value.downcast_ref::<ObjectPath>().ok()).map(|path| path.to_string())
}

/// The engine names devices by their address as `xx-xx-xx-xx-xx-xx`; `BlueZ` writes `XX:XX:XX:XX:XX:XX`.
pub(super) fn engine_address(bluez: &str) -> String {
    bluez.replace(':', "-").to_ascii_lowercase()
}

fn is_phone(properties: &Properties, names: &[String]) -> bool {
    [string_of(properties, "Name"), string_of(properties, "Alias")]
        .into_iter()
        .flatten()
        .any(|name| names.iter().any(|wanted| names_match(&name, wanted)))
}

/// The adapter Phone Link uses: a powered one before one that is off, then the one whose object path
/// sorts first, so the radio, the devices, discovery and the proximity state all use the same one.
pub(super) fn chosen_adapter(objects: &Objects) -> Option<(&OwnedObjectPath, &Properties)> {
    objects
        .iter()
        .filter_map(|(path, interfaces)| Some((path, interfaces.get(ADAPTER)?)))
        .min_by_key(|(path, adapter)| (!bool_of(adapter, "Powered"), path.as_str()))
}

/// The devices `BlueZ` knows on `adapter`.
pub(super) fn devices_on<'a>(
    objects: &'a Objects,
    adapter: &str,
) -> impl Iterator<Item = (&'a OwnedObjectPath, &'a Properties)> {
    objects
        .iter()
        .filter_map(|(path, interfaces)| Some((path, interfaces.get(DEVICE)?)))
        .filter(move |(_, device)| path_of(device, "Adapter").as_deref() == Some(adapter))
}

/// The Bluetooth Classic devices on `adapter`: those with a class of device, which a device seen only
/// over Bluetooth Low Energy does not have.
fn classic_devices<'a>(
    objects: &'a Objects,
    adapter: &str,
) -> impl Iterator<Item = (&'a OwnedObjectPath, &'a Properties)> {
    devices_on(objects, adapter).filter(|(_, device)| device.contains_key("Class"))
}

/// The paired Bluetooth Classic device on the adapter in use that is the phone. With several, the
/// one whose object path sorts first, so every call picks the same device.
pub(super) fn paired_phone_device<'a>(objects: &'a Objects, names: &[String]) -> Option<&'a Properties> {
    let (adapter, _) = chosen_adapter(objects)?;
    classic_devices(objects, adapter.as_str())
        .filter(|(_, device)| bool_of(device, "Paired") && is_phone(device, names))
        .min_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()))
        .map(|(_, device)| device)
}

/// The device a discovery on `adapter` found that is the phone: a Bluetooth Classic device this
/// discovery saw (`BlueZ` has its signal strength, or it was not known before the discovery) whose name
/// is one of `names`. With several, the one whose object path sorts first.
pub(super) fn discovered_phone<'a>(
    objects: &'a Objects,
    adapter: &str,
    known_before: &HashSet<String>,
    names: &[String],
) -> Option<(&'a OwnedObjectPath, &'a Properties)> {
    classic_devices(objects, adapter)
        .filter(|(path, device)| {
            (device.contains_key("RSSI") || !known_before.contains(path.as_str())) && is_phone(device, names)
        })
        .min_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()))
}

/// The messaging proximity gate's state from `BlueZ`'s objects: Bluetooth is off while the adapter in
/// use is off (no adapter is powered); otherwise the paired phone is connected or not, or no paired
/// device is the phone.
pub(super) fn proximity_of(objects: &Objects, names: &[String]) -> ProximityState {
    if !chosen_adapter(objects).is_some_and(|(_, adapter)| bool_of(adapter, "Powered")) {
        return ProximityState::BluetoothOff;
    }
    match paired_phone_device(objects, names) {
        Some(device) if bool_of(device, "Connected") => ProximityState::Connected,
        Some(_) => ProximityState::NotConnected,
        None => ProximityState::PhoneNotPaired,
    }
}

/// The device lists `service` among the services `BlueZ` read from it.
pub(super) fn offers(device: &Properties, service: &str) -> bool {
    strings_of(device, "UUIDs").iter().any(|uuid| uuid.eq_ignore_ascii_case(service))
}
