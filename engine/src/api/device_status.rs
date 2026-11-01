//! Device status for the UI ([`crate::features::device_status`]).
//!
//! It covers the menu-bar status of the selected phone, its Do Not Disturb, ringer and
//! ring-my-phone controls with their prompts, and the low-battery notification setting.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::device_status::{
    BatteryItem, CellularItem, ControlOutcome, DeviceStatus, DeviceStatusEvent, DndShown, Prompt, RadioState,
    RingShown, RingerShown, StatusView, WifiItem,
};

use super::{FeatureApiError, core_of, features_of, selected_phone};

/// The phone's battery: its charge level and charging state, or unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BatteryState {
    Unknown,
    Level { percent: i32, charging: bool, power_save: bool },
}

/// The phone's cellular signal: bars and operator, airplane mode, no SIM, or unknown.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CellularState {
    Unknown,
    AirplaneMode,
    NoSim,
    /// `level` of `max` (4 or 5) bars.
    Bars {
        level: i32,
        max: i32,
        operator: Option<String>,
        four_bar_glyphs: bool,
    },
}

/// The phone's Wi-Fi signal: bars, not connected, or unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WifiState {
    Unknown,
    NotConnected,
    /// 1–4 bars.
    Bars {
        level: i32,
    },
}

/// Whether a radio of the phone (Bluetooth) is on, off, or unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RadioStatus {
    On,
    Off,
    Unknown,
}

/// What the Do Not Disturb control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DndDisplay {
    Unknown,
    Status { status: i32, on: bool },
    TurningOn,
    TurningOff,
}

/// What the ringer control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RingerDisplay {
    Unknown,
    /// `0` silent, `1` vibrate, `2` sound.
    Mode {
        mode: i32,
    },
    TurningOn,
    TurningOff,
    TurningVibrate,
}

/// What the ring-my-phone control shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RingDisplay {
    /// The phone is not connected or turned every feature off: the control is disabled.
    Unknown,
    /// Not ringing: the control offers ringing the phone.
    Off,
    /// Ringing for `remaining_seconds` more. `confirm_stop`: the user asked to stop once; the next
    /// stop sends it without asking again.
    Ringing { remaining_seconds: u32, confirm_stop: bool },
}

/// The ring-my-phone control: what it shows and whether the user can activate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RingControlState {
    pub shown: RingDisplay,
    pub enabled: bool,
    /// A ringing control offers stopping it.
    pub can_stop: bool,
}

/// The Do Not Disturb control: what it shows and whether the user can activate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct DndControlState {
    pub shown: DndDisplay,
    pub enabled: bool,
}

/// The ringer control: what it shows and whether the user can activate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RingerControlState {
    pub shown: RingerDisplay,
    pub enabled: bool,
}

/// The selected phone's status as the menu bar shows it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneStatus {
    /// The phone is connected and its features are not all disabled; otherwise every item is
    /// unknown and the controls are disabled.
    pub live: bool,
    pub battery: BatteryState,
    pub cellular: CellularState,
    pub wifi: WifiState,
    pub bluetooth: RadioStatus,
    /// Mobile data on or off; `None` while unknown (hidden).
    pub mobile_data: Option<bool>,
    /// The connection is metered (hidden while `false`).
    pub metered: bool,
    /// `None`: the phone has no Do Not Disturb control.
    pub do_not_disturb: Option<DndControlState>,
    /// `None`: the phone has no ringer control.
    pub ringer: Option<RingerControlState>,
    /// `None`: the phone has no ring-my-phone control.
    pub ring_my_phone: Option<RingControlState>,
}

impl From<RadioState> for RadioStatus {
    fn from(r: RadioState) -> Self {
        match r {
            RadioState::On => RadioStatus::On,
            RadioState::Off => RadioStatus::Off,
            RadioState::Unknown => RadioStatus::Unknown,
        }
    }
}

impl From<&StatusView> for PhoneStatus {
    fn from(v: &StatusView) -> Self {
        Self {
            live: v.live,
            battery: match v.battery {
                BatteryItem::Unknown => BatteryState::Unknown,
                BatteryItem::Level { percent, charging, power_save } => {
                    BatteryState::Level { percent, charging, power_save }
                }
            },
            cellular: match &v.cellular {
                CellularItem::Unknown => CellularState::Unknown,
                CellularItem::AirplaneMode => CellularState::AirplaneMode,
                CellularItem::NoSim => CellularState::NoSim,
                CellularItem::Bars { level, max, operator, four_bar_glyphs } => CellularState::Bars {
                    level: *level,
                    max: *max,
                    operator: operator.clone(),
                    four_bar_glyphs: *four_bar_glyphs,
                },
            },
            wifi: match v.wifi {
                WifiItem::Unknown => WifiState::Unknown,
                WifiItem::NotConnected => WifiState::NotConnected,
                WifiItem::Bars(level) => WifiState::Bars { level },
            },
            bluetooth: v.bluetooth.into(),
            mobile_data: v.mobile_data,
            metered: v.metered,
            do_not_disturb: v.do_not_disturb.map(|c| DndControlState {
                shown: match c.shown {
                    DndShown::Unknown => DndDisplay::Unknown,
                    DndShown::Status { status, on } => DndDisplay::Status { status, on },
                    DndShown::TurningOn => DndDisplay::TurningOn,
                    DndShown::TurningOff => DndDisplay::TurningOff,
                },
                enabled: c.enabled,
            }),
            ringer: v.ringer.map(|c| RingerControlState {
                shown: match c.shown {
                    RingerShown::Unknown => RingerDisplay::Unknown,
                    RingerShown::Mode(mode) => RingerDisplay::Mode { mode },
                    RingerShown::TurningOn => RingerDisplay::TurningOn,
                    RingerShown::TurningOff => RingerDisplay::TurningOff,
                    RingerShown::TurningVibrate => RingerDisplay::TurningVibrate,
                },
                enabled: c.enabled,
            }),
            ring_my_phone: v.ring_my_phone.map(|c| RingControlState {
                shown: match c.shown {
                    RingShown::Unknown => RingDisplay::Unknown,
                    RingShown::Off => RingDisplay::Off,
                    RingShown::Ringing { remaining_seconds, confirm_stop } => {
                        RingDisplay::Ringing { remaining_seconds, confirm_stop }
                    }
                },
                enabled: c.enabled,
                can_stop: c.can_stop,
            }),
        }
    }
}

/// A prompt a control shows instead of sending; `accept_prompt` runs its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StatusPrompt {
    /// Do Not Disturb needs the phone's notification access; the action requests it.
    DndNotificationsPermission,
    /// The phone's Do Not Disturb rule for Link to Windows is off; the action asks the phone to fix it.
    DndRuleDisabled,
    /// The ringer needs the phone's notification access; the action requests it.
    RingerNotificationsPermission,
    /// Turning Do Not Disturb off was not confirmed; the action asks the phone to fix the rule.
    DndTurnOffFailed,
    /// The user asked to stop the ringing phone; the action stops it.
    StopRinging,
}

impl From<Prompt> for StatusPrompt {
    fn from(p: Prompt) -> Self {
        match p {
            Prompt::DndNotificationsPermission => StatusPrompt::DndNotificationsPermission,
            Prompt::DndRuleDisabled => StatusPrompt::DndRuleDisabled,
            Prompt::RingerNotificationsPermission => StatusPrompt::RingerNotificationsPermission,
            Prompt::DndTurnOffFailed => StatusPrompt::DndTurnOffFailed,
            Prompt::StopRinging => StatusPrompt::StopRinging,
        }
    }
}

impl From<StatusPrompt> for Prompt {
    fn from(p: StatusPrompt) -> Self {
        match p {
            StatusPrompt::DndNotificationsPermission => Prompt::DndNotificationsPermission,
            StatusPrompt::DndRuleDisabled => Prompt::DndRuleDisabled,
            StatusPrompt::RingerNotificationsPermission => Prompt::RingerNotificationsPermission,
            StatusPrompt::DndTurnOffFailed => Prompt::DndTurnOffFailed,
            StatusPrompt::StopRinging => Prompt::StopRinging,
        }
    }
}

/// What activating a control did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StatusControlOutcome {
    /// The command was sent; the control shows its pending state.
    Sent,
    /// A prompt is shown instead.
    Prompt { prompt: StatusPrompt },
    /// The control is unavailable or a change is pending: nothing sent.
    NotAvailable,
}

impl From<ControlOutcome> for StatusControlOutcome {
    fn from(o: ControlOutcome) -> Self {
        match o {
            ControlOutcome::Sent { .. } | ControlOutcome::RingSent(_) => StatusControlOutcome::Sent,
            ControlOutcome::Prompt(p) => StatusControlOutcome::Prompt { prompt: p.into() },
            ControlOutcome::NotAvailable => StatusControlOutcome::NotAvailable,
        }
    }
}

/// A change to device status: re-read [`DeviceStatusApi::status`], or show or withdraw a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DeviceStatusChange {
    /// What `status` shows may have changed.
    Changed,
    /// The engine raises a prompt by itself (a Do Not Disturb turn-off that did not take).
    PromptRequested { prompt: StatusPrompt },
    /// A prompt raised earlier no longer applies.
    PromptWithdrawn { prompt: StatusPrompt },
    /// The user clicked the low-battery notification.
    LowBatteryNotificationActivated,
}

impl From<&DeviceStatusEvent> for DeviceStatusChange {
    fn from(e: &DeviceStatusEvent) -> Self {
        match e {
            DeviceStatusEvent::Changed { .. } => DeviceStatusChange::Changed,
            DeviceStatusEvent::PromptRequested { prompt, .. } => {
                DeviceStatusChange::PromptRequested { prompt: (*prompt).into() }
            }
            DeviceStatusEvent::PromptWithdrawn { prompt, .. } => {
                DeviceStatusChange::PromptWithdrawn { prompt: (*prompt).into() }
            }
            DeviceStatusEvent::LowBatteryNotificationActivated { .. } => {
                DeviceStatusChange::LowBatteryNotificationActivated
            }
        }
    }
}

/// Device status of the selected phone.
#[derive(uniffi::Object)]
pub struct DeviceStatusApi {
    engine: Arc<Engine>,
}

impl DeviceStatusApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(&self, f: impl FnOnce(&DeviceStatus, &crate::types::DcgClient) -> T) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.device_status.clone();
        let phone = selected_phone(&core)?;
        Ok(f(&feature, &phone))
    }
}

#[uniffi::export]
impl DeviceStatusApi {
    /// The selected phone's status and controls as the menu bar shows them.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn status(&self) -> Result<PhoneStatus, FeatureApiError> {
        self.run(|f, phone| (&f.status(phone)).into())
    }

    /// Turns the phone's Do Not Disturb on when it is off, and off when it is on.
    ///
    /// It returns a prompt instead when the phone has not granted notification access or its Do
    /// Not Disturb rule is off. Nothing is sent while the control is unavailable or a change is
    /// pending.
    pub fn toggle_do_not_disturb(&self) -> Result<StatusControlOutcome, FeatureApiError> {
        self.run(|f, phone| f.toggle_do_not_disturb(phone).into())
    }

    /// Moves the ringer to its next mode (sound → vibrate → silent → sound).
    pub fn cycle_ringer_mode(&self) -> Result<StatusControlOutcome, FeatureApiError> {
        self.run(|f, phone| f.cycle_ringer_mode(phone).into())
    }

    /// Rings the phone: the control counts down while the phone rings.
    pub fn ring_my_phone(&self) -> Result<StatusControlOutcome, FeatureApiError> {
        self.run(|f, phone| f.ring_my_phone(phone).into())
    }

    /// Stops the ringing phone; the first stop of a ring returns the `StopRinging` prompt, whose
    /// acceptance stops it.
    pub fn stop_ringing(&self) -> Result<StatusControlOutcome, FeatureApiError> {
        self.run(|f, phone| f.stop_ringing(phone).into())
    }

    /// Runs a prompt's action.
    pub fn accept_prompt(&self, prompt: StatusPrompt) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.accept_prompt(phone, prompt.into()))
    }

    /// Whether this Mac notifies the user when the selected phone's battery runs low (default on).
    pub fn low_battery_notifications_enabled(&self) -> Result<bool, FeatureApiError> {
        Ok(self.run(|f, phone| f.low_battery_notifications_enabled(phone))??)
    }

    /// Turns the low-battery notification for the selected phone on or off; the choice is saved.
    pub fn set_low_battery_notifications_enabled(&self, enabled: bool) -> Result<(), FeatureApiError> {
        Ok(self.run(|f, phone| f.set_low_battery_notifications_enabled(phone, enabled))??)
    }
}
