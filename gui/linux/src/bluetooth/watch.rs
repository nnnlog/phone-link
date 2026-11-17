//! The watcher: a copy of `BlueZ`'s adapters and devices kept from its signals and from the owner
//! changes of its name, so the proximity state is read without waiting for `BlueZ`.

use std::collections::HashMap;

use phonelink_core::sync::MutexExt;
use phonelink_messaging::ProximityState;
use tracing::{info, warn};
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::{Message, Type as MessageType};
use zbus::zvariant::OwnedObjectPath;

use super::bluez::{ADAPTER, BLUEZ, Bluez, DBUS, DBUS_PATH, DEVICE, OBJECT_MANAGER};
use super::objects::{Objects, Properties, proximity_of};
use super::profile::HANDSFREE_AUDIO_GATEWAY;
use crate::dbus::PROPERTIES;

/// The interfaces the watcher keeps of `BlueZ`'s objects.
fn mirrored_interface(name: &str) -> bool {
    name == ADAPTER || name == DEVICE
}

pub(super) fn mirrored(objects: Objects) -> Objects {
    objects
        .into_iter()
        .filter_map(|(path, mut interfaces)| {
            interfaces.retain(|name, _| mirrored_interface(name));
            (!interfaces.is_empty()).then_some((path, interfaces))
        })
        .collect()
}

/// `InterfacesAdded`: the object gained `interfaces`, each with all its properties.
pub(super) fn add_interfaces(objects: &mut Objects, path: OwnedObjectPath, interfaces: HashMap<String, Properties>) {
    let mut added = interfaces.into_iter().filter(|(name, _)| mirrored_interface(name)).peekable();
    if added.peek().is_some() {
        objects.entry(path).or_default().extend(added);
    }
}

/// `InterfacesRemoved`: the object lost `interfaces`; one left with none is gone.
pub(super) fn remove_interfaces(objects: &mut Objects, path: &OwnedObjectPath, interfaces: &[String]) {
    if let Some(object) = objects.get_mut(path) {
        for name in interfaces {
            object.remove(name);
        }
        if object.is_empty() {
            objects.remove(path);
        }
    }
}

/// `PropertiesChanged`: new values of some of an interface's properties, and properties it no longer
/// has a value for.
pub(super) fn change_properties(
    objects: &mut Objects,
    path: &OwnedObjectPath,
    interface: &str,
    changed: Properties,
    invalidated: &[String],
) {
    let Some(properties) = objects.get_mut(path).and_then(|object| object.get_mut(interface)) else { return };
    properties.extend(changed);
    for name in invalidated {
        properties.remove(name);
    }
}

/// Applies one of `BlueZ`'s object signals to `objects`.
fn apply_signal(objects: &mut Objects, member: &str, message: &Message) -> zbus::Result<()> {
    match member {
        "InterfacesAdded" => {
            let (path, interfaces): (OwnedObjectPath, HashMap<String, Properties>) = message.body().deserialize()?;
            add_interfaces(objects, path, interfaces);
        }
        "InterfacesRemoved" => {
            let (path, interfaces): (OwnedObjectPath, Vec<String>) = message.body().deserialize()?;
            remove_interfaces(objects, &path, &interfaces);
        }
        "PropertiesChanged" => {
            let Some(path) = message.header().path().map(|path| OwnedObjectPath::from(path.to_owned())) else {
                return Ok(());
            };
            let (interface, changed, invalidated): (String, Properties, Vec<String>) = message.body().deserialize()?;
            change_properties(objects, &path, &interface, changed, &invalidated);
        }
        _ => {}
    }
    Ok(())
}

/// What the watcher keeps of `BlueZ`: the connection that owns `BlueZ`'s name (`None` while `BlueZ` is
/// not running) and the adapter and device properties of its objects.
#[derive(Default)]
pub(super) struct Mirror {
    owner: Option<String>,
    objects: Objects,
}

/// A connection of its own subscribed to `BlueZ`'s signals and to the owner changes of `BlueZ`'s name.
///
/// From the returned iterator on, everything the connection receives comes out of it in order, so
/// objects read after it is made and the signals after them describe `BlueZ` as it is.
pub(super) fn subscribe() -> zbus::Result<MessageIterator> {
    let bus = Connection::system()?;
    let rules = [
        MatchRule::builder().msg_type(MessageType::Signal).sender(BLUEZ)?.build(),
        MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(DBUS)?
            .interface(DBUS)?
            .member("NameOwnerChanged")?
            .arg(0, BLUEZ)?
            .build(),
    ];
    for rule in rules {
        bus.call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "AddMatch", &(rule.to_string(),))?;
    }
    Ok(MessageIterator::from(bus))
}

/// The connection that owns `BlueZ`'s name; `None` while `BlueZ` is not running.
fn bluez_owner(bus: &Connection) -> zbus::Result<Option<String>> {
    match bus.call_method(Some(DBUS), DBUS_PATH, Some(DBUS), "GetNameOwner", &(BLUEZ,)) {
        Ok(reply) => Ok(Some(reply.body().deserialize::<String>()?)),
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner" => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

impl Bluez {
    /// The messaging proximity state from the watcher's copy of `BlueZ`'s objects.
    ///
    /// It never waits for `BlueZ`; while `BlueZ` is not running the copy is empty, and Bluetooth counts
    /// as off.
    pub(super) fn proximity(&self, names: &[String]) -> ProximityState {
        proximity_of(&self.mirror.lock_or_recover().objects, names)
    }

    /// `BlueZ`'s name has a new owner (`None`: `BlueZ` stopped). The registrations went with the old one;
    /// the objects are read from the new one, which gets the Hands-Free registration again.
    fn follow_owner(&self, owner: Option<String>) {
        {
            let mut mirror = self.mirror.lock_or_recover();
            mirror.owner = owner.clone();
            mirror.objects.clear();
        }
        self.registrations.lock_or_recover().clear();
        if owner.is_none() {
            info!("BlueZ is not running; Bluetooth counts as off until it is");
            return;
        }

        match self.objects() {
            Ok(objects) => self.mirror.lock_or_recover().objects = mirrored(objects),
            Err(e) => warn!(error = %e, "BlueZ's objects were not read; Bluetooth counts as off until they change"),
        }
        // Registered up front so the phone sees the HF service record before it offers calls; a
        // registration that fails here is tried again when a channel to the gateway is opened.
        if let Err(e) = self.ensure_profile(HANDSFREE_AUDIO_GATEWAY) {
            warn!(error = %e, "this computer does not advertise hands-free calling");
        }
    }

    /// Applies one message from the watcher's connection.
    fn follow_message(&self, message: &Message) {
        if message.message_type() != MessageType::Signal {
            return;
        }
        let header = message.header();
        let (Some(interface), Some(member)) = (header.interface(), header.member()) else { return };
        let sender = header.sender().map(|sender| sender.as_str());

        match (interface.as_str(), member.as_str()) {
            (DBUS, "NameOwnerChanged") if sender == Some(DBUS) => {
                match message.body().deserialize::<(String, String, String)>() {
                    Ok((name, _, owner)) if name == BLUEZ => self.follow_owner((!owner.is_empty()).then_some(owner)),
                    Ok(_) => {}
                    Err(e) => warn!(error = %e, "a name owner change did not read"),
                }
            }
            (OBJECT_MANAGER | PROPERTIES, member) => {
                let mut mirror = self.mirror.lock_or_recover();
                // Only the signals of the connection that owns BlueZ's name describe its objects.
                if sender.is_none() || sender != mirror.owner.as_deref() {
                    return;
                }
                if let Err(e) = apply_signal(&mut mirror.objects, member, message) {
                    warn!(error = %e, "a BlueZ signal did not read; the proximity state may lag until the next");
                }
            }
            _ => {}
        }
    }

    /// The watcher: reads `BlueZ`'s objects, then follows its signals and the owner of its name.
    pub(super) fn follow(&self, signals: MessageIterator) {
        match bluez_owner(&self.bus) {
            Ok(owner) => self.follow_owner(owner),
            Err(e) => warn!(error = %e, "BlueZ's owner was not read; Bluetooth counts as off until it changes"),
        }
        for message in signals {
            match message {
                Ok(message) => self.follow_message(&message),
                Err(e) => warn!(error = %e, "a message about BlueZ did not read"),
            }
        }
        warn!("BlueZ's changes are no longer followed; the proximity state stays as it was");
    }
}
