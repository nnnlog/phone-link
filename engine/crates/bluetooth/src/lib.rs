//! The phone's Bluetooth Classic device, shared by every feature that reaches the phone over
//! Bluetooth (instant hotspot's side channel today).
//!
//! It covers which device paired with this Mac is the phone, the phone app's Bluetooth facts, and
//! the pairing a feature runs as the first step of the user's action when no paired device is the
//! phone.
//!
//! No feature owns the pairing, so there is no separate
//! pairing step for the user: using a Bluetooth feature pairs when it has to.
//!
//! - **Phone state**: the Bluetooth provider's `connect` reply keys (this module's connect provider)
//!   and the answers to the Bluetooth `/permission` request write the phone record's
//!   [`BluetoothRecord`].
//! - **Pairing** ([`Bluetooth::pair`]): the phone app's Bluetooth permission when its capabilities
//!   ask for it first, the `BluetoothAdvertise` permission that makes the phone discoverable, a
//!   device-status sync for its current Bluetooth name, then a discovery by name and the pairing
//!   through the [`PhoneBluetooth`] edge. One pairing runs at a time; a feature that asks meanwhile
//!   waits for it and then finds the phone paired.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod edge;

use phonelink_core::config::features::BluetoothConfig;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use phonelink_app::consts::{permission, platform};
use phonelink_app::{
    AppOutcome, BluetoothRecord, ConnectContext, ConnectContribution, ConnectContributor, ConnectProvider, PhoneRecord,
    RequestOptions,
};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::DcgClient;

pub use edge::{NoBluetooth, PairingError, PhoneBluetooth, RfcommError};

use phonelink_features::{FeatureEnv, FeatureError};

/// The Android SDK version from which the phone reports its Bluetooth name only once its app holds
/// the Bluetooth permission.
const ANDROID_SDK_BLUETOOTH_NAME: u32 = 31;

/// Why the phone's Bluetooth device is not at hand.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BluetoothFailure {
    #[error("Bluetooth is off")]
    BluetoothOff,
    #[error("the phone did not grant its Bluetooth permission")]
    PermissionNotGranted,
    #[error("the phone did not become discoverable")]
    NotDiscoverable,
    #[error("the phone was not found")]
    PhoneNotFound,
    #[error("pairing failed: {0}")]
    PairingFailed(String),
    #[error("{0}")]
    Internal(String),
}

/// The phone's Bluetooth device, for the features that use it.
pub struct Bluetooth {
    env: FeatureEnv,
    config: BluetoothConfig,
    edge: Arc<dyn PhoneBluetooth>,
    /// Held for the whole of a pairing, so a second one waits and then finds the phone paired.
    pairing: tokio::sync::Mutex<()>,
}

/// The connect provider: adds nothing; reads the Bluetooth keys of the reply.
struct BluetoothContributor;

#[async_trait]
impl ConnectContributor for BluetoothContributor {
    async fn contribute(&self, _ctx: &ConnectContext<'_>) -> ConnectContribution {
        ConnectContribution::default()
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        ctx.app.update_record_for(ctx.phone, |r| r.bluetooth.apply_connect_reply(response));
    }
}

impl Bluetooth {
    /// Registers the connect provider. Call it once, before the App layer starts.
    pub fn install(
        env: &FeatureEnv,
        config: BluetoothConfig,
        edge: Arc<dyn PhoneBluetooth>,
    ) -> Result<Arc<Self>, FeatureError> {
        env.app.register_connect_contributor(ConnectProvider::Bluetooth, Arc::new(BluetoothContributor))?;
        Ok(Arc::new(Self { env: env.clone(), config, edge, pairing: tokio::sync::Mutex::new(()) }))
    }

    /// The OS edge, for a feature's RFCOMM channels to the paired phone.
    pub fn edge(&self) -> Arc<dyn PhoneBluetooth> {
        self.edge.clone()
    }

    fn record(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        self.env.app.phone_record().filter(|r| r.phone().is_same_client(phone))
    }

    /// The phone's names as a Bluetooth device may carry them: its display name, its device-list
    /// name and, for an Android phone, the Bluetooth name its device status reports.
    pub fn phone_names(record: &PhoneRecord) -> Vec<String> {
        let bluetooth_name =
            (record.platform == platform::ANDROID).then(|| record.status_keys.bluetooth_name.clone()).flatten();
        [record.display_name.clone(), record.name.clone(), bluetooth_name].into_iter().flatten().collect()
    }

    /// The address of the first device paired with this Mac that is `phone`; `None` when none is.
    pub async fn paired_phone(&self, phone: &DcgClient) -> Result<Option<String>, BluetoothFailure> {
        let record = self.record(phone).ok_or_else(no_record)?;
        let edge = self.edge.clone();
        let names = Self::phone_names(&record);

        let (radio_on, paired) = blocking(move || {
            let on = edge.radio_on();
            (on, if on { edge.paired_phone(names) } else { None })
        })
        .await?;

        if !radio_on {
            return Err(BluetoothFailure::BluetoothOff);
        }
        Ok(paired)
    }

    /// The address of `phone`'s Bluetooth device, pairing with it first when no device paired with
    /// this Mac is the phone. `scenario` names the feature whose action needs the device in the
    /// `/permission` requests' traces.
    pub async fn pair(&self, phone: &DcgClient, scenario: &str) -> Result<String, BluetoothFailure> {
        let _pairing = self.pairing.lock().await;
        if let Some(address) = self.paired_phone(phone).await? {
            return Ok(address);
        }

        let record = self.record(phone).ok_or_else(no_record)?;
        info!(bluetooth_scenario = scenario, "Bluetooth pairing started; no paired device is the phone");
        self.make_discoverable(phone, &record, scenario).await?;
        self.sync_bluetooth_name(&record).await;

        let names = Self::phone_names(&self.record(phone).unwrap_or(record));
        self.discover_and_pair(names).await
    }

    /// The permissions pairing needs: the phone app's Bluetooth permission when its capabilities ask
    /// for it first, then the `BluetoothAdvertise` permission that makes the phone discoverable.
    async fn make_discoverable(
        &self,
        phone: &DcgClient,
        record: &PhoneRecord,
        scenario: &str,
    ) -> Result<(), BluetoothFailure> {
        if record.bluetooth.capabilities & BluetoothRecord::REQUEST_BLUETOOTH_ADVERTISE_CONNECT_SCAN != 0
            && !record.bluetooth.permission
            && self.request_permission(phone, scenario).await? != Some(true)
        {
            return Err(BluetoothFailure::PermissionNotGranted);
        }

        if self.permission(permission::BLUETOOTH_ADVERTISE, "SetDiscoverableMode", scenario).await? != Some(true) {
            return Err(BluetoothFailure::NotDiscoverable);
        }
        Ok(())
    }

    /// Before the phone app holds the permission it may report a name other than its Bluetooth
    /// name; a device-status sync brings the current `bluetoothName`. Its outcome is logged only.
    async fn sync_bluetooth_name(&self, record: &PhoneRecord) {
        if record.platform == platform::ANDROID && record.android_sdk_version >= ANDROID_SDK_BLUETOOTH_NAME {
            let request = phonelink_device_status::phone_link::sync_request("SetDiscoverableMode");
            match self.env.app.request(request).await {
                Ok(r) => info!(outcome = ?r.outcome, "Bluetooth name device-status sync finished"),
                Err(e) => {
                    warn!(error = %e, "Bluetooth name device-status sync not sent; discovery looks for the names on record")
                }
            }
        }
    }

    /// The discovery of a device carrying one of `names` and the pairing with it, through the edge.
    async fn discover_and_pair(&self, names: Vec<String>) -> Result<String, BluetoothFailure> {
        let edge = self.edge.clone();
        let timeout = phonelink_core::time::duration_ms(self.config.discovery_timeout);

        match blocking(move || edge.discover_and_pair(names, timeout)).await? {
            Ok(address) => {
                info!("Bluetooth pairing finished");
                Ok(address)
            }
            Err(PairingError::NotFound) => Err(BluetoothFailure::PhoneNotFound),
            Err(PairingError::Failed { reason }) => Err(BluetoothFailure::PairingFailed(reason)),
            Err(e) => Err(BluetoothFailure::PairingFailed(e.to_string())),
        }
    }

    /// Makes the Mac read the service records of the paired device at `address` again, for a feature
    /// whose RFCOMM service was missing from them: the Mac reads them when it pairs and a service the
    /// phone opened later is not among them. The outcome is logged only; the feature's next attempt
    /// finds the service or not.
    pub async fn refresh_services(&self, address: &str, service_uuid: &str) {
        let edge = self.edge.clone();
        let (address, uuid) = (address.to_string(), service_uuid.to_string());
        let timeout = phonelink_core::time::duration_ms(self.config.service_refresh_timeout);

        match blocking(move || edge.refresh_services(address, uuid, timeout)).await {
            Ok(Ok(found)) => info!(found, service_uuid, "Bluetooth service records refreshed"),
            Ok(Err(e)) => warn!(error = %e, service_uuid, "Bluetooth service records not refreshed"),
            Err(e) => warn!(error = %e, service_uuid, "Bluetooth service records not refreshed"),
        }
    }

    /// The phone app's Bluetooth `/permission` request for `scenario`; the answer that decides is
    /// recorded and returned, `None` for the others.
    pub async fn request_permission(
        &self,
        phone: &DcgClient,
        scenario: &str,
    ) -> Result<Option<bool>, BluetoothFailure> {
        let granted = self.permission(permission::BLUETOOTH, "RequestBluetoothPermission", scenario).await?;
        if let Some(granted) = granted {
            self.env.app.update_record_for(phone, |r| r.bluetooth.permission = granted);
        }
        Ok(granted)
    }

    /// Sends a user-initiated `/permission` request for `bit`; `Some(granted)` for the outcomes that
    /// decide, `None` for the others.
    async fn permission(&self, bit: u32, trigger: &str, scenario: &str) -> Result<Option<bool>, BluetoothFailure> {
        let options =
            RequestOptions::user_initiated().with_timeout(self.config.permission_timeout).named(trigger, scenario);
        let request = self.env.app.permission_request(bit, false, options);
        let response = self.env.app.request(request).await.map_err(|e| BluetoothFailure::Internal(e.to_string()))?;
        info!(bit, outcome = ?response.outcome, "Bluetooth /permission request finished");

        Ok(match response.outcome {
            AppOutcome::Success | AppOutcome::Accepted => Some(true),
            AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission => Some(false),
            _ => None,
        })
    }
}

fn no_record() -> BluetoothFailure {
    BluetoothFailure::Internal("the phone record is not loaded".into())
}

/// Runs `f` on the blocking pool; a panic in it is an internal failure.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, BluetoothFailure> {
    tokio::task::spawn_blocking(f).await.map_err(|e| BluetoothFailure::Internal(format!("an edge call failed: {e}")))
}
