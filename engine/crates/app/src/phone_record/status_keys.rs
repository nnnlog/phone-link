//! The Phone Link device-status keys the record keeps, and how each is read from a reply.

use super::status_capability;
use phonelink_core::codec::valueset::{Value, ValueSet};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// How many times a confirmation-relevant value of [`StatusKeys`] changed since the record was
/// loaded.
///
/// Not persisted: the DND and ringer controls compare it to stop their confirmation timers
/// on a **change**, which a reader of two snapshots could otherwise miss.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusChanges {
    pub do_not_disturb_status: u64,
    pub do_not_disturb_rule_enabled: u64,
    pub ringer_mode: u64,
}

/// The device-status feature's **Phone Link model** of one phone.
///
/// Each value as the phone last delivered it over Phone Link's contract, `None` while it has not —
/// the feature's default then applies (the `*_DEFAULT` constants; `None` is *unknown* for the
/// `EnabledState` keys) and the precedence rule falls back to the CDEH model.
///
/// The `Some` fields are the persisted set of keys
/// the phone has delivered.
///
/// Every writer goes through the phone record, so they apply in arrival order: inbound dispatch on
/// every phone-content payload ([`StatusKeys::record`]), `deviceStatus` items and the `connect`
/// reply ([`PhoneRecord::apply_device_status_item`],
/// [`PhoneRecord::apply_device_status_connect_reply`]), `phoneCommand` type 0
/// (`sync_over_metered_connection`), and the reset after a failed App request
/// ([`StatusKeys::reset_wifi_state`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusKeys {
    pub battery_percent: Option<i32>,
    pub battery_charging: Option<bool>,
    /// Read as `batteryPowersave` from items and the `connect` reply, as `batteryPowerSave` from
    /// every payload — two spellings of the same value.
    pub battery_power_save: Option<bool>,
    pub bluetooth_state: Option<i32>,
    /// `bluetoothName` (items and the `connect` reply only; a non-empty `String`).
    pub bluetooth_name: Option<String>,
    pub do_not_disturb_status: Option<i32>,
    /// Read whatever the phone sends; only the Android 15 DND flow uses it.
    pub do_not_disturb_rule_enabled: Option<bool>,
    pub network_operator: Option<String>,
    pub network_signal_strength: Option<i32>,
    pub network_signal_strength_max: Option<i32>,
    pub network_indicator_api_support: Option<bool>,
    pub mobile_data_status: Option<i32>,
    pub sync_over_metered_connection: Option<bool>,
    pub ringer_mode: Option<i32>,
    pub wifi_signal_level: Option<i32>,
    pub wifi_state: Option<bool>,
    pub airplane_mode: Option<bool>,
    #[serde(skip)]
    pub changes: StatusChanges,
}

/// How an item or `connect`-reply key of the wrong `DataValueType` is handled: `Warn` logs a warning
/// and skips it (Phone Link on Windows applies none of the keys), `Ignore` skips it quietly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OnWrongType {
    Warn,
    Ignore,
}

impl StatusKeys {
    pub const BATTERY_PERCENT_DEFAULT: i32 = -1;
    pub const BLUETOOTH_STATE_DEFAULT: i32 = -1;
    pub const DO_NOT_DISTURB_STATUS_DEFAULT: i32 = -1;
    pub const DO_NOT_DISTURB_RULE_ENABLED_DEFAULT: bool = true;
    pub const NETWORK_SIGNAL_STRENGTH_DEFAULT: i32 = -2;
    pub const NETWORK_SIGNAL_STRENGTH_MAX_DEFAULT: i32 = 5;
    pub const MOBILE_DATA_STATUS_DEFAULT: i32 = -1;
    pub const RINGER_MODE_DEFAULT: i32 = -1;
    pub const WIFI_SIGNAL_LEVEL_DEFAULT: i32 = -1;

    /// Every inbound phone-content payload, whatever its `contentType`: the keys present with their
    /// stated types (`batteryPowerSave` spelling, `networkOperator` only as a `String`); a key of
    /// another type is ignored; no capability bit is added.
    pub fn record(&mut self, values: &ValueSet) {
        rec_i32(&mut self.battery_percent, values, "batteryPercent");
        rec_bool(&mut self.battery_charging, values, "batteryCharging");
        rec_bool(&mut self.battery_power_save, values, "batteryPowerSave");
        rec_i32(&mut self.bluetooth_state, values, "bluetoothState");

        if let Some(v) = rec_value(values, "doNotDisturbStatus", as_i32) {
            self.set_do_not_disturb_status(v);
        }
        if let Some(v) = rec_value(values, "doNotDisturbRuleEnabled", as_bool) {
            self.set_do_not_disturb_rule_enabled(v);
        }

        rec_string(&mut self.network_operator, values, "networkOperator");
        rec_i32(&mut self.network_signal_strength, values, "networkSignalStrength");
        rec_i32(&mut self.network_signal_strength_max, values, "networkSignalStrengthMax");
        rec_bool(&mut self.network_indicator_api_support, values, "networkIndicatorApiSupport");
        rec_i32(&mut self.mobile_data_status, values, "mobileDataStatus");
        rec_bool(&mut self.sync_over_metered_connection, values, "syncOverMeteredConnection");

        if let Some(v) = rec_value(values, "ringerMode", as_i32) {
            self.set_ringer_mode(v);
        }

        rec_i32(&mut self.wifi_signal_level, values, "wifiSignalLevel");
        rec_bool(&mut self.wifi_state, values, "wifiState");
        rec_bool(&mut self.airplane_mode, values, "airplaneMode");
    }

    /// The status keys of a `deviceStatus` item or of a `connect` reply's top level:
    /// `batteryPowersave` spelling, `bluetoothName`, a `null` (`Empty`) `networkOperator` stored as
    /// `""`. A key of another `DataValueType` is
    /// logged and skipped, the rest applied. Returns the capability bits the applied keys add.
    pub fn apply(&mut self, values: &ValueSet) -> u32 {
        use status_capability as sc;
        let mut bits = 0;

        // Battery: pattern-matched; any applied key adds BatteryIndicator.
        let mut battery = false;
        if let Some(v) = item_value(values, "batteryPercent", OnWrongType::Ignore, as_i32) {
            self.battery_percent = Some(v);
            battery = true;
        }
        if let Some(v) = item_value(values, "batteryCharging", OnWrongType::Ignore, as_bool) {
            self.battery_charging = Some(v);
            battery = true;
        }
        if let Some(v) = item_value(values, "batteryPowersave", OnWrongType::Ignore, as_bool) {
            self.battery_power_save = Some(v);
            battery = true;
        }
        if battery {
            bits |= sc::BATTERY_INDICATOR;
        }

        if let Some(v) = item_value(values, "bluetoothState", OnWrongType::Warn, as_i32) {
            self.bluetooth_state = Some(v);
            bits |= sc::BLUETOOTH_INDICATOR;
        }
        if let Some(name) = item_value(values, "bluetoothName", OnWrongType::Ignore, as_string) {
            if !name.is_empty() {
                self.bluetooth_name = Some(name);
            }
        }

        if let Some(v) = item_value(values, "doNotDisturbStatus", OnWrongType::Warn, as_i32) {
            self.set_do_not_disturb_status(v);
            bits |= sc::DO_NOT_DISTURB_INDICATOR;
        }
        if let Some(v) = item_value(values, "doNotDisturbRuleEnabled", OnWrongType::Warn, as_bool) {
            self.set_do_not_disturb_rule_enabled(v);
        }

        // A `null` value is stored as `""`.
        match values.get("networkOperator") {
            None => {}
            Some(Value::String(s)) => self.network_operator = Some(s.clone()),
            Some(Value::Empty) => self.network_operator = Some(String::new()),
            Some(other) => skipped("networkOperator", other, OnWrongType::Warn),
        }
        if let Some(v) = item_value(values, "networkSignalStrength", OnWrongType::Warn, as_i32) {
            self.network_signal_strength = Some(v);
            bits |= sc::NETWORK_SIGNAL_INDICATOR;
        }
        if let Some(v) = item_value(values, "networkSignalStrengthMax", OnWrongType::Warn, as_i32) {
            self.network_signal_strength_max = Some(v);
        }
        if let Some(v) = item_value(values, "networkIndicatorApiSupport", OnWrongType::Ignore, as_bool) {
            self.network_indicator_api_support = Some(v);
        }
        if let Some(v) = item_value(values, "mobileDataStatus", OnWrongType::Warn, as_i32) {
            self.mobile_data_status = Some(v);
        }
        if let Some(v) = item_value(values, "syncOverMeteredConnection", OnWrongType::Ignore, as_bool) {
            self.sync_over_metered_connection = Some(v);
        }

        if let Some(v) = item_value(values, "ringerMode", OnWrongType::Warn, as_i32) {
            self.set_ringer_mode(v);
            bits |= sc::RINGER_INDICATOR;
        }

        if let Some(v) = item_value(values, "wifiSignalLevel", OnWrongType::Warn, as_i32) {
            self.wifi_signal_level = Some(v);
            bits |= sc::WIFI_INDICATOR;
        }
        if let Some(v) = item_value(values, "wifiState", OnWrongType::Ignore, as_bool) {
            self.wifi_state = Some(v);
        }
        if let Some(v) = item_value(values, "airplaneMode", OnWrongType::Ignore, as_bool) {
            self.airplane_mode = Some(v);
            bits |= sc::AIRPLANE_MODE;
        }

        // `ltwTermsUpdateAvailable` is read only by the terms-update prompt, which is not built.
        bits
    }

    /// The reset after an App request whose response is missing or did not arrive successfully:
    /// the Wi-Fi state becomes unknown, which releases it to the CDEH model.
    pub fn reset_wifi_state(&mut self) {
        self.wifi_state = None;
    }

    pub(super) fn set_do_not_disturb_status(&mut self, v: i32) {
        if self.do_not_disturb_status != Some(v) {
            self.do_not_disturb_status = Some(v);
            self.changes.do_not_disturb_status += 1;
        }
    }

    pub(super) fn set_do_not_disturb_rule_enabled(&mut self, v: bool) {
        if self.do_not_disturb_rule_enabled != Some(v) {
            self.do_not_disturb_rule_enabled = Some(v);
            self.changes.do_not_disturb_rule_enabled += 1;
        }
    }

    pub(super) fn set_ringer_mode(&mut self, v: i32) {
        if self.ringer_mode != Some(v) {
            self.ringer_mode = Some(v);
            self.changes.ringer_mode += 1;
        }
    }
}

pub(super) fn as_i32(v: &Value) -> Option<i32> {
    match v {
        Value::Int32(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(v) => Some(*v),
        _ => None,
    }
}

pub(super) fn as_string(v: &Value) -> Option<String> {
    match v {
        Value::String(v) => Some(v.clone()),
        _ => None,
    }
}

pub(super) fn mistyped(key: &str, v: &Value) {
    debug!(wire_key = key, value_type = v.type_name(), "device-status key of another type ignored");
}

/// A status key of an inbound phone-content payload read with its stated type; another type is ignored.
pub(super) fn rec_value<T>(values: &ValueSet, key: &str, read: fn(&Value) -> Option<T>) -> Option<T> {
    let v = values.get(key)?;
    let typed = read(v);
    if typed.is_none() {
        mistyped(key, v);
    }
    typed
}

pub(super) fn rec_i32(slot: &mut Option<i32>, values: &ValueSet, key: &str) {
    if let Some(v) = rec_value(values, key, as_i32) {
        *slot = Some(v);
    }
}

pub(super) fn rec_bool(slot: &mut Option<bool>, values: &ValueSet, key: &str) {
    if let Some(v) = rec_value(values, key, as_bool) {
        *slot = Some(v);
    }
}

pub(super) fn rec_string(slot: &mut Option<String>, values: &ValueSet, key: &str) {
    if let Some(v) = rec_value(values, key, as_string) {
        *slot = Some(v);
    }
}

/// An item or `connect`-reply key that is not of its stated type: skipped, the rest applied
/// (differs from Phone Link on Windows).
pub(super) fn skipped(key: &str, v: &Value, on_wrong_type: OnWrongType) {
    match on_wrong_type {
        OnWrongType::Warn => {
            warn!(wire_key = key, value_type = v.type_name(), "device-status key of another type skipped")
        }
        OnWrongType::Ignore => {
            debug!(wire_key = key, value_type = v.type_name(), "device-status key of another type ignored")
        }
    }
}

pub(super) fn item_value<T>(
    values: &ValueSet,
    key: &str,
    on_wrong_type: OnWrongType,
    read: fn(&Value) -> Option<T>,
) -> Option<T> {
    let v = values.get(key)?;
    let typed = read(v);
    if typed.is_none() {
        skipped(key, v, on_wrong_type);
    }
    typed
}
