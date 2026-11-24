//! The `BlueZ` client the edges and the watcher share: `BlueZ`'s D-Bus names, and its objects read on
//! demand, with the radio and the paired phone among them.

use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use tracing::info;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

use super::objects::{
    Objects, Properties, bool_of, chosen_adapter, devices_on, engine_address, paired_phone_device, string_of,
};
use super::profile::{Registrations, Waiters};
use super::rfcomm::Channels;
use super::watch::Mirror;

pub(super) const BLUEZ: &str = "org.bluez";
pub(super) const ADAPTER: &str = "org.bluez.Adapter1";
pub(super) const DEVICE: &str = "org.bluez.Device1";
pub(super) const PROFILE_MANAGER: &str = "org.bluez.ProfileManager1";
pub(super) const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
pub(super) const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
pub(super) const DBUS: &str = "org.freedesktop.DBus";
pub(super) const DBUS_PATH: &str = "/org/freedesktop/DBus";
/// How long a wait for `BlueZ`'s objects to change sleeps before it reads them again.
pub(super) const POLL: Duration = Duration::from_millis(300);

/// What the edges and the watcher share.
pub(super) struct Bluez {
    pub(super) bus: Connection,
    pub(super) waiters: Waiters,
    pub(super) next_waiter: AtomicU64,
    /// Held across a registration, so two never race; no `BlueZ` callback takes it.
    pub(super) registering: Mutex<()>,
    pub(super) registrations: Registrations,
    pub(super) channels: Channels,
    pub(super) next_channel: AtomicU64,
    pub(super) mirror: Mutex<Mirror>,
}

impl Bluez {
    pub(super) fn objects(&self) -> zbus::Result<Objects> {
        Proxy::new(&self.bus, BLUEZ, "/", OBJECT_MANAGER)?.call("GetManagedObjects", &())
    }

    pub(super) fn device(&self, path: &OwnedObjectPath) -> zbus::Result<Proxy<'static>> {
        Proxy::new(&self.bus, BLUEZ, path.clone(), DEVICE)
    }

    /// The object path of the device at `address` (`xx-xx-…`) on the adapter in use.
    ///
    /// `None` when `BlueZ` knows no such device there.
    pub(super) fn device_path(&self, address: &str) -> zbus::Result<Option<OwnedObjectPath>> {
        let objects = self.objects()?;
        let Some((adapter, _)) = chosen_adapter(&objects) else { return Ok(None) };
        Ok(devices_on(&objects, adapter.as_str())
            .find(|(_, device)| string_of(device, "Address").is_some_and(|a| engine_address(&a) == address))
            .map(|(path, _)| path.clone()))
    }

    /// The device's properties as `BlueZ` has them now; `None` when it is gone.
    pub(super) fn device_properties(&self, path: &OwnedObjectPath) -> zbus::Result<Option<Properties>> {
        Ok(self.objects()?.remove(path).and_then(|mut interfaces| interfaces.remove(DEVICE)))
    }

    /// Whether the adapter in use is powered; off when `BlueZ` does not answer.
    pub(super) fn radio_on(&self) -> bool {
        match self.objects() {
            Ok(objects) => chosen_adapter(&objects).is_some_and(|(_, adapter)| bool_of(adapter, "Powered")),
            Err(e) => {
                info!(error = %e, "BlueZ did not answer; the radio counts as off");
                false
            }
        }
    }

    /// The address of the adapter in use, as `BlueZ` writes it; `None` without an adapter or an answer.
    pub(super) fn adapter_address(&self) -> Option<String> {
        let objects = self.objects().inspect_err(|e| info!(error = %e, "BlueZ did not answer")).ok()?;
        chosen_adapter(&objects).and_then(|(_, adapter)| string_of(adapter, "Address"))
    }

    /// The address of the paired phone named one of `names`, in the engine's form.
    pub(super) fn paired_phone(&self, names: &[String]) -> Option<String> {
        let objects = self.objects().inspect_err(|e| info!(error = %e, "BlueZ did not answer")).ok()?;
        paired_phone_device(&objects, names).and_then(|device| string_of(device, "Address")).map(|a| engine_address(&a))
    }
}
