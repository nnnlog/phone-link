//! The two device-status models and what the UI shows.
//!
//! It covers the CDEH decoding of `DeviceStatusResponseMessage` into `DeviceStatus`, and the status
//! model — the precedence rule and the presentation table, including what is shown when data is
//! missing.
//!
//! The Phone Link model is the phone record's [`StatusKeys`]; the CDEH model is a [`CdehStatus`]
//! per phone, held in memory by the feature. Everything here is pure: the feature hands in both
//! models, the connection state and the controls' pending states.

use prost::Message;

use phonelink_app::consts::{permission, platform};
use phonelink_app::phone_record::status_capability as sc;
use phonelink_app::{PhoneRecord, StatusKeys};
use phonelink_core::types::DcgClient;
use phonelink_proto::device_info::v1::{DeviceStatus, DeviceStatusResponseMessage, DeviceStatusResponseStatus};

/// A radio state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadioState {
    On,
    Off,
    Unknown,
}

impl RadioState {
    /// `DeviceStatus` #4 read as CDEH does: `10`, `13` → Off; `11`, `12` → On; else Unknown.
    pub fn from_bluetooth_state(v: i32) -> Self {
        match v {
            10 | 13 => RadioState::Off,
            11 | 12 => RadioState::On,
            _ => RadioState::Unknown,
        }
    }

    /// Phone Link's Bluetooth indicator: `12` on, `10` off, anything else unknown.
    pub fn from_phone_link_bluetooth_state(v: i32) -> Self {
        match v {
            12 => RadioState::On,
            10 => RadioState::Off,
            _ => RadioState::Unknown,
        }
    }

    /// The mobile-data mapping of `DeviceStatus` #9 (both models): `-1` Unknown, `0` Off, `1` On,
    /// `2` On, `3` Off, `4` Off, other Unknown.
    pub fn from_mobile_data_status(v: i32) -> Self {
        match v {
            1 | 2 => RadioState::On,
            0 | 3 | 4 => RadioState::Off,
            _ => RadioState::Unknown,
        }
    }
}

/// The CDEH model of one phone: the `info` of an `OK` `DeviceStatusResponseMessage` decoded as CDEH
/// reads it (fields CDEH does not read are dropped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdehStatus {
    /// The status carried `info`. Without it the values below are CDEH's "info absent" reading,
    /// which supplies no value to the precedence rule ([`select`]):
    /// its `-1` cellular level and `0` DND would otherwise take Phone Link's meanings (no SIM, a
    /// known DND).
    pub has_info: bool,
    /// `-1` when `info` is absent.
    pub battery_percent: i32,
    pub battery_charging: bool,
    pub battery_power_save: bool,
    pub bluetooth: RadioState,
    /// `0` when `info` is absent; DND is on iff `> 1`.
    pub do_not_disturb_status: i32,
    /// The value when present, `true` when absent.
    pub do_not_disturb_rule_enabled: bool,
    /// `-1` when `info` is absent.
    pub cellular_signal_level: i32,
    /// `-1` when `info` is absent.
    pub cellular_signal_level_max: i32,
    /// Mobile data (#9).
    pub cellular: RadioState,
    /// `-1` when `info` is absent.
    pub ringer_mode: i32,
    /// `-1` when `info` is absent.
    pub wifi_signal_level: i32,
    /// `true` → On, otherwise Off.
    pub wifi: RadioState,
    pub airplane_mode: bool,
    pub is_connection_metered: bool,
}

impl CdehStatus {
    /// Reads `info` as CDEH does.
    pub fn read(info: Option<&DeviceStatus>) -> Self {
        match info {
            Some(i) => CdehStatus {
                has_info: true,
                battery_percent: i.battery_percent,
                battery_charging: i.battery_charging,
                battery_power_save: i.battery_power_save,
                bluetooth: RadioState::from_bluetooth_state(i.bluetooth_state),
                do_not_disturb_status: i.do_not_disturb_status,
                do_not_disturb_rule_enabled: i.do_not_disturb_rule_enabled.unwrap_or(true),
                cellular_signal_level: i.network_signal_strength,
                cellular_signal_level_max: i.network_signal_strength_max,
                cellular: RadioState::from_mobile_data_status(i.mobile_data_status),
                ringer_mode: i.ringer_mode,
                wifi_signal_level: i.wifi_signal_level,
                wifi: if i.wifi_state { RadioState::On } else { RadioState::Off },
                airplane_mode: i.airplane_mode,
                is_connection_metered: i.is_connection_metered,
            },
            None => CdehStatus {
                has_info: false,
                battery_percent: -1,
                battery_charging: false,
                battery_power_save: false,
                bluetooth: RadioState::Unknown,
                do_not_disturb_status: 0,
                do_not_disturb_rule_enabled: true,
                cellular_signal_level: -1,
                cellular_signal_level_max: -1,
                cellular: RadioState::Unknown,
                ringer_mode: -1,
                wifi_signal_level: -1,
                wifi: RadioState::Off,
                airplane_mode: false,
                is_connection_metered: false,
            },
        }
    }

    pub fn do_not_disturb_on(&self) -> bool {
        self.do_not_disturb_status > 1
    }
}

/// A `DeviceStatusResponseMessage` received as a pull reply or a push.
#[derive(Debug, Clone, PartialEq)]
pub enum Received {
    /// `OK`: the phone's status replaces the CDEH model.
    Ok(CdehStatus),
    /// Any other status (a message without content counts as `UNSPECIFIED`): ignored, status kept.
    NotOk(i32),
}

/// Decodes a pull reply or push body; a malformed one is an error (logged, status kept).
pub fn decode_received(bytes: &[u8]) -> Result<Received, prost::DecodeError> {
    let message = DeviceStatusResponseMessage::decode(bytes)?;
    Ok(if message.status == DeviceStatusResponseStatus::Ok as i32 {
        Received::Ok(CdehStatus::read(message.info.as_ref()))
    } else {
        Received::NotOk(message.status)
    })
}

/// Each quantity the UI shows, chosen by the precedence rule.
///
/// The Phone Link model's value while the phone has delivered one over Phone Link's contract, else
/// the CDEH model's once an `OK` CDEH status has arrived, else unknown (`None`, or the Phone Link
/// default where one stands for unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    pub battery_percent: Option<i32>,
    pub battery_charging: bool,
    pub battery_power_save: bool,
    pub bluetooth: RadioState,
    pub do_not_disturb_status: Option<i32>,
    pub do_not_disturb_rule_enabled: bool,
    pub cellular_signal_level: Option<i32>,
    /// The maximum of the model the level comes from: the bars and their scale are one quantity
    /// (Phone Link's default `5` when it delivered a level without a maximum).
    pub cellular_signal_level_max: i32,
    /// Phone Link only (CDEH does not read `network_operator`); a non-empty name.
    pub network_operator: Option<String>,
    /// Phone Link only.
    pub network_indicator_api_support: bool,
    pub mobile_data: RadioState,
    pub ringer_mode: Option<i32>,
    pub wifi_signal_level: Option<i32>,
    pub airplane_mode: bool,
    /// CDEH only (Phone Link's contract never carries it).
    pub is_connection_metered: bool,
}

/// The precedence rule over the Phone Link model `pl` and the CDEH model `cdeh` (`None` until an
/// `OK` CDEH status arrived).
///
/// An `OK` status without `info` supplies nothing: every quantity is then
/// Phone Link's, else unknown, as before any CDEH status.
pub fn select(pl: &StatusKeys, cdeh: Option<&CdehStatus>) -> Selected {
    fn pick<T>(pl: Option<T>, cdeh: Option<&CdehStatus>, f: impl Fn(&CdehStatus) -> T) -> Option<T> {
        pl.or_else(|| cdeh.map(f))
    }

    let cdeh = cdeh.filter(|c| c.has_info);
    let pl_max = pl.network_signal_strength_max.unwrap_or(StatusKeys::NETWORK_SIGNAL_STRENGTH_MAX_DEFAULT);
    let (cellular_signal_level, cellular_signal_level_max) = match (pl.network_signal_strength, cdeh) {
        (Some(level), _) => (Some(level), pl_max),
        (None, Some(c)) => (Some(c.cellular_signal_level), c.cellular_signal_level_max),
        (None, None) => (None, pl_max),
    };

    Selected {
        battery_percent: pick(pl.battery_percent, cdeh, |c| c.battery_percent),
        battery_charging: pick(pl.battery_charging, cdeh, |c| c.battery_charging).unwrap_or(false),
        battery_power_save: pick(pl.battery_power_save, cdeh, |c| c.battery_power_save).unwrap_or(false),
        bluetooth: pick(pl.bluetooth_state.map(RadioState::from_phone_link_bluetooth_state), cdeh, |c| c.bluetooth)
            .unwrap_or(RadioState::Unknown),
        do_not_disturb_status: pick(pl.do_not_disturb_status, cdeh, |c| c.do_not_disturb_status),
        do_not_disturb_rule_enabled: pick(pl.do_not_disturb_rule_enabled, cdeh, |c| c.do_not_disturb_rule_enabled)
            .unwrap_or(StatusKeys::DO_NOT_DISTURB_RULE_ENABLED_DEFAULT),
        cellular_signal_level,
        cellular_signal_level_max,
        network_operator: pl.network_operator.clone().filter(|s| !s.is_empty()),
        network_indicator_api_support: pl.network_indicator_api_support == Some(true),
        mobile_data: pick(pl.mobile_data_status.map(RadioState::from_mobile_data_status), cdeh, |c| c.cellular)
            .unwrap_or(RadioState::Unknown),
        ringer_mode: pick(pl.ringer_mode, cdeh, |c| c.ringer_mode),
        wifi_signal_level: pick(pl.wifi_signal_level, cdeh, |c| c.wifi_signal_level),
        airplane_mode: pick(pl.airplane_mode, cdeh, |c| c.airplane_mode).unwrap_or(false),
        is_connection_metered: cdeh.is_some_and(|c| c.is_connection_metered),
    }
}

/// The battery item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryItem {
    Unknown,
    Level { percent: i32, charging: bool, power_save: bool },
}

/// The cellular item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellularItem {
    Unknown,
    /// Shown first when the phone's status capabilities have `AirplaneMode` (`0x400`).
    AirplaneMode,
    /// Level `-1`.
    NoSim,
    Bars {
        level: i32,
        /// `4` or `5`.
        max: i32,
        operator: Option<String>,
        /// Phone Link's four-bar glyph set: `networkIndicatorApiSupport` and a maximum of `4`.
        four_bar_glyphs: bool,
    },
}

/// The Wi-Fi item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiItem {
    Unknown,
    NotConnected,
    Bars(i32),
}

/// What the DND control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndShown {
    Unknown,
    /// A known `doNotDisturbStatus` (`0`–`4`); DND is on iff it is `> 1`.
    Status {
        status: i32,
        on: bool,
    },
    TurningOn,
    TurningOff,
}

/// What the ringer control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingerShown {
    Unknown,
    /// A known ringer mode: `0` silent, `1` vibrate, `2` normal (another value as delivered).
    Mode(i32),
    TurningOn,
    TurningOff,
    TurningVibrate,
}

/// A pending change of the DND control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndPending {
    TurningOn,
    TurningOff,
}

/// A pending change of the ringer control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingerPending {
    /// Turning the ringer on.
    On,
    /// Turning it off.
    Off,
    /// Turning it to vibrate.
    Vibrate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DndControl {
    pub shown: DndShown,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingerControl {
    pub shown: RingerShown,
    pub enabled: bool,
}

/// One phone's status as the menu-bar item shows it. Battery, cellular, Wi-Fi and Bluetooth read
/// unknown (not shown) also while the phone's status capabilities lack their indicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusView {
    pub phone: DcgClient,
    /// The phone is connected and its features are not all disabled; otherwise every item is
    /// unknown and the controls are disabled.
    pub live: bool,
    pub battery: BatteryItem,
    pub cellular: CellularItem,
    pub wifi: WifiItem,
    pub bluetooth: RadioState,
    /// Mobile data on (`true`) or off; `None` (hidden) while unknown.
    pub mobile_data: Option<bool>,
    /// The connection is metered (the item is hidden while `false`).
    pub metered: bool,
    /// `None`: the control does not exist for this phone.
    pub do_not_disturb: Option<DndControl>,
    /// `None`: the control does not exist (or the ringer is not supported, `-5`).
    pub ringer: Option<RingerControl>,
    /// `None`: the phone has no ring control (it is not the selected phone, cannot ring, or ring my
    /// phone is off).
    pub ring_my_phone: Option<super::RingControl>,
}

/// What a control shows besides the selected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay<P> {
    None,
    /// A command was sent and is not confirmed yet.
    Pending(P),
    /// A confirmation timer reverted the control to this value (shown until the next change).
    Reverted(i32),
}

/// Everything a view is built from.
#[derive(Debug, Clone)]
pub struct ViewInputs<'a> {
    pub phone: &'a DcgClient,
    /// The phone record (Phone Link model, status capabilities, permissions); `None` when the phone
    /// has none.
    pub record: Option<&'a PhoneRecord>,
    pub cdeh: Option<&'a CdehStatus>,
    pub connected: bool,
    pub dnd: Overlay<DndPending>,
    pub ringer: Overlay<RingerPending>,
}

/// The phone's post-notification permission gates the controls on Android 13 and later; an
/// unreported permission counts as granted.
pub fn post_notification_allows_controls(record: &PhoneRecord) -> bool {
    !record.post_notification_ask_supported() || record.permission_or(permission::POST_NOTIFICATIONS, true)
}

/// The phone's notifications-sync permission (an unreported one counts as missing).
pub fn notifications_sync_granted(record: &PhoneRecord) -> bool {
    record.permission_or(permission::NOTIFICATIONS_SYNC, false)
}

/// The action strip exists (`DeviceActionIndicator`, Android or iOS).
fn action_strip(record: &PhoneRecord) -> bool {
    (record.platform == platform::ANDROID || record.platform == platform::IOS)
        && record.status_capabilities & sc::DEVICE_ACTION_INDICATOR != 0
}

/// The status strip exists: `DeviceStatusIndicator` on Android; on iOS that bit or the battery
/// indicator's; no strip on any other platform.
fn status_strip(record: &PhoneRecord) -> bool {
    let caps = record.status_capabilities;
    if record.platform == platform::ANDROID {
        caps & sc::DEVICE_STATUS_INDICATOR != 0
    } else if record.platform == platform::IOS {
        caps & (sc::DEVICE_STATUS_INDICATOR | sc::BATTERY_INDICATOR) != 0
    } else {
        false
    }
}

/// The DND value the control shows: its overlay, else the selected status.
pub fn dnd_shown(selected: Option<i32>, overlay: Overlay<DndPending>) -> i32 {
    match overlay {
        Overlay::Pending(DndPending::TurningOn) => -3,
        Overlay::Pending(DndPending::TurningOff) => -2,
        Overlay::Reverted(v) => v,
        Overlay::None => selected.unwrap_or(StatusKeys::DO_NOT_DISTURB_STATUS_DEFAULT),
    }
}

/// The ringer value the control shows: its overlay, else the selected mode.
pub fn ringer_shown(selected: Option<i32>, overlay: Overlay<RingerPending>) -> i32 {
    match overlay {
        Overlay::Pending(RingerPending::Off) => -2,
        Overlay::Pending(RingerPending::On) => -3,
        Overlay::Pending(RingerPending::Vibrate) => -4,
        Overlay::Reverted(v) => v,
        Overlay::None => selected.unwrap_or(StatusKeys::RINGER_MODE_DEFAULT),
    }
}

/// Builds one phone's view: the presentation table, including what is shown when data is missing,
/// and the controls' existence and enabled rules.
pub fn build_view(inputs: &ViewInputs<'_>) -> StatusView {
    let default_keys = StatusKeys::default();
    let keys = inputs.record.map_or(&default_keys, |r| &r.status_keys);
    let sel = select(keys, inputs.cdeh);
    let live = inputs.connected && inputs.record.is_none_or(|r| !r.all_features_disabled);
    let capabilities = inputs.record.map_or(0, |r| r.status_capabilities);

    // The four status indicators exist only with the status strip, each with its own bit or all of
    // `0x1FFF`; one that does not exist is not shown, like an unknown item.
    let strip = inputs.record.is_some_and(status_strip);
    let indicator = |bit: u32| strip && sc::supports(capabilities, bit);

    let battery = match sel.battery_percent {
        Some(percent) if live && indicator(sc::BATTERY_INDICATOR) && percent >= 0 => {
            BatteryItem::Level { percent, charging: sel.battery_charging, power_save: sel.battery_power_save }
        }
        _ => BatteryItem::Unknown,
    };
    let cellular = cellular_item(&sel, live && indicator(sc::NETWORK_SIGNAL_INDICATOR), capabilities);
    let wifi_shown = live && indicator(sc::WIFI_INDICATOR);
    let wifi = match sel.wifi_signal_level {
        Some(0) if wifi_shown => WifiItem::NotConnected,
        Some(level @ 1..=4) if wifi_shown => WifiItem::Bars(level),
        _ => WifiItem::Unknown,
    };
    let bluetooth = if live && indicator(sc::BLUETOOTH_INDICATOR) { sel.bluetooth } else { RadioState::Unknown };
    let mobile_data = match sel.mobile_data {
        RadioState::On if live => Some(true),
        RadioState::Off if live => Some(false),
        _ => None,
    };

    let post_notification_ok = inputs.record.is_some_and(post_notification_allows_controls);
    let do_not_disturb = dnd_control(inputs, &sel, live, post_notification_ok);
    let ringer = ringer_control(inputs, &sel, live, post_notification_ok);

    StatusView {
        phone: inputs.phone.clone(),
        live,
        battery,
        cellular,
        wifi,
        bluetooth,
        mobile_data,
        metered: live && sel.is_connection_metered,
        do_not_disturb,
        ringer,
        // The feature handle fills it in: it needs the account and the ring's countdown.
        ring_my_phone: None,
    }
}

/// The cellular item: unknown when not `shown`, else airplane mode, no SIM or the signal bars.
fn cellular_item(sel: &Selected, shown: bool, capabilities: u32) -> CellularItem {
    if !shown {
        CellularItem::Unknown
    } else if capabilities & sc::AIRPLANE_MODE != 0 && sel.airplane_mode {
        CellularItem::AirplaneMode
    } else {
        // A maximum outside Phone Link's value space (`4` or `5`) — e.g. CDEH's `-1` or `0` when the
        // phone sent none — shows as `5`, Phone Link's five-bar glyph set.
        let max = match sel.cellular_signal_level_max {
            m @ (4 | 5) => m,
            _ => StatusKeys::NETWORK_SIGNAL_STRENGTH_MAX_DEFAULT,
        };

        match sel.cellular_signal_level {
            Some(-1) => CellularItem::NoSim,
            Some(level) if level >= 0 => CellularItem::Bars {
                level,
                max,
                operator: sel.network_operator.clone(),
                four_bar_glyphs: sel.network_indicator_api_support && max == 4,
            },
            _ => CellularItem::Unknown,
        }
    }
}

/// The DND control, when the phone's action strip has one.
fn dnd_control(inputs: &ViewInputs<'_>, sel: &Selected, live: bool, post_notification_ok: bool) -> Option<DndControl> {
    inputs.record.filter(|r| action_strip(r) && sc::supports(r.status_capabilities, sc::DO_NOT_DISTURB_INDICATOR)).map(
        |_| {
            let shown = if live { dnd_shown(sel.do_not_disturb_status, inputs.dnd) } else { -1 };
            let shown = match shown {
                -3 => DndShown::TurningOn,
                -2 => DndShown::TurningOff,
                -1 => DndShown::Unknown,
                status => DndShown::Status { status, on: status > 1 },
            };
            DndControl { shown, enabled: live && matches!(shown, DndShown::Status { .. }) && post_notification_ok }
        },
    )
}

/// The ringer control, when the phone's action strip has one and its value is not hidden.
fn ringer_control(
    inputs: &ViewInputs<'_>,
    sel: &Selected,
    live: bool,
    post_notification_ok: bool,
) -> Option<RingerControl> {
    inputs.record.filter(|r| action_strip(r) && sc::supports(r.status_capabilities, sc::RINGER_INDICATOR)).and_then(
        |_| {
            let shown = if live { ringer_shown(sel.ringer_mode, inputs.ringer) } else { -1 };
            let shown = match shown {
                -5 => return None,
                -4 => RingerShown::TurningVibrate,
                -3 => RingerShown::TurningOn,
                -2 => RingerShown::TurningOff,
                -1 => RingerShown::Unknown,
                mode => RingerShown::Mode(mode),
            };
            Some(RingerControl {
                shown,
                enabled: live && matches!(shown, RingerShown::Mode(_)) && post_notification_ok,
            })
        },
    )
}

#[cfg(test)]
mod tests;
