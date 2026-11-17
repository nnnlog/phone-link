//! The phone's status (battery, signal, the quick controls) as the window reads it: the raw state the
//! sidebar draws, the status lines of the tray menu, and each control's texts for the sidebar's tiles
//! and the tray menu.

use phonelink_engine::api::device_status::{
    BatteryState, CellularState, DndControlState, DndDisplay, PhoneStatus, RadioStatus, RingControlState, RingDisplay,
    RingerControlState, RingerDisplay, WifiState,
};
use serde_json::{Value, json};

use crate::l10n::{fill, translate};

/// `template` translated, with its one count (`%d`, `%u` or `%lld`) filled in.
fn with_count(template: &str, count: impl std::fmt::Display) -> String {
    fill(translate(template), &[count.to_string()])
}

/// The phone's ringer modes as the engine numbers them; the ringer control moves
/// sound → vibrate → silent → sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingerMode {
    Silent,
    Vibrate,
    Sound,
}

impl RingerMode {
    /// A mode outside 0/1/2 is still one the phone reported: shown as Sound, which the ringer control
    /// switches it to.
    fn of(raw: i32) -> Self {
        match raw {
            0 => Self::Silent,
            1 => Self::Vibrate,
            _ => Self::Sound,
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Sound => Self::Vibrate,
            Self::Vibrate => Self::Silent,
            Self::Silent => Self::Sound,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Vibrate => "vibrate",
            Self::Sound => "sound",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Silent => translate("Silent"),
            Self::Vibrate => translate("Vibrate"),
            Self::Sound => translate("Sound"),
        }
    }
}

/// One line of the tray menu's status: an item the engine knows, with a freedesktop icon name.
fn line(id: &str, text: String, icon: &str) -> Value {
    json!({ "id": id, "text": text, "icon": icon })
}

/// The battery icon nearest to `percent`, in steps of ten.
fn battery_icon(percent: i32, charging: bool) -> String {
    let step = (percent.clamp(0, 100) + 5) / 10 * 10;
    if charging { format!("battery-{step:03}-charging") } else { format!("battery-{step:03}") }
}

/// The status lines of the tray menu, in its order; none while the phone is not live.
fn lines(status: &PhoneStatus) -> Vec<Value> {
    if !status.live {
        return Vec::new();
    }

    let mut lines = Vec::new();
    if let BatteryState::Level { percent, charging, power_save } = status.battery {
        let template = match (charging, power_save) {
            (false, false) => "%d%%",
            (true, false) => "%d%% · Charging",
            (false, true) => "%d%% · Battery Saver",
            (true, true) => "%d%% · Charging · Battery Saver",
        };
        lines.push(line("battery", with_count(template, percent), &battery_icon(percent, charging)));
    }
    match &status.cellular {
        CellularState::Unknown => {}
        CellularState::AirplaneMode => {
            lines.push(line("cellular", translate("Airplane Mode").to_owned(), "network-flightmode-on"));
        }
        CellularState::NoSim => lines.push(line("cellular", translate("No SIM").to_owned(), "network-mobile-off")),
        CellularState::Bars { level, max, operator, .. } => {
            let name = operator
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| translate("Mobile Network").to_owned(), str::to_owned);
            let percent = if *max > 0 { (level.clamp(&0, max) * 100 / max + 10) / 20 * 20 } else { 0 };
            lines.push(line("cellular", name, &format!("network-mobile-{percent}")));
        }
    }
    match status.wifi {
        WifiState::Unknown => {}
        WifiState::NotConnected => {
            lines.push(line("wifi", translate("Wi-Fi Not Connected").to_owned(), "network-wireless-disconnected"));
        }
        WifiState::Bars { level } => {
            let icon = match level {
                ..=1 => "network-wireless-signal-weak",
                2 => "network-wireless-signal-ok",
                3 => "network-wireless-signal-good",
                _ => "network-wireless-signal-excellent",
            };
            lines.push(line("wifi", translate("Wi-Fi").to_owned(), icon));
        }
    }
    match status.bluetooth {
        RadioStatus::On => lines.push(line("bluetooth", translate("Bluetooth On").to_owned(), "bluetooth-active")),
        RadioStatus::Off => lines.push(line("bluetooth", translate("Bluetooth Off").to_owned(), "bluetooth-disabled")),
        RadioStatus::Unknown => {}
    }
    if let Some(on) = status.mobile_data {
        let text = if on { "Mobile Data On" } else { "Mobile Data Off" };
        lines.push(line("mobileData", translate(text).to_owned(), "network-transmit-receive"));
    }
    if status.metered {
        lines.push(line("metered", translate("Metered Connection").to_owned(), "network-limited"));
    }
    lines
}

/// A control as the sidebar's tile shows it: its state in a word or two (`null` when there is none to
/// show), drawn highlighted while on, with progress while a change is under way. While the phone is
/// not live the tile shows its name only and cannot be clicked.
fn tile(state: Option<String>, on: bool, busy: bool, enabled: bool) -> Value {
    json!({ "state": state, "on": on, "busy": busy, "enabled": enabled })
}

fn dnd_json(control: DndControlState, live: bool) -> Value {
    let (shown, title, menu_on, menu_enabled) = match control.shown {
        DndDisplay::Status { on, .. } => {
            (if on { "on" } else { "off" }, translate("Do Not Disturb"), Some(on), control.enabled && live)
        }
        DndDisplay::Unknown => ("unknown", translate("Do Not Disturb"), Some(false), false),
        DndDisplay::TurningOn => ("turningOn", translate("Turning On Do Not Disturb..."), None, false),
        DndDisplay::TurningOff => ("turningOff", translate("Turning Off Do Not Disturb..."), None, false),
    };

    let tile = match control.shown {
        _ if !live => tile(None, false, false, false),
        DndDisplay::Status { on, .. } => {
            tile(Some(translate(if on { "On" } else { "Off" }).to_owned()), on, false, control.enabled)
        }
        DndDisplay::Unknown => tile(None, false, false, false),
        DndDisplay::TurningOn => tile(Some(translate("Turning On...").to_owned()), false, true, false),
        DndDisplay::TurningOff => tile(Some(translate("Turning Off...").to_owned()), false, true, false),
    };

    json!({
        "shown": shown,
        "enabled": control.enabled,
        "menuTitle": title,
        "menuOn": menu_on,
        "menuEnabled": menu_enabled,
        "tile": tile,
    })
}

fn ringer_json(control: RingerControlState, live: bool) -> Value {
    let pending = |mode: RingerMode| {
        (
            "changing",
            mode,
            fill(translate("Ringer: Switching to %@..."), &[mode.name().to_owned()]),
            false,
            tile(Some(translate("Switching...").to_owned()), false, true, false),
        )
    };
    let (shown, mode, title, menu_enabled, tile_json) = match control.shown {
        RingerDisplay::Mode { mode } => {
            let mode = RingerMode::of(mode);
            let title = fill(translate("Ringer: %@"), &[mode.name().to_owned()]);
            let tile_json = tile(Some(mode.name().to_owned()), false, false, control.enabled);
            (mode.key(), mode, title, control.enabled && live, tile_json)
        }
        RingerDisplay::Unknown => {
            ("unknown", RingerMode::Sound, translate("Ringer").to_owned(), false, tile(None, false, false, false))
        }
        RingerDisplay::TurningVibrate => pending(RingerMode::Vibrate),
        RingerDisplay::TurningOff => pending(RingerMode::Silent),
        RingerDisplay::TurningOn => pending(RingerMode::Sound),
    };

    let next = menu_enabled.then(|| fill(translate("Click to switch to %@"), &[mode.next().name().to_owned()]));
    json!({
        "shown": shown,
        "enabled": control.enabled,
        "mode": mode.key(),
        "menuTitle": title,
        "menuHint": next,
        "menuEnabled": menu_enabled,
        "tile": if live { tile_json } else { tile(None, false, false, false) },
    })
}

fn ring_json(control: RingControlState, live: bool) -> Value {
    let (ringing, remaining) = match control.shown {
        RingDisplay::Ringing { remaining_seconds, .. } => (true, Some(remaining_seconds)),
        RingDisplay::Off | RingDisplay::Unknown => (false, None),
    };
    let (title, menu_enabled, stop) = match (control.shown, remaining) {
        (RingDisplay::Ringing { .. }, Some(seconds)) => {
            let stop = (control.can_stop && control.enabled && live).then(|| translate("Stop Ringing").to_owned());
            (with_count("Ringing... %u s", seconds), false, stop)
        }
        (RingDisplay::Off, _) => (translate("Ring Phone").to_owned(), control.enabled && live, None),
        _ => (translate("Ring Phone").to_owned(), false, None),
    };

    let tile_json = match (control.shown, remaining) {
        _ if !live => tile(None, false, false, false),
        (RingDisplay::Ringing { .. }, Some(seconds)) => {
            tile(Some(with_count("Ringing · %u s", seconds)), true, false, control.can_stop && control.enabled)
        }
        (RingDisplay::Off, _) => tile(None, false, false, control.enabled),
        _ => tile(None, false, false, false),
    };

    json!({
        "ringing": ringing,
        "enabled": control.enabled,
        "canStop": control.can_stop,
        "remainingSeconds": remaining,
        "menuTitle": title,
        "menuEnabled": menu_enabled,
        "stopTitle": stop,
        "tile": tile_json,
    })
}

/// The phone's status as the window reads it, as JSON text.
pub fn status_json(status: &PhoneStatus) -> String {
    status_value(status).to_string()
}

fn status_value(status: &PhoneStatus) -> Value {
    let (battery, charging) = match status.battery {
        BatteryState::Level { percent, charging, .. } => (Some(percent), charging),
        BatteryState::Unknown => (None, false),
    };
    let wifi_bars = match status.wifi {
        WifiState::Bars { level } => Some(level),
        WifiState::NotConnected | WifiState::Unknown => None,
    };
    let (cell_bars, cell_max) = match &status.cellular {
        CellularState::Bars { level, max, .. } => (Some(*level), Some(*max)),
        _ => (None, None),
    };

    json!({
        "live": status.live,
        "battery": battery,
        "charging": charging,
        "wifiBars": wifi_bars,
        "cellBars": cell_bars,
        "cellMax": cell_max,
        "lines": lines(status),
        "dnd": status.do_not_disturb.map(|control| dnd_json(control, status.live)),
        "ringer": status.ringer.map(|control| ringer_json(control, status.live)),
        "ring": status.ring_my_phone.map(|control| ring_json(control, status.live)),
    })
}

#[cfg(test)]
mod tests;
