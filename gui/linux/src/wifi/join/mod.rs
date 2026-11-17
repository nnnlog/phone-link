//! Joining the phone's hotspot: finding its access point, with a directed scan when the last scan
//! did not see it, and activating a volatile profile for it.

use std::collections::HashMap;
use std::ffi::CStr;
use std::thread;
use std::time::{Duration, Instant};

use phonelink_hotspot::edge::WifiError;
use tracing::warn;
use zbus::blocking::Proxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{NM_WIRELESS, NetworkManagerWifi, failed, nm_proxy, wifi_err};

#[cfg(test)]
mod tests;

const NM_ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const NM_ACTIVE_CONNECTION: &str = "org.freedesktop.NetworkManager.Connection.Active";
const NM_ACTIVE_CONNECTION_STATE_ACTIVATED: u32 = 2;
const NM_ACTIVE_CONNECTION_STATE_DEACTIVATING: u32 = 3;
const NM_ACTIVE_CONNECTION_STATE_DEACTIVATED: u32 = 4;
const JOIN_TIMEOUT: Duration = Duration::from_secs(30);
const SCAN_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(300);

/// How far an activation has got, from its active connection's `State`.
#[derive(Debug, PartialEq, Eq)]
enum Activation {
    Pending,
    Activated,
    Ended,
}

fn activation(state: u32) -> Activation {
    match state {
        NM_ACTIVE_CONNECTION_STATE_ACTIVATED => Activation::Activated,
        // An active connection's state only moves forward, so one that is going down never comes up.
        NM_ACTIVE_CONNECTION_STATE_DEACTIVATING | NM_ACTIVE_CONNECTION_STATE_DEACTIVATED => Activation::Ended,
        _ => Activation::Pending,
    }
}

/// The options of a scan for `ssid`: a directed scan, which a hotspot that does not broadcast its
/// name answers too.
fn scan_options(ssid: &str) -> HashMap<&'static str, Value<'static>> {
    HashMap::from([("ssids", Value::from(vec![ssid.as_bytes().to_vec()]))])
}

/// The options of the hotspot's `AddAndActivateConnection2`. A volatile profile lives in memory only
/// while it is active and is deleted when it deactivates, so joining writes nothing to
/// `/etc/NetworkManager/system-connections` and leaves no profile behind.
fn join_options() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([("persist", Value::from("volatile"))])
}

/// The hotspot's profile, for `user` only: on some systems a profile for every user needs an
/// administrator's approval. It holds the passphrase but no key management: `NetworkManager` takes it
/// from the access point the profile is activated on (WPA2, WPA3, or both), as `nmcli device wifi
/// connect` has it do. It is hidden, so a hotspot that does not broadcast its name is joined too.
fn join_settings<'a>(
    ssid: &'a str,
    passphrase: &'a str,
    user: &str,
) -> HashMap<&'static str, HashMap<&'static str, Value<'a>>> {
    HashMap::from([
        (
            "connection",
            HashMap::from([
                ("id", Value::from(ssid)),
                ("type", Value::from("802-11-wireless")),
                ("autoconnect", Value::from(false)),
                ("permissions", Value::from(vec![format!("user:{user}")])),
            ]),
        ),
        (
            "802-11-wireless",
            HashMap::from([
                ("ssid", Value::from(ssid.as_bytes().to_vec())),
                ("mode", Value::from("infrastructure")),
                ("hidden", Value::from(true)),
            ]),
        ),
        ("802-11-wireless-security", HashMap::from([("psk", Value::from(passphrase))])),
    ])
}

/// The login name of the user this process runs as, from the user database.
#[expect(unsafe_code, reason = "getpwuid_r has no safe wrapper in the dependencies")]
fn login_name() -> Result<String, WifiError> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: passwd is plain data; getpwuid_r fills it in.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer points to memory owned here that outlives the call, and the buffer's
    // length is passed with it.
    let status = unsafe { libc::getpwuid_r(uid, &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    if status != 0 || found.is_null() || entry.pw_name.is_null() {
        return Err(failed(format!("the user database has no entry for user {uid} (status {status})")));
    }
    // SAFETY: getpwuid_r succeeded, so pw_name points to a NUL-terminated string in `buffer`.
    let name = unsafe { CStr::from_ptr(entry.pw_name) };
    name.to_str().map(str::to_owned).map_err(|_| failed(format!("the login name of user {uid} is not UTF-8")))
}

impl NetworkManagerWifi {
    /// Joins the network named `ssid` with `passphrase` through its access point, the one the last scan
    /// saw or, when it is gone, the one a new scan finds.
    pub(super) fn join_network(&self, ssid: &str, passphrase: &str) -> Result<(), WifiError> {
        let device = self.device()?;
        self.ensure_usable(&device)?;
        let wireless = nm_proxy(self.bus()?, &device, NM_WIRELESS).map_err(wifi_err)?;
        let access_point = match self.access_point(&wireless, ssid)? {
            Some(access_point) => access_point,
            None => self.scan_for(&device, ssid)?.ok_or_else(|| failed("the network is not visible".to_owned()))?,
        };

        let user = login_name()?;
        let (_profile, active, _result): (OwnedObjectPath, OwnedObjectPath, HashMap<String, OwnedValue>) = self
            .manager()?
            .call(
                "AddAndActivateConnection2",
                &(join_settings(ssid, passphrase, &user), &device, &access_point, join_options()),
            )
            .map_err(wifi_err)?;

        self.wait_until_activated(&active)
    }

    /// The name of the network of `access_point`; `None` when it is gone or its name is not UTF-8.
    pub(super) fn ssid_of(&self, access_point: &OwnedObjectPath) -> Option<String> {
        let bytes: Vec<u8> =
            nm_proxy(self.bus.as_ref()?, access_point, NM_ACCESS_POINT).ok()?.get_property("Ssid").ok()?;
        String::from_utf8(bytes).ok()
    }

    /// The access point of the network named `ssid` among those the device last saw: the strongest,
    /// then the one whose path sorts first. An access point that disappears while it is read is passed
    /// over.
    fn access_point(&self, wireless: &Proxy<'_>, ssid: &str) -> Result<Option<OwnedObjectPath>, WifiError> {
        let bus = self.bus()?;
        let access_points: Vec<OwnedObjectPath> = wireless.call("GetAllAccessPoints", &()).map_err(wifi_err)?;
        Ok(access_points
            .into_iter()
            .filter(|access_point| self.ssid_of(access_point).is_some_and(|name| name == ssid))
            .filter_map(|access_point| {
                let strength: u8 = nm_proxy(bus, &access_point, NM_ACCESS_POINT).ok()?.get_property("Strength").ok()?;
                Some((strength, access_point))
            })
            .min_by(|(a_strength, a), (b_strength, b)| {
                b_strength.cmp(a_strength).then_with(|| a.as_str().cmp(b.as_str()))
            })
            .map(|(_, access_point)| access_point))
    }

    /// Scans for `ssid` and returns the access point of the network, waiting at most
    /// [`SCAN_TIMEOUT`] for the scan to find it or to end.
    pub(super) fn scan_for(&self, device: &OwnedObjectPath, ssid: &str) -> Result<Option<OwnedObjectPath>, WifiError> {
        let wireless = nm_proxy(self.bus()?, device, NM_WIRELESS).map_err(wifi_err)?;
        let last_scan = || wireless.get_property::<i64>("LastScan").map_err(wifi_err);
        let scanned_before = last_scan()?;
        if let Err(e) = wireless.call::<_, _, ()>("RequestScan", &(scan_options(ssid),)) {
            // A device that became unavailable (Wi-Fi switched off) refuses to scan.
            self.ensure_usable(device)?;
            return Err(wifi_err(e));
        }

        let deadline = Instant::now() + SCAN_TIMEOUT;
        loop {
            let scan_finished = last_scan()? != scanned_before;
            if let Some(access_point) = self.access_point(&wireless, ssid)? {
                return Ok(Some(access_point));
            }
            if scan_finished || Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(POLL);
        }
    }

    /// Waits for the activation `AddAndActivateConnection2` started, by its own active connection:
    /// the device can still read activated for the network it is leaving.
    fn wait_until_activated(&self, active: &OwnedObjectPath) -> Result<(), WifiError> {
        let connection = nm_proxy(self.bus()?, active, NM_ACTIVE_CONNECTION).map_err(wifi_err)?;
        let deadline = Instant::now() + JOIN_TIMEOUT;

        loop {
            // NetworkManager removes the object of an activation that failed, so a read that fails
            // means the attempt ended.
            let state = connection
                .get_property::<u32>("State")
                .map_err(|e| failed(format!("the connection ended before it came up: {e}")))?;
            match activation(state) {
                Activation::Activated => return Ok(()),
                Activation::Ended => return Err(failed("the connection failed".to_owned())),
                Activation::Pending if Instant::now() >= deadline => {
                    // Stop the attempt too, so the computer does not join after the failure is reported.
                    if let Err(e) = self.deactivate(active) {
                        warn!(error = %e, "the hotspot connection that did not come up in time was not stopped");
                    }
                    return Err(failed("the connection did not come up in time".to_owned()));
                }
                Activation::Pending => thread::sleep(POLL),
            }
        }
    }
}
