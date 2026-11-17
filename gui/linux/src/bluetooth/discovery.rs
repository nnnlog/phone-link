//! Discovery: a Bluetooth Classic scan on the adapter in use for the phone by its names, then pairing
//! with it when it is not paired yet, and marking it trusted.

use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};

use phonelink_bluetooth::PairingError;
use tracing::{info, warn};
use zbus::blocking::Proxy;
use zbus::zvariant::{OwnedObjectPath, Value};

use super::bluez::{ADAPTER, BLUEZ, Bluez, POLL};
use super::objects::{bool_of, chosen_adapter, devices_on, discovered_phone, engine_address, string_of};

/// How long a pairing waits, after the discovery, for the user to confirm the code on both devices.
const CONFIRMATION_WAIT: Duration = Duration::from_secs(120);

impl Bluez {
    /// Finds the phone named one of `names` within `timeout_ms`, pairs with it when it is not paired,
    /// and marks it trusted; its address in the engine's form.
    pub(super) fn discover_and_pair(&self, names: &[String], timeout_ms: u64) -> Result<String, PairingError> {
        let discovery_deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let failed = |what: &str, e: zbus::Error| PairingError::Failed { reason: format!("{what}: {e}") };
        let objects = self.objects().map_err(|e| failed("BlueZ did not answer", e))?;
        let (adapter_path, _) =
            chosen_adapter(&objects).ok_or(PairingError::Unexpected { message: "no Bluetooth adapter".to_owned() })?;
        let adapter_path = adapter_path.clone();
        let known_before: HashSet<String> =
            devices_on(&objects, adapter_path.as_str()).map(|(path, _)| path.to_string()).collect();

        let adapter = Proxy::new(&self.bus, BLUEZ, adapter_path.clone(), ADAPTER)
            .map_err(|e| failed("the adapter was not reached", e))?;
        // The phone is a Bluetooth Classic device; a Low Energy scan would only find what pairing cannot use.
        let filter = HashMap::from([("Transport", Value::from("bredr"))]);
        adapter
            .call::<_, _, ()>("SetDiscoveryFilter", &(filter,))
            .map_err(|e| failed("the discovery filter was not set", e))?;
        adapter.call::<_, _, ()>("StartDiscovery", &()).map_err(|e| failed("discovery did not start", e))?;
        let found = self.find_phone(adapter_path.as_str(), &known_before, names, discovery_deadline);
        if let Err(e) = adapter.call::<_, _, ()>("StopDiscovery", &()) {
            info!(error = %e, "Bluetooth discovery was not stopped");
        }

        let (path, paired, address) = found?.ok_or(PairingError::NotFound)?;
        if !paired {
            self.pair(&path, Instant::now() + CONFIRMATION_WAIT)?;
        }
        let trusted = self.device(&path).and_then(|device| device.set_property("Trusted", true).map_err(Into::into));
        if let Err(e) = trusted {
            warn!(error = %e, "the paired phone was not marked trusted");
        }
        Ok(engine_address(&address))
    }

    /// Polls `BlueZ`'s devices on `adapter` until one is the phone (see [`discovered_phone`]) or
    /// `deadline` passes: its object path, whether it is paired, and its address.
    fn find_phone(
        &self,
        adapter: &str,
        known_before: &HashSet<String>,
        names: &[String],
        deadline: Instant,
    ) -> Result<Option<(OwnedObjectPath, bool, String)>, PairingError> {
        loop {
            let objects = self.objects().map_err(|e| PairingError::Failed { reason: e.to_string() })?;
            if let Some((path, device)) = discovered_phone(&objects, adapter, known_before, names) {
                let address = string_of(device, "Address")
                    .ok_or(PairingError::Failed { reason: "the device has no address".to_owned() })?;
                return Ok(Some((path.clone(), bool_of(device, "Paired"), address)));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            thread::sleep(POLL.min(left));
        }
    }
}
