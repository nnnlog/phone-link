//! `BlueZ` as the engine's edges: the phone's Bluetooth (`PhoneBluetooth`) and the messaging proximity
//! state (`PhoneProximity`).

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use phonelink_bluetooth::{PairingError, PhoneBluetooth, RfcommError};
use phonelink_messaging::{PhoneProximity, ProximityPhone, ProximityState};

use super::bluez::Bluez;
use super::watch::subscribe;
use crate::dbus::{self, Bus};

/// The bound of every `BlueZ` call. It is longer than the pairing's wait, so a pairing that does not
/// finish is given up and cancelled here first; the other calls that can take long run on threads of
/// their own against their caller's deadline.
const METHOD_TIMEOUT: Duration = Duration::from_secs(150);

pub(crate) struct BluezBluetooth {
    bluez: Arc<Bluez>,
}

impl BluezBluetooth {
    /// Connects to the system bus and starts the watcher, which registers Hands-Free with `BlueZ` as
    /// soon as `BlueZ` runs, and again whenever `BlueZ` restarts.
    pub(crate) fn connect() -> zbus::Result<Self> {
        let bus = dbus::connect(Bus::System, Some(METHOD_TIMEOUT))?;
        let bluez = Arc::new(Bluez {
            bus,
            waiters: Arc::default(),
            next_waiter: AtomicU64::new(1),
            registering: Mutex::default(),
            registrations: Arc::default(),
            channels: Arc::default(),
            next_channel: AtomicU64::new(1),
            mirror: Mutex::default(),
        });

        let signals = subscribe()?;
        let watcher = bluez.clone();
        thread::Builder::new()
            .name("bluez-watch".to_owned())
            .spawn(move || watcher.follow(signals))
            .map_err(|e| zbus::Error::InputOutput(Arc::new(e)))?;
        Ok(Self { bluez })
    }

    /// The address of the adapter in use, as `BlueZ` writes it (`XX:XX:XX:XX:XX:XX`).
    pub(crate) fn adapter_address(&self) -> Option<String> {
        self.bluez.adapter_address()
    }
}

impl PhoneBluetooth for BluezBluetooth {
    fn radio_on(&self) -> bool {
        self.bluez.radio_on()
    }

    fn paired_phone(&self, names: Vec<String>) -> Option<String> {
        self.bluez.paired_phone(&names)
    }

    fn discover_and_pair(&self, names: Vec<String>, timeout_ms: u64) -> Result<String, PairingError> {
        self.bluez.discover_and_pair(&names, timeout_ms)
    }

    fn rfcomm_open(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<u64, RfcommError> {
        self.bluez.rfcomm_open(&address, &service_uuid, timeout_ms)
    }

    fn refresh_services(&self, address: String, service_uuid: String, timeout_ms: u64) -> Result<bool, RfcommError> {
        self.bluez.refresh_services(&address, &service_uuid, timeout_ms)
    }

    fn rfcomm_write(&self, channel: u64, bytes: Vec<u8>) -> Result<(), RfcommError> {
        self.bluez.rfcomm_write(channel, &bytes)
    }

    fn rfcomm_read(&self, channel: u64, timeout_ms: u64) -> Result<Vec<u8>, RfcommError> {
        self.bluez.rfcomm_read(channel, timeout_ms)
    }

    fn rfcomm_close(&self, channel: u64) {
        self.bluez.rfcomm_close(channel);
    }
}

impl PhoneProximity for BluezBluetooth {
    fn state(&self, phone: ProximityPhone) -> ProximityState {
        self.bluez.proximity(&phone.names)
    }
}
