use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use phonelink_bluetooth::{PairingError, PhoneBluetooth, RfcommError};
use zbus::blocking::{Connection, Proxy, connection};
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const PROFILE_MANAGER: &str = "org.bluez.ProfileManager1";
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(300);

type Properties = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<String, Properties>>;
type Waiters = Arc<Mutex<HashMap<String, mpsc::Sender<OwnedFd>>>>;

fn bool_of(properties: &Properties, name: &str) -> bool {
    properties.get(name).and_then(|value| value.downcast_ref::<bool>().ok()).unwrap_or(false)
}

fn string_of(properties: &Properties, name: &str) -> Option<String> {
    properties.get(name).and_then(|value| value.downcast_ref::<&str>().ok()).map(str::to_owned)
}

fn strings_of(properties: &Properties, name: &str) -> Vec<String> {
    properties
        .get(name)
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<String>::try_from(value).ok())
        .unwrap_or_default()
}

/// The engine names devices as macOS does, `xx-xx-xx-xx-xx-xx`; BlueZ writes `XX:XX:XX:XX:XX:XX`.
fn engine_address(bluez: &str) -> String {
    bluez.replace(':', "-").to_ascii_lowercase()
}

fn is_phone(properties: &Properties, names: &[String]) -> bool {
    [string_of(properties, "Name"), string_of(properties, "Alias")].into_iter().flatten().any(|name| {
        let name = name.trim();
        !name.is_empty() && names.iter().any(|wanted| wanted.trim().eq_ignore_ascii_case(name))
    })
}

struct ProfileHandler {
    waiters: Waiters,
}

#[zbus::interface(name = "org.bluez.Profile1")]
impl ProfileHandler {
    fn new_connection(&self, device: OwnedObjectPath, fd: OwnedFd, _properties: HashMap<String, OwnedValue>) {
        if let Some(waiter) = self.waiters.lock().unwrap_or_else(PoisonError::into_inner).remove(device.as_str()) {
            let _ = waiter.send(fd);
        }
    }

    fn request_disconnection(&self, _device: OwnedObjectPath) {}

    fn release(&self) {}
}

// BlueZ gives a profile registered as Hands-Free (0x111E) the Audio Gateway (0x111F) as its remote
// side and publishes the HF service record the phone needs before it offers calls; a profile
// registered as 0x111F would instead make this computer the gateway, which ConnectProfile to the
// phone's gateway cannot use.
const HANDSFREE: &str = "0000111e-0000-1000-8000-00805f9b34fb";
const HANDSFREE_AUDIO_GATEWAY: &str = "0000111f-0000-1000-8000-00805f9b34fb";

pub struct BluezBluetooth {
    bus: Connection,
    waiters: Waiters,
    profiles: Mutex<Vec<String>>,
    channels: Mutex<HashMap<u64, UnixStream>>,
    next_channel: AtomicU64,
}

fn rfcomm_err(e: impl std::fmt::Display) -> RfcommError {
    RfcommError::Unexpected { message: format!("BlueZ: {e}") }
}

fn connect_err(e: zbus::Error) -> RfcommError {
    let text = e.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("page-timeout") || lower.contains("host-down") || lower.contains("host is down") {
        RfcommError::Unreachable
    } else if lower.contains("notavailable") || lower.contains("profile-unavailable") || lower.contains("not found") {
        RfcommError::ServiceNotFound
    } else {
        RfcommError::ConnectFailed { reason: text }
    }
}

impl BluezBluetooth {
    pub fn connect() -> zbus::Result<Self> {
        let bus = connection::Builder::system()?.method_timeout(PAIRING_TIMEOUT).build()?;
        let bluetooth = Self {
            bus,
            waiters: Arc::default(),
            profiles: Mutex::default(),
            channels: Mutex::default(),
            next_channel: AtomicU64::new(1),
        };
        match bluetooth.register_profile(HANDSFREE, None) {
            Ok(()) => bluetooth
                .profiles
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(HANDSFREE_AUDIO_GATEWAY.to_owned()),
            Err(e) => eprintln!("this computer does not advertise hands-free calling: {e}"),
        }
        Ok(bluetooth)
    }

    fn objects(&self) -> zbus::Result<Objects> {
        Proxy::new(&self.bus, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")?.call("GetManagedObjects", &())
    }

    fn adapter(&self) -> Option<(OwnedObjectPath, Properties)> {
        self.objects().ok()?.into_iter().find_map(|(path, mut interfaces)| Some((path, interfaces.remove(ADAPTER)?)))
    }

    fn device_path(&self, address: &str) -> Option<OwnedObjectPath> {
        self.objects().ok()?.into_iter().find_map(|(path, interfaces)| {
            let device = interfaces.get(DEVICE)?;
            (engine_address(&string_of(device, "Address")?) == address).then_some(path)
        })
    }

    fn device(&self, path: &OwnedObjectPath) -> zbus::Result<Proxy<'static>> {
        Proxy::new(&self.bus, BLUEZ, path.clone(), DEVICE)
    }

    fn ensure_profile(&self, uuid: &str) -> Result<(), RfcommError> {
        let mut profiles = self.profiles.lock().unwrap_or_else(PoisonError::into_inner);
        if profiles.iter().any(|registered| registered == uuid) {
            return Ok(());
        }
        self.register_profile(uuid, Some("client"))?;
        profiles.push(uuid.to_owned());
        Ok(())
    }

    fn register_profile(&self, uuid: &str, role: Option<&str>) -> Result<(), RfcommError> {
        let path = format!("/dev/phonelink/profile/p{}", uuid.replace('-', "_"));
        self.bus
            .object_server()
            .at(path.as_str(), ProfileHandler { waiters: self.waiters.clone() })
            .map_err(rfcomm_err)?;
        let mut options: HashMap<&str, Value> =
            HashMap::from([("AutoConnect", Value::from(false)), ("RequireAuthentication", Value::from(true))]);
        if let Some(role) = role {
            options.insert("Role", Value::from(role));
        }
        let object_path = zbus::zvariant::ObjectPath::try_from(path.as_str()).map_err(rfcomm_err)?;
        Proxy::new(&self.bus, BLUEZ, "/org/bluez", PROFILE_MANAGER)
            .and_then(|manager| manager.call::<_, _, ()>("RegisterProfile", &(object_path, uuid, options)))
            .map_err(rfcomm_err)
    }

    fn with_channel<T>(
        &self,
        channel: u64,
        f: impl FnOnce(&mut UnixStream) -> Result<T, RfcommError>,
    ) -> Result<T, RfcommError> {
        let mut channels = self.channels.lock().unwrap_or_else(PoisonError::into_inner);
        let stream =
            channels.get_mut(&channel).ok_or(RfcommError::Io { reason: "the channel is closed".to_owned() })?;
        let mut stream = stream.try_clone().map_err(|e| RfcommError::Io { reason: e.to_string() })?;
        drop(channels);
        f(&mut stream)
    }
}

impl PhoneBluetooth for BluezBluetooth {
    fn radio_on(&self) -> bool {
        self.adapter().is_some_and(|(_, adapter)| bool_of(&adapter, "Powered"))
    }

    fn paired_phone(&self, names: Vec<String>) -> Option<String> {
        self.objects()
            .ok()?
            .values()
            .find_map(|interfaces| {
                let device = interfaces.get(DEVICE)?;
                (bool_of(device, "Paired") && is_phone(device, &names)).then(|| string_of(device, "Address")).flatten()
            })
            .map(|address| engine_address(&address))
    }

    fn discover_and_pair(&self, names: Vec<String>, timeout_ms: u64) -> Result<String, PairingError> {
        let pairing_err = |e: zbus::Error| PairingError::Failed { reason: e.to_string() };
        let (adapter_path, _) =
            self.adapter().ok_or(PairingError::Unexpected { message: "no Bluetooth adapter".to_owned() })?;
        let adapter = Proxy::new(&self.bus, BLUEZ, adapter_path, ADAPTER).map_err(pairing_err)?;
        let _ = adapter.call::<_, _, ()>("StartDiscovery", &());

        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let found = loop {
            let found = self.objects().ok().and_then(|objects| {
                objects.into_iter().find_map(|(path, interfaces)| {
                    let device = interfaces.get(DEVICE)?;
                    is_phone(device, &names).then(|| (path, bool_of(device, "Paired"), string_of(device, "Address")))
                })
            });
            if found.is_some() || Instant::now() >= deadline {
                break found;
            }
            thread::sleep(POLL);
        };
        let _ = adapter.call::<_, _, ()>("StopDiscovery", &());

        let (path, paired, address) = found.ok_or(PairingError::NotFound)?;
        let device = self.device(&path).map_err(pairing_err)?;
        if !paired {
            device.call::<_, _, ()>("Pair", &()).map_err(pairing_err)?;
        }
        let _ = device.set_property("Trusted", true);
        address
            .map(|a| engine_address(&a))
            .ok_or(PairingError::Failed { reason: "the device has no address".to_owned() })
    }

    fn rfcomm_open(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<u64, RfcommError> {
        let uuid = service_uuid.to_ascii_lowercase();
        self.ensure_profile(&uuid)?;
        let path = self.device_path(&address).ok_or(RfcommError::Unreachable)?;
        let (sender, receiver) = mpsc::channel();
        self.waiters.lock().unwrap_or_else(PoisonError::into_inner).insert(path.to_string(), sender);

        let device = self.device(&path).map_err(rfcomm_err)?;
        let connected = device.call::<_, _, ()>("ConnectProfile", &(uuid.as_str(),));
        let fd = match connected {
            Ok(()) => receiver.recv_timeout(Duration::from_millis(timeout_ms)).map_err(|_| RfcommError::TimedOut),
            Err(e) => Err(connect_err(e)),
        };
        self.waiters.lock().unwrap_or_else(PoisonError::into_inner).remove(path.as_str());

        let stream = UnixStream::from(std::os::fd::OwnedFd::from(fd?));
        stream.set_nonblocking(false).map_err(|e| RfcommError::Io { reason: e.to_string() })?;
        let channel = self.next_channel.fetch_add(1, Ordering::SeqCst);
        self.channels.lock().unwrap_or_else(PoisonError::into_inner).insert(channel, stream);
        Ok(channel)
    }

    fn refresh_services(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<bool, RfcommError> {
        let uuid = service_uuid.to_ascii_lowercase();
        let path = self.device_path(&address).ok_or(RfcommError::Unreachable)?;
        let has_service = || {
            self.objects()
                .ok()
                .and_then(|objects| objects.get(&path).and_then(|i| i.get(DEVICE)).map(|d| strings_of(d, "UUIDs")))
                .is_some_and(|uuids| uuids.iter().any(|u| u.eq_ignore_ascii_case(&uuid)))
        };
        if has_service() {
            return Ok(true);
        }
        let device = self.device(&path).map_err(rfcomm_err)?;
        let _ = device.call::<_, _, ()>("Connect", &());
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        while Instant::now() < deadline {
            if has_service() {
                return Ok(true);
            }
            thread::sleep(POLL);
        }
        Ok(has_service())
    }

    fn rfcomm_write(&self, channel: u64, bytes: Vec<u8>) -> Result<(), RfcommError> {
        self.with_channel(channel, |stream| {
            stream.write_all(&bytes).map_err(|e| RfcommError::Io { reason: e.to_string() })
        })
    }

    fn rfcomm_read(&self, channel: u64, timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
        self.with_channel(channel, |stream| {
            stream
                .set_read_timeout(Some(Duration::from_millis(timeout_ms.max(1))))
                .map_err(|e| RfcommError::Io { reason: e.to_string() })?;
            let mut buffer = vec![0; 4096];
            match stream.read(&mut buffer) {
                Ok(read) => {
                    buffer.truncate(read);
                    Ok(buffer)
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Err(RfcommError::TimedOut),
                Err(e) => Err(RfcommError::Io { reason: e.to_string() }),
            }
        })
    }

    fn rfcomm_close(&self, channel: u64) {
        if let Some(stream) = self.channels.lock().unwrap_or_else(PoisonError::into_inner).remove(&channel) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_use_the_engine_form() {
        assert_eq!(engine_address("A4:5E:60:0B:1C:2D"), "a4-5e-60-0b-1c-2d");
    }
}
