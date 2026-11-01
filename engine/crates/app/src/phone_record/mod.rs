//! The per-phone record of the App layer and the per-install identifiers.
//!
//! The record is what the DCG device list and the connect response teach the PC: contract version,
//! YPP version and capabilities, capability bits (`registeredPermissionTypes`), permission bits,
//! attributes, the "all features disabled" flag, and the device-status feature's Phone Link model
//! ([`StatusKeys`]) with the phone's status capabilities, which every inbound phone-content request
//! writes as well as the device-status feature. It is persisted through [`SecureStore`] under
//! `app/phone/<dcgClientId>.<Environment>`.

use serde::{Deserialize, Serialize};
use tracing::warn;

use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::config::AppConfig;
use phonelink_core::types::{DcgClient, DeviceInfoItem, Environment};

use super::consts::{asset_category, capability, permission, platform, status, ypp};

mod custom_data;
mod persist;
mod typed;

use custom_data::{json_flags, json_i32, json_string, json_u32, parse_custom_data};
#[cfg(feature = "test-support")]
pub use persist::{is_installation_id, new_app_session_id};
pub use persist::{load_or_create_installation_id, load_record, process_app_session_id, record_key, save_record};
use typed::{set_bool, set_i32, set_string, typed};

mod status_keys;
use status_keys::{OnWrongType, skipped};
pub use status_keys::{StatusChanges, StatusKeys};

/// Store key of the per-install `installationId`.
pub const INSTALLATION_ID_KEY: &str = "app/installation-id";
/// Prefix of the per-phone record keys.
pub const PHONE_RECORD_KEY_PREFIX: &str = "app/phone/";

/// `StatusCapabilities` flags.
pub mod status_capability {
    pub const SYNC_OVER_METERED_CONNECTION_REMOTE_TOGGLE: u32 = 0x1;
    pub const DEVICE_STATUS_INDICATOR: u32 = 0x2;
    pub const DEVICE_ACTION_INDICATOR: u32 = 0x4;
    pub const BATTERY_INDICATOR: u32 = 0x8;
    pub const BLUETOOTH_INDICATOR: u32 = 0x10;
    pub const DO_NOT_DISTURB_INDICATOR: u32 = 0x20;
    pub const NETWORK_SIGNAL_INDICATOR: u32 = 0x40;
    pub const RINGER_INDICATOR: u32 = 0x80;
    pub const WIFI_INDICATOR: u32 = 0x100;
    pub const DEVICE_STATUS_REQUEST_SUPPORT: u32 = 0x200;
    pub const AIRPLANE_MODE: u32 = 0x400;
    pub const PLAYER_INDICATOR: u32 = 0x800;
    pub const REENABLE_LTW_REMOTE_TOGGLE: u32 = 0x1000;
    /// Every flag; a `connect` reply's value outside it is ignored.
    pub const ALL: u32 = 0x1FFF;

    /// The phone has `bits`, or all of [`ALL`] (an indicator's support test).
    pub fn supports(capabilities: u32, bits: u32) -> bool {
        capabilities & ALL == ALL || capabilities & bits == bits
    }
}

/// `RingMyPhoneCapabilities` values of a `connect` reply's `capabilities.ringMyPhoneCapabilities`.
const RING_MY_PHONE_NONE: i32 = 0;
const RING_MY_PHONE_SUPPORTED: i32 = 1;

/// `ContentTransferCapabilities` flags of [`PhoneRecord::content_transfer`] (the connect response's
/// `capabilities.contentTransfer`).
pub mod content_transfer {
    pub const COPY_PASTE: i32 = 0x1;
    pub const DRAG_AND_DROP: i32 = 0x2;
    pub const COPY_PASTE_MIGRATED: i32 = 0x4;
}

/// One phone's record.
///
/// Field defaults are a **new record**'s values; the device list then writes
/// its values and Android defaults ([`PhoneRecord::apply_device_list`]) and every connect response
/// overwrites what it carries ([`PhoneRecord::apply_connect_response`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhoneRecord {
    pub phone_id: String,
    pub environment: Environment,

    // --- from the device list entry ---------------------------------------------------------
    /// `consentVersion` (`null` → `0`); a new record holds `-1`. Any value `>= 0` — every phone
    /// from the device list — subjects non-user-initiated requests to the `AsyncTrust` gate.
    pub consent_version: i64,
    pub name: Option<String>,
    pub os_name: Option<String>,
    /// The device list's `osVersion` (`None` when the entry has none). The phone's OS version is
    /// one value both writers set, the last one winning: read [`PhoneRecord::current_os_version`].
    pub os_version: Option<String>,
    pub client_version: Option<String>,
    pub client_type: Option<String>,
    /// The phone's feature capability strings (the device list entry's `capabilities`).
    pub device_capabilities: Vec<String>,

    // --- written by the device list and the connect response -------------------------------
    /// `registeredPermissionTypes` (`DeviceCapabilities` flags); a new record holds `0x10`.
    pub capability_bits: u32,
    pub ypp_capabilities: u32,
    pub ypp_version: i32,
    /// `StatusCapabilities` ([`status_capability`]): set by the device list, replaced by a
    /// `connect` reply, extended by each status key a `deviceStatus` item or `connect` reply carries.
    pub status_capabilities: u32,
    pub asset_category: u32,
    pub android_sdk_version: u32,
    pub locale: Option<String>,
    pub country_code: Option<String>,
    pub market: Option<String>,
    pub flight_ring: Option<String>,
    pub device_resolution: Option<String>,
    pub client_model_name: Option<u32>,
    pub client_model_revision: Option<u32>,
    /// `0` Unknown, `1` Android, `2` iOS.
    pub platform: i32,
    /// The last contract version a connect response reported (kept across device-list refreshes;
    /// differs from Phone Link on Windows).
    pub reported_contract_version: Option<f64>,
    /// `DevicePermissions` flags granted on the phone.
    pub permission_bits: u32,
    /// The `DevicePermissions` flags some writer has set or cleared ([`PhoneRecord::set_permission`]);
    /// a flag outside this set still holds its feature's default ([`PhoneRecord::permission_or`]).
    pub permission_known_bits: u32,

    // --- connect response only --------------------------------------------------------------
    /// `attributes.displayName` — the phone's name (PII).
    pub display_name: Option<String>,
    pub manufacturer: Option<String>,
    pub device_model: Option<String>,
    /// `attributes.sdkVersion` — the phone app's version.
    pub sdk_version: Option<String>,
    /// `attributes.osVersion` of the last connect reply, while no device-list application came after
    /// it (each one clears it): the phone's OS version is the last value either writer set
    /// ([`PhoneRecord::current_os_version`]).
    pub attributes_os_version: Option<String>,
    pub package_name: Option<String>,
    /// `attributes.userId` — the phone's MSA user name (PII).
    pub user_id: Option<String>,
    pub device_capability_negotiation: Option<i32>,
    pub wake_capabilities: Option<i32>,
    pub remoting_capabilities: Option<i32>,
    pub contacts_capabilities: Option<i32>,
    pub content_transfer: Option<i32>,
    pub input_injector_available: Option<bool>,
    pub black_screen_available: Option<bool>,
    pub two_phone_mode_enabled: Option<bool>,
    pub aadc_optional_data_collection_policy: Option<i32>,
    pub ignoring_battery_optimization: Option<bool>,
    pub ltw_a2d_migration_needed: Option<bool>,

    // --- local state ------------------------------------------------------------------------
    /// "All features disabled".
    pub all_features_disabled: bool,
    /// The device-status feature's Phone Link model ([`StatusKeys`]).
    pub status_keys: StatusKeys,
    /// The phone can ring on request: the last `connect` reply's
    /// `capabilities.ringMyPhoneCapabilities` (persisted; `false` until a reply says `1`).
    pub ring_my_phone_capable: bool,
    /// What the Bluetooth features know of the phone's Bluetooth ([`BluetoothRecord`]).
    pub bluetooth: BluetoothRecord,
    /// What instant hotspot knows of the phone ([`HotspotRecord`]).
    pub hotspot: HotspotRecord,
}

/// The phone app's Bluetooth facts every feature that reaches the phone over Bluetooth shares: what
/// the Bluetooth provider's `connect` reply keys and the Bluetooth `/permission` answers report.
///
/// Every fact is `false` (`0`) until something reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BluetoothRecord {
    /// The phone app holds its Bluetooth permission (`permissions.bluetooth`, or the answer to the
    /// Bluetooth `/permission` request).
    pub permission: bool,
    /// `capabilities.bluetoothCapabilities` (`RequestBluetoothAdvertiseConnectScan` = `1`).
    pub capabilities: u32,
}

impl BluetoothRecord {
    /// The only defined `bluetoothCapabilities` flag: the phone asks for its Bluetooth permission
    /// before it advertises, connects or scans.
    pub const REQUEST_BLUETOOTH_ADVERTISE_CONNECT_SCAN: u32 = 0x1;

    /// The Bluetooth provider's keys of a `connect` reply; each one present with its type overwrites
    /// its fact, a mistyped one is logged and ignored.
    pub fn apply_connect_reply(&mut self, response: &ValueSet) {
        if let Some(Value::Object(capabilities)) = response.get("capabilities")
            && let Some(v) = typed::<i32>(capabilities, "bluetoothCapabilities")
        {
            let bits = v as u32;
            if bits & Self::REQUEST_BLUETOOTH_ADVERTISE_CONNECT_SCAN == bits {
                self.capabilities = bits;
            } else {
                warn!(flags = v, "connect bluetoothCapabilities outside the defined flags; ignored");
            }
        }
        if let Some(Value::Object(permissions)) = response.get("permissions")
            && let Some(granted) = typed::<bool>(permissions, "bluetooth")
        {
            self.permission = granted;
        }
    }
}

/// Instant hotspot's facts about one phone.
///
/// What its `connect` replies and its `HotspotPermissionsSettingUpdate` commands report, whether
/// setup finished on this Mac, and the network the last connect joined.
///
/// Every fact is `false` (`None`) until something reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HotspotRecord {
    /// `capabilities.hotspot` has `HotspotCapable` (`1`).
    pub capable: bool,
    /// The user allowed instant hotspot on the phone (`hotspotUserConsent`).
    pub user_consent: bool,
    /// The OEM Bluetooth permission (`permissions.oem_bluetooth`, `oemBluetoothPermission`).
    pub oem_bluetooth_permission: bool,
    /// The OEM RFCOMM server is open: `oemBluetoothRfcommServerOpen` turns it on and never off;
    /// `oemBluetoothRfcommEnabled` sets it either way.
    pub oem_rfcomm_enabled: bool,
    /// Setup finished on this Mac (local; never sent).
    pub setup_complete: bool,
    /// The SSID the last successful connect joined (local; never sent): the hotspot reads as joined
    /// while the Mac's Wi-Fi is associated with a network of this name.
    pub ssid: Option<String>,
}

impl HotspotRecord {
    const HOTSPOT_CAPABLE: i32 = 0x1;

    /// Consent, the phone app's Bluetooth permission (`bluetooth`) and the OEM Bluetooth permission
    /// are all granted.
    pub fn phone_permissions_granted(&self, bluetooth: &BluetoothRecord) -> bool {
        self.user_consent && bluetooth.permission && self.oem_bluetooth_permission
    }

    /// The hotspot provider's keys of a `connect` reply and the OEM Bluetooth keys; each one present
    /// with its type overwrites its fact, a mistyped one is logged and ignored.
    pub fn apply_connect_reply(&mut self, response: &ValueSet) {
        if let Some(consent) = typed::<bool>(response, "hotspotUserConsent") {
            self.user_consent = consent;
        }
        if let Some(Value::Object(capabilities)) = response.get("capabilities")
            && let Some(v) = typed::<i32>(capabilities, "hotspot")
        {
            self.capable = v & Self::HOTSPOT_CAPABLE != 0;
        }
        if let Some(Value::Object(permissions)) = response.get("permissions")
            && let Some(granted) = typed::<bool>(permissions, "oem_bluetooth")
        {
            self.oem_bluetooth_permission = granted;
        }
        if typed::<bool>(response, "oemBluetoothRfcommServerOpen") == Some(true) {
            self.oem_rfcomm_enabled = true;
        }
    }

    /// A `HotspotPermissionsSettingUpdate`: each key the command carries overwrites its fact.
    pub fn apply_permissions_update(
        &mut self,
        consent: Option<bool>,
        oem_bluetooth_permission: Option<bool>,
        oem_rfcomm_enabled: Option<bool>,
    ) {
        if let Some(v) = consent {
            self.user_consent = v;
        }
        if let Some(v) = oem_bluetooth_permission {
            self.oem_bluetooth_permission = v;
        }
        if let Some(v) = oem_rfcomm_enabled {
            self.oem_rfcomm_enabled = v;
        }
    }
}

impl Default for PhoneRecord {
    fn default() -> Self {
        Self {
            phone_id: String::new(),
            environment: Environment::Prod,
            consent_version: -1,
            name: None,
            os_name: None,
            os_version: None,
            client_version: None,
            client_type: None,
            device_capabilities: Vec::new(),
            capability_bits: capability::NEW_RECORD,
            ypp_capabilities: 0,
            ypp_version: 0,
            status_capabilities: 0,
            asset_category: asset_category::UNKNOWN,
            android_sdk_version: 0,
            locale: None,
            country_code: None,
            market: None,
            flight_ring: None,
            device_resolution: None,
            client_model_name: None,
            client_model_revision: None,
            platform: platform::UNKNOWN,
            reported_contract_version: None,
            permission_bits: 0,
            permission_known_bits: 0,
            display_name: None,
            manufacturer: None,
            device_model: None,
            sdk_version: None,
            attributes_os_version: None,
            package_name: None,
            user_id: None,
            device_capability_negotiation: None,
            wake_capabilities: None,
            remoting_capabilities: None,
            contacts_capabilities: None,
            content_transfer: None,
            input_injector_available: None,
            black_screen_available: None,
            two_phone_mode_enabled: None,
            aadc_optional_data_collection_policy: None,
            ignoring_battery_optimization: None,
            ltw_a2d_migration_needed: None,
            all_features_disabled: false,
            status_keys: StatusKeys::default(),
            ring_my_phone_capable: false,
            bluetooth: BluetoothRecord::default(),
            hotspot: HotspotRecord::default(),
        }
    }
}

impl PhoneRecord {
    /// A new record for `phone`.
    pub fn new(phone: &DcgClient) -> Self {
        Self { phone_id: phone.id.clone(), environment: phone.env, ..Self::default() }
    }

    pub fn phone(&self) -> DcgClient {
        DcgClient::new(self.phone_id.clone(), self.environment)
    }

    /// The phone's contract version: `3.99` (the PC's) for an Android phone until a connect
    /// response reports one, then the last reported value.
    pub fn contract_version(&self, config: &AppConfig) -> f64 {
        self.reported_contract_version.unwrap_or(config.contract_version)
    }

    /// The phone has a post-notification permission: an Android phone whose major OS version
    /// ([`PhoneRecord::android_major_version`]) is `≥ 13`.
    pub fn post_notification_ask_supported(&self) -> bool {
        self.android_major_version().is_some_and(|major| major >= 13)
    }

    pub fn has_capability(&self, bits: u32) -> bool {
        self.capability_bits & bits == bits
    }

    pub fn has_permission(&self, bits: u32) -> bool {
        self.permission_bits & bits == bits
    }

    /// Sets or clears `bits` in the permission bits and marks them as reported.
    pub fn set_permission(&mut self, bits: u32, granted: bool) {
        if granted {
            self.permission_bits |= bits;
        } else {
            self.permission_bits &= !bits;
        }
        self.permission_known_bits |= bits;
    }

    /// One `DevicePermissions` flag: its value once any writer has set or cleared it, else
    /// `default` — the feature's default for a permission the phone has not reported yet (e.g.
    /// messages and contacts default to granted, notification access to denied).
    pub fn permission_or(&self, bit: u32, default: bool) -> bool {
        if self.permission_known_bits & bit == bit { self.permission_bits & bit == bit } else { default }
    }

    /// The phone's remoting capability flags: `0` until a connect reply (`capabilities.remoting`) or a
    /// `phoneApps` message (`phoneAppsSettings`) reports them.
    pub fn remoting_flags(&self) -> u32 {
        self.remoting_capabilities.map_or(0, |v| v as u32)
    }

    /// Writes the remoting capabilities a `phoneApps` message reports (its `phoneAppsSettings`; `0`
    /// when the object carries no `Int32` `phoneAppsCapabilities`). The connect reply writes them in
    /// [`PhoneRecord::apply_connect_response`]; the last writer wins.
    pub fn set_remoting_flags(&mut self, flags: u32) {
        self.remoting_capabilities = Some(flags as i32);
    }

    /// The phone came from the DCG device list (`consentVersion` present or defaulted to `0`).
    pub fn is_from_device_list(&self) -> bool {
        self.consent_version >= 0
    }

    /// `AsyncTrust` in the YPP capabilities; `iOS` peers (by the device list's `osName`) count as
    /// capable.
    pub fn is_async_trust_capable(&self) -> bool {
        self.ypp_capabilities & ypp::ASYNC_TRUST != 0
            || self.os_name.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("iOS"))
    }

    /// Writes the record from one device-list entry and its `customData` JSON (Android). The
    /// reported contract version is kept.
    pub fn apply_device_list(&mut self, item: &DeviceInfoItem) {
        self.consent_version = item.consent_version.unwrap_or(0);
        self.name = item.name.clone();
        self.os_name = item.os_name.clone();

        // One OS version, last writer wins: the entry's value (absent → none) supersedes an earlier
        // connect reply's `attributes.osVersion`.
        self.os_version = item.os_version.clone();
        self.attributes_os_version = None;

        self.client_version = item.client_version.clone();
        self.client_type = item.client_type.clone();
        self.device_capabilities = item.capabilities.clone().unwrap_or_default();

        if let Some(os) = item.os_name.as_deref() {
            if os.eq_ignore_ascii_case("Android") {
                self.platform = platform::ANDROID;
            } else if os.eq_ignore_ascii_case("iOS") {
                self.platform = platform::IOS;
            }
        }

        let custom = parse_custom_data(item.custom_data.as_deref());
        self.apply_custom_data_capabilities(&custom);
        self.apply_custom_data_attributes(&custom);
    }

    /// Writes the capability keys of a device-list entry's `customData`: asset category, capability
    /// bits, YPP and status capabilities.
    fn apply_custom_data_capabilities(&mut self, custom: &serde_json::Map<String, serde_json::Value>) {
        let field = |name: &str| custom.get(name).filter(|v| !v.is_null());

        // `assetCategory` first: the default capability bits depend on it.
        if let Some(v) = field("assetCategory") {
            match json_flags(v, &asset_category::NAMES) {
                Some(c) => self.asset_category = c,
                None => {
                    warn!(raw_asset_category = %v, "device-list assetCategory unparsable; the record keeps its value")
                }
            }
        }

        self.apply_custom_data_permission_types(field("registeredPermissionTypes"));

        if let Some(v) = field("yppCapabilities") {
            match json_flags(v, &ypp::NAMES) {
                Some(caps) => self.ypp_capabilities = caps | (self.ypp_capabilities & ypp::ASYNC_TRUST),
                None => {
                    warn!(raw_ypp_capabilities = %v, "device-list yppCapabilities unparsable; the record keeps its value")
                }
            }
        }
        self.ypp_version = field("yppVersion").and_then(json_i32).unwrap_or(ypp::ANDROID_DEFAULT_VERSION);

        // Re-applied on every refresh, so a `customData` value overwrites what a connect reply set.
        match field("statusCapabilities").and_then(json_u32) {
            Some(v) => self.status_capabilities = v,
            None => {
                if self.status_capabilities == 0 && self.platform == platform::ANDROID {
                    self.status_capabilities = status::ANDROID_DEFAULT_CAPABILITIES;
                }
            }
        }
    }

    /// Writes `customData`'s `registeredPermissionTypes` as the capability bits; a new record without
    /// a parsable value gets the Android default of its asset category.
    fn apply_custom_data_permission_types(&mut self, value: Option<&serde_json::Value>) {
        match value.map(|v| (v, json_u32(v))) {
            Some((_, Some(bits))) => self.capability_bits = bits,
            other => {
                if let Some((v, None)) = other {
                    warn!(raw_permission_types = %v, "device-list registeredPermissionTypes unparsable; treated as absent");
                }
                if self.capability_bits == capability::NEW_RECORD {
                    self.capability_bits = if self.asset_category == asset_category::TABLET {
                        capability::ANDROID_TABLET_DEFAULT
                    } else {
                        capability::ANDROID_PHONE_DEFAULT
                    };
                }
            }
        }
    }

    /// Writes the descriptive keys of a device-list entry's `customData`: SDK version, locale,
    /// market, flight ring, resolution and model.
    fn apply_custom_data_attributes(&mut self, custom: &serde_json::Map<String, serde_json::Value>) {
        let field = |name: &str| custom.get(name).filter(|v| !v.is_null());

        self.android_sdk_version =
            field("androidSdkVersion").and_then(json_u32).unwrap_or(status::ANDROID_DEFAULT_SDK_VERSION);
        if let Some(locale) = field("defaultLocale").and_then(json_string) {
            self.country_code = Some(locale.clone());
            self.locale = Some(locale);
        }
        if let Some(s) = field("market").and_then(json_string) {
            self.market = Some(s);
        }
        if let Some(s) = field("flightRing").and_then(json_string) {
            self.flight_ring = Some(s);
        }
        if let Some(s) = field("deviceResolution").and_then(json_string) {
            self.device_resolution = Some(s);
        }
        if let Some(n) = field("clientModelName").and_then(json_u32) {
            self.client_model_name = Some(n);
        }
        if let Some(n) = field("clientModelRevision").and_then(json_u32) {
            self.client_model_revision = Some(n);
        }
    }

    /// Applies the non-feature keys of a connect response, before the feature providers read
    /// theirs: **`DeviceInfo` first** — contract version, platform, capability bits (with Calls added
    /// for an Android phone at contract `<= 3.93`) — then the rest of `DeviceInfo`, Ypp,
    /// `SyncTrustMigration`, `GlobalPermissions` and `AccountId`. Each key is read only with its stated
    /// type; a mistyped one is ignored.
    pub fn apply_connect_response(&mut self, values: &ValueSet, config: &AppConfig) {
        let attributes = typed::<&ValueSet>(values, "attributes");
        let capabilities = typed::<&ValueSet>(values, "capabilities");

        // DeviceInfo: what the feature gates read.
        self.apply_connect_gate_keys(values, attributes, capabilities, config);

        // DeviceInfo: the rest.
        if let Some(a) = attributes {
            self.apply_connect_attributes(a);
        }
        if let Some(c) = capabilities {
            self.apply_connect_capabilities(c);
        }
        if let Some(u) = typed::<&ValueSet>(values, "userAttributes") {
            set_i32(&mut self.aadc_optional_data_collection_policy, u, "aadcOptionalDataCollectionPolicy");
        }
        if let Some(d) = typed::<&ValueSet>(values, "deviceConfiguration") {
            set_bool(&mut self.ignoring_battery_optimization, d, "ignoringBatteryOptimization");
        }

        // Ypp.
        self.apply_connect_ypp(values);

        // SyncTrustMigration (stored only).
        set_bool(&mut self.ltw_a2d_migration_needed, values, "ltwA2DMigrationNeeded");

        // GlobalPermissions.
        if let Some(p) = typed::<&ValueSet>(values, "permissions") {
            for (name, bit) in [
                ("post_notifications", permission::POST_NOTIFICATIONS),
                ("notification_sensitive", permission::NOTIFICATIONS_SENSITIVE),
                ("auto_launch", permission::AUTO_LAUNCH),
            ] {
                if let Some(granted) = typed::<bool>(p, name) {
                    self.set_permission(bit, granted);
                }
            }
        }

        // AccountId.
        if let Some(a) = attributes {
            set_string(&mut self.user_id, a, "userId");
        }
    }

    /// The `DeviceInfo` keys of a connect response the feature gates read: contract version,
    /// platform and capability bits (with Calls added for an Android phone at contract `<= 3.93`).
    fn apply_connect_gate_keys(
        &mut self,
        values: &ValueSet,
        attributes: Option<&ValueSet>,
        capabilities: Option<&ValueSet>,
        config: &AppConfig,
    ) {
        match typed::<f64>(values, "contractVersion") {
            Some(v) if v.is_finite() => self.reported_contract_version = Some(v),
            Some(v) => warn!(contract_version = v, "connect contractVersion is not a finite number; ignored"),
            None => {}
        }

        if let Some(p) = attributes.and_then(|a| typed::<i32>(a, "platform")) {
            self.platform = p;
        }

        if let Some(bits) = capabilities.and_then(|c| typed::<i32>(c, "registeredPermissionTypes")) {
            let mut bits = bits as u32;
            if self.platform == platform::ANDROID
                && self.contract_version(config) <= capability::CALLS_ADDED_UP_TO_CONTRACT
            {
                bits |= capability::CALLS;
            }
            self.capability_bits = bits;
        }
    }

    /// The `attributes` object of a connect response: names, versions, locale and model.
    fn apply_connect_attributes(&mut self, a: &ValueSet) {
        set_string(&mut self.display_name, a, "displayName");
        set_string(&mut self.manufacturer, a, "manufacturer");
        set_string(&mut self.device_model, a, "deviceModel");
        set_string(&mut self.sdk_version, a, "sdkVersion");
        set_string(&mut self.attributes_os_version, a, "osVersion");
        set_string(&mut self.flight_ring, a, "flightRing");
        set_string(&mut self.locale, a, "defaultLocale");
        set_string(&mut self.country_code, a, "countryCode");
        set_string(&mut self.market, a, "market");
        set_string(&mut self.package_name, a, "packageName");

        if let Some(s) = typed::<&str>(a, "assetCategory") {
            match s.trim().parse::<u32>() {
                Ok(c) => self.asset_category = c,
                Err(_) => warn!(raw_asset_category = s, "connect attributes.assetCategory is not an integer; ignored"),
            }
        }
        if let Some(v) = typed::<i32>(a, "androidSdkVersion").and_then(|v| u32::try_from(v).ok()) {
            self.android_sdk_version = v;
        }
        if let Some(v) = typed::<i32>(a, "clientModelName").and_then(|v| u32::try_from(v).ok()) {
            self.client_model_name = Some(v);
        }
        if let Some(v) = typed::<i32>(a, "clientModelRevision").and_then(|v| u32::try_from(v).ok()) {
            self.client_model_revision = Some(v);
        }
    }

    /// The `capabilities` object of a connect response: the per-feature capability values and flags.
    fn apply_connect_capabilities(&mut self, c: &ValueSet) {
        set_i32(&mut self.device_capability_negotiation, c, "deviceCapabilityNegotiation");
        set_i32(&mut self.wake_capabilities, c, "wakeCapabilities");
        set_i32(&mut self.remoting_capabilities, c, "remoting");
        set_i32(&mut self.contacts_capabilities, c, "contacts");
        set_i32(&mut self.content_transfer, c, "contentTransfer");
        set_bool(&mut self.input_injector_available, c, "inputInjectorAvailable");
        set_bool(&mut self.black_screen_available, c, "blackScreenAvailable");
        set_bool(&mut self.two_phone_mode_enabled, c, "isTwoPhoneModeEnabled");
    }

    /// The YPP keys of a connect response: `yppVersion` (`-1` when absent and unset) and
    /// `yppCapabilities`.
    fn apply_connect_ypp(&mut self, values: &ValueSet) {
        match values.get("yppVersion") {
            None => {
                if self.ypp_version == 0 {
                    self.ypp_version = -1;
                }
            }
            Some(Value::Int32(v)) => self.ypp_version = *v,
            Some(other) => {
                warn!(
                    wire_key = "yppVersion",
                    value_type = other.type_name(),
                    "connect response key of another type ignored"
                )
            }
        }

        if let Some(caps) = typed::<i32>(values, "yppCapabilities") {
            self.ypp_capabilities = caps as u32;
        }
    }

    /// One `deviceStatus` item, in this order: the status keys (adding their capability bits),
    /// the phone's friendly name from a non-blank `displayName`, then the flags of other features —
    /// `isTwoPhoneModeEnabled` and the `ContentTransfer*` bits of
    /// [`PhoneRecord::content_transfer`]. A key of another `DataValueType` is logged and skipped,
    /// the rest applied. `ltwUpdate` belongs to the update banner, which is not built.
    pub fn apply_device_status_item(&mut self, item: &ValueSet) {
        self.status_capabilities |= self.status_keys.apply(item);

        match item.get("displayName") {
            None => {}
            Some(Value::String(name)) => {
                if !name.trim().is_empty() {
                    self.display_name = Some(name.clone());
                }
            }
            Some(other) => skipped("displayName", other, OnWrongType::Ignore),
        }

        match item.get("isTwoPhoneModeEnabled") {
            None => {}
            Some(Value::Bool(b)) => self.two_phone_mode_enabled = Some(*b),
            Some(other) => skipped("isTwoPhoneModeEnabled", other, OnWrongType::Warn),
        }

        for (key, flag) in [
            ("ContentTransferCopyPasteSupported", content_transfer::COPY_PASTE),
            ("ContentTransferDragDropSupported", content_transfer::DRAG_AND_DROP),
        ] {
            match item.get(key) {
                None => {}
                Some(Value::Bool(supported)) => {
                    let bits = self.content_transfer.unwrap_or(0);
                    self.content_transfer = Some(if *supported { bits | flag } else { bits & !flag });
                }
                Some(other) => skipped(key, other, OnWrongType::Warn),
            }
        }
    }

    /// The device-status part of a `connect` reply: `capabilities.statusCapabilities` replaces the
    /// status capabilities when it lies within [`status_capability::ALL`], and
    /// `capabilities.ringMyPhoneCapabilities` sets whether the phone can ring (`Int32 1` → yes; `0`, a
    /// missing key, another type or value → no; no `capabilities` object → unchanged); then the
    /// top-level status keys apply and add their bits. `ltwUpdate` belongs to a feature that is not
    /// built.
    pub fn apply_device_status_connect_reply(&mut self, response: &ValueSet) {
        match response.get("capabilities") {
            None => {}
            Some(Value::Object(capabilities)) => {
                match capabilities.get("statusCapabilities") {
                    None => {}
                    Some(Value::Int32(v)) => {
                        let bits = *v as u32;
                        if bits & status_capability::ALL == bits {
                            self.status_capabilities = bits;
                        } else {
                            warn!(flags = v, "connect statusCapabilities outside the defined flags; ignored");
                        }
                    }
                    Some(other) => skipped("capabilities.statusCapabilities", other, OnWrongType::Ignore),
                }

                self.ring_my_phone_capable = match capabilities.get("ringMyPhoneCapabilities") {
                    Some(Value::Int32(RING_MY_PHONE_SUPPORTED)) => true,
                    Some(Value::Int32(RING_MY_PHONE_NONE)) | None => false,
                    Some(Value::Int32(v)) => {
                        warn!(code = v, "connect ringMyPhoneCapabilities not a known value; the phone cannot ring");
                        false
                    }
                    Some(other) => {
                        skipped("capabilities.ringMyPhoneCapabilities", other, OnWrongType::Warn);
                        false
                    }
                };
            }
            Some(other) => skipped("capabilities", other, OnWrongType::Ignore),
        }

        self.status_capabilities |= self.status_keys.apply(response);
    }

    /// Whether the phone takes device-status requests: the status capabilities have
    /// `DeviceStatusRequestSupport`, or the phone is Android.
    pub fn device_status_request_supported(&self) -> bool {
        self.status_capabilities & status_capability::DEVICE_STATUS_REQUEST_SUPPORT != 0
            || self.platform == platform::ANDROID
    }

    /// The phone's OS version: one value with two writers — the device list's `osVersion` (every
    /// application; an entry without one leaves none) and the connect reply's
    /// `attributes.osVersion` (when it is a `String`) — the last writer winning. Read it here, not
    /// from either field.
    pub fn current_os_version(&self) -> Option<&str> {
        self.attributes_os_version.as_deref().or(self.os_version.as_deref())
    }

    /// An Android phone's major OS version: the text of [`PhoneRecord::current_os_version`] before
    /// the first `.`, parsed as an integer, for the device-status confirmation timers and photos.
    /// `None` for another platform, without a version, or for one that does not parse.
    pub fn android_major_version(&self) -> Option<i32> {
        if self.platform != platform::ANDROID {
            return None;
        }
        self.current_os_version()?.split('.').next()?.trim().parse::<i32>().ok()
    }
}

#[cfg(test)]
mod tests;
