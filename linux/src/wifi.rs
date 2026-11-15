use std::collections::HashMap;
use std::thread;
use std::time::{Duration, Instant};

use phonelink_hotspot::HotspotWifi;
use phonelink_hotspot::edge::WifiError;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const NM_WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const NM_ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const NM_DEVICE_TYPE_WIFI: u32 = 2;
const NM_DEVICE_STATE_ACTIVATED: u32 = 100;
const NM_DEVICE_STATE_FAILED: u32 = 120;
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);

pub struct NetworkManagerWifi {
    bus: Option<Connection>,
}

const SCAN_TIMEOUT: Duration = Duration::from_secs(10);

fn wifi_err(e: impl std::fmt::Display) -> WifiError {
    WifiError::Failed { reason: format!("NetworkManager: {e}") }
}

impl NetworkManagerWifi {
    pub fn new() -> Self {
        Self { bus: Connection::system().ok() }
    }

    fn bus(&self) -> Result<&Connection, WifiError> {
        self.bus.as_ref().ok_or(WifiError::Unavailable)
    }

    fn device(&self) -> Result<OwnedObjectPath, WifiError> {
        let bus = self.bus()?;
        let devices: Vec<OwnedObjectPath> =
            Proxy::new(bus, NM, NM_PATH, NM).and_then(|nm| nm.call("GetDevices", &())).map_err(wifi_err)?;
        devices
            .into_iter()
            .find(|path| {
                Proxy::new(bus, NM, path.clone(), NM_DEVICE)
                    .and_then(|device| device.get_property::<u32>("DeviceType"))
                    .is_ok_and(|kind| kind == NM_DEVICE_TYPE_WIFI)
            })
            .ok_or(WifiError::Unavailable)
    }

    fn ssid_of(&self, access_point: OwnedObjectPath) -> Option<String> {
        let bytes: Vec<u8> =
            Proxy::new(self.bus.as_ref()?, NM, access_point, NM_ACCESS_POINT).ok()?.get_property("Ssid").ok()?;
        String::from_utf8(bytes).ok()
    }
}

impl HotspotWifi for NetworkManagerWifi {
    fn has_interface(&self) -> bool {
        self.device().is_ok()
    }

    fn is_visible(&self, ssid: String) -> Result<bool, WifiError> {
        let wireless = Proxy::new(self.bus()?, NM, self.device()?, NM_WIRELESS).map_err(wifi_err)?;
        let last_scan = || wireless.get_property::<i64>("LastScan").unwrap_or_default();
        let scanned_before = last_scan();
        let _ = wireless.call::<_, _, ()>("RequestScan", &(HashMap::<&str, Value>::new(),));
        let deadline = std::time::Instant::now() + SCAN_TIMEOUT;
        loop {
            let scan_finished = last_scan() != scanned_before;
            let access_points: Vec<OwnedObjectPath> = wireless.call("GetAllAccessPoints", &()).map_err(wifi_err)?;
            if access_points.into_iter().any(|ap| self.ssid_of(ap).is_some_and(|name| name == ssid)) {
                return Ok(true);
            }
            if scan_finished || std::time::Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(Duration::from_millis(300));
        }
    }

    fn join(&self, ssid: String, passphrase: String) -> Result<(), WifiError> {
        let bus = self.bus()?;
        let device = self.device()?;
        let settings: HashMap<&str, HashMap<&str, Value>> = HashMap::from([
            (
                "connection",
                HashMap::from([
                    ("id", Value::from(ssid.as_str())),
                    ("type", Value::from("802-11-wireless")),
                    ("autoconnect", Value::from(false)),
                ]),
            ),
            (
                "802-11-wireless",
                HashMap::from([
                    ("ssid", Value::from(ssid.as_bytes().to_vec())),
                    ("mode", Value::from("infrastructure")),
                ]),
            ),
            (
                "802-11-wireless-security",
                HashMap::from([("key-mgmt", Value::from("wpa-psk")), ("psk", Value::from(passphrase.as_str()))]),
            ),
        ]);
        let any = ObjectPath::try_from("/").map_err(wifi_err)?;
        Proxy::new(bus, NM, NM_PATH, NM)
            .and_then(|nm| {
                nm.call::<_, _, (OwnedObjectPath, OwnedObjectPath)>(
                    "AddAndActivateConnection",
                    &(settings, &device, any),
                )
            })
            .map_err(wifi_err)?;

        let device = Proxy::new(bus, NM, device, NM_DEVICE).map_err(wifi_err)?;
        let deadline = Instant::now() + JOIN_TIMEOUT;
        while Instant::now() < deadline {
            match device.get_property::<u32>("State").map_err(wifi_err)? {
                NM_DEVICE_STATE_ACTIVATED => return Ok(()),
                NM_DEVICE_STATE_FAILED => return Err(WifiError::Failed { reason: "the connection failed".to_owned() }),
                _ => thread::sleep(Duration::from_millis(300)),
            }
        }
        Err(WifiError::Failed { reason: "the connection did not finish in time".to_owned() })
    }

    fn current_ssid(&self) -> Option<String> {
        let access_point: OwnedObjectPath = Proxy::new(self.bus.as_ref()?, NM, self.device().ok()?, NM_WIRELESS)
            .ok()?
            .get_property("ActiveAccessPoint")
            .ok()?;
        (access_point.as_str() != "/").then(|| self.ssid_of(access_point)).flatten()
    }

    fn leave(&self) -> Result<(), WifiError> {
        Proxy::new(self.bus()?, NM, self.device()?, NM_DEVICE)
            .and_then(|device| device.call::<_, _, ()>("Disconnect", &()))
            .map_err(wifi_err)
    }
}
