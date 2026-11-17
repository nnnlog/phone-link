//! The RFCOMM profiles registered with `BlueZ`: what this computer registers to reach a service on the
//! phone, and the handler that hands each connection `BlueZ` makes to the open waiting for it.

use std::collections::HashMap;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, mpsc};

use phonelink_bluetooth::RfcommError;
use phonelink_core::sync::MutexExt;
use tracing::info;
use zbus::blocking::Proxy;
use zbus::zvariant::{ObjectPath, OwnedFd, OwnedObjectPath, OwnedValue, Value};

use super::bluez::{BLUEZ, Bluez, PROFILE_MANAGER};
use super::rfcomm::{Channels, rfcomm_err};

// BlueZ gives a profile registered as Hands-Free (0x111E) the Audio Gateway (0x111F) as its remote
// side and publishes the HF service record the phone needs before it offers calls; a profile
// registered as 0x111F would instead make this computer the gateway, which ConnectProfile to the
// phone's gateway cannot use.
pub(super) const HANDSFREE: &str = "0000111e-0000-1000-8000-00805f9b34fb";
pub(super) const HANDSFREE_AUDIO_GATEWAY: &str = "0000111f-0000-1000-8000-00805f9b34fb";
/// The Hands-Free features the published record names: three-way calling (0x02) and caller ID
/// presentation (0x04), the features the engine announces in `AT+BRSF` that the record has bits for.
const HANDSFREE_RECORD_FEATURES: u16 = 0x0006;

/// Whom an incoming RFCOMM connection is for: the device's object path and the service on it
/// (lowercase UUID) that an `rfcomm_open` asked for.
pub(super) type WaiterKey = (String, String);

/// What ends an `rfcomm_open`'s wait: the connection `BlueZ` handed over, or why `BlueZ` did not connect.
pub(super) type Opened = Result<OwnedFd, RfcommError>;

/// An `rfcomm_open` waiting for its connection. `id` tells its entry from a later open's under the
/// same key, so each open removes only its own.
pub(super) struct Waiter {
    pub(super) id: u64,
    pub(super) sender: mpsc::Sender<Opened>,
}

pub(super) type Waiters = Arc<Mutex<HashMap<WaiterKey, Waiter>>>;

/// The services on the phone a profile registered with the running `BlueZ` reaches.
pub(super) type Registrations = Arc<Mutex<Vec<String>>>;

/// Hands `fd` to the open waiting for `key`; shuts it down when no open waits for it or the open has
/// given up. The waiters stay locked until the connection is in the open's channel, so an open that
/// gives up either finds it there or leaves it to be shut down here. Whether an open took it.
pub(super) fn deliver(waiters: &Mutex<HashMap<WaiterKey, Waiter>>, key: &WaiterKey, fd: OwnedFd) -> bool {
    let mut waiting = waiters.lock_or_recover();
    let undelivered = match waiting.remove(key) {
        Some(waiter) => match waiter.sender.send(Ok(fd)) {
            Ok(()) => return true,
            Err(mpsc::SendError(sent)) => sent.ok(),
        },
        None => Some(fd),
    };
    drop(waiting);

    if let Some(fd) = undelivered {
        shut_down(fd);
    }
    false
}

/// Ends the wait of open `id` for `key`: removes its waiter, so a connection that comes later is shut
/// down by the profile's handler, and shuts down one that came between the end of the wait and now.
pub(super) fn stop_waiting(
    waiters: &Mutex<HashMap<WaiterKey, Waiter>>,
    key: &WaiterKey,
    id: u64,
    receiver: &mpsc::Receiver<Opened>,
) {
    {
        let mut waiting = waiters.lock_or_recover();
        if waiting.get(key).is_some_and(|waiter| waiter.id == id) {
            waiting.remove(key);
        }
    }

    while let Ok(late) = receiver.try_recv() {
        if let Ok(fd) = late {
            info!("closed an RFCOMM connection that came after its open gave up");
            shut_down(fd);
        }
    }
}

/// Ends an RFCOMM connection `BlueZ` handed over.
///
/// `BlueZ` keeps its own descriptor of the socket, so only a shutdown ends the connection.
fn shut_down(fd: OwnedFd) {
    if let Err(e) = UnixStream::from(std::os::fd::OwnedFd::from(fd)).shutdown(Shutdown::Both) {
        info!(error = %e, "an RFCOMM connection was not shut down");
    }
}

pub(super) fn waiter_key(device: &str, service: &str) -> WaiterKey {
    (device.to_owned(), service.to_ascii_lowercase())
}

/// Where `BlueZ` sends one registered profile's connections. Each registration has its own object, and
/// the connections it receives reach one service on the phone, so a connection goes to the open
/// that asked for that service on that device.
pub(super) struct ProfileHandler {
    pub(super) waiters: Waiters,
    pub(super) channels: Channels,
    pub(super) registrations: Registrations,
    /// The service on the phone this profile's connections reach (lowercase UUID).
    pub(super) remote_service: String,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl ProfileHandler {
    pub(super) fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) {
        let key = waiter_key(device.as_str(), &self.remote_service);
        if !deliver(&self.waiters, &key, fd) {
            info!(service_uuid = %self.remote_service, "closed an RFCOMM connection no open waited for");
        }
    }

    /// `BlueZ` disconnected the service (the device was disconnected, from here or from the desktop's
    /// Bluetooth settings): the channels to it on that device end, so their readers see them closed.
    pub(super) fn request_disconnection(&self, device: OwnedObjectPath) {
        let mut closed = 0;
        for channel in self.channels.lock_or_recover().values() {
            if channel.device == device.as_str() && channel.service == self.remote_service {
                match channel.stream.shutdown(Shutdown::Both) {
                    Ok(()) => closed += 1,
                    Err(e) => info!(error = %e, "an RFCOMM channel BlueZ disconnected was not shut down"),
                }
            }
        }
        info!(service_uuid = %self.remote_service, count = closed, "BlueZ disconnected a service's channels");
    }

    /// `BlueZ` dropped the registration (it is stopping): the next open registers the profile again.
    pub(super) fn release(&self) {
        self.registrations.lock_or_recover().retain(|service| *service != self.remote_service);
        info!(service_uuid = %self.remote_service, "BlueZ released a profile registration");
    }
}

/// What this computer registers to reach `service` on the phone: the profile's UUID and its options.
/// The phone's audio gateway is reached through the Hands-Free registration, any other service
/// through a client registration of its own UUID.
pub(super) fn registration_for(service: &str) -> (&str, HashMap<&'static str, Value<'static>>) {
    let mut options =
        HashMap::from([("AutoConnect", Value::from(false)), ("RequireAuthentication", Value::from(true))]);
    if service == HANDSFREE_AUDIO_GATEWAY {
        options.insert("Features", Value::from(HANDSFREE_RECORD_FEATURES));
        (HANDSFREE, options)
    } else {
        options.insert("Role", Value::from("client"));
        (service, options)
    }
}

/// `BlueZ` refused a registration because this connection holds it already: `BlueZ` keeps one profile
/// per connection and object path, and one agent per connection.
pub(super) fn already_registered(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.bluez.Error.AlreadyExists")
}

impl Bluez {
    /// Registers the profile that reaches `service` on the phone unless the running `BlueZ` has it.
    pub(super) fn ensure_profile(&self, service: &str) -> Result<(), RfcommError> {
        let _registering = self.registering.lock_or_recover();
        if self.registrations.lock_or_recover().iter().any(|reached| reached == service) {
            return Ok(());
        }
        self.register_profile(service)?;
        self.registrations.lock_or_recover().push(service.to_owned());
        Ok(())
    }

    fn register_profile(&self, service: &str) -> Result<(), RfcommError> {
        let (uuid, options) = registration_for(service);
        let path = format!("/dev/phonelink/profile/p{}", uuid.replace('-', "_"));
        let handler = ProfileHandler {
            waiters: self.waiters.clone(),
            channels: self.channels.clone(),
            registrations: self.registrations.clone(),
            remote_service: service.to_owned(),
        };
        // An object served for an earlier registration (one BlueZ dropped) stays: it is the same.
        self.bus.object_server().at(path.as_str(), handler).map_err(rfcomm_err)?;

        let object_path = ObjectPath::try_from(path.as_str()).map_err(rfcomm_err)?;
        match Proxy::new(&self.bus, BLUEZ, "/org/bluez", PROFILE_MANAGER)
            .and_then(|manager| manager.call::<_, _, ()>("RegisterProfile", &(object_path, uuid, options)))
        {
            Ok(()) => Ok(()),
            // Registered while BlueZ's restart cleared the list of registrations.
            Err(e) if already_registered(&e) => Ok(()),
            Err(e) => Err(rfcomm_err(e)),
        }
    }
}
