use std::sync::atomic::{AtomicU64, Ordering};

use super::*;

/// A temporary directory, removed when the test ends.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "phonelink-power-supply-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }

    fn supply(&self, name: &str, attributes: &[(&str, &str)]) -> &Self {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir).unwrap();
        for (attribute, value) in attributes {
            fs::write(dir.join(attribute), format!("{value}\n")).unwrap();
        }
        self
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn battery(current: i32, max: i32, on_ac_power: bool) -> Option<PcBattery> {
    Some(PcBattery { current_capacity: current, max_capacity: max, on_ac_power })
}

#[test]
fn the_system_batteries_energy_is_added_up_and_peripherals_are_left_out() {
    let power = Fixture::new();
    power
        .supply("AC", &[("type", "Mains"), ("online", "0")])
        .supply("BAT0", &[("type", "Battery"), ("energy_now", "30000000"), ("energy_full", "50000000")])
        .supply("BAT1", &[("type", "Battery"), ("energy_now", "10000000"), ("energy_full", "20000000")])
        .supply(
            "hidpp_battery_0",
            &[("type", "Battery"), ("scope", "Device"), ("energy_now", "1"), ("energy_full", "1000")],
        );
    assert_eq!(system_battery(&power.0).unwrap(), battery(40_000_000, 70_000_000, false));
}

#[test]
fn the_charge_is_added_up_where_the_batteries_report_no_energy() {
    let power = Fixture::new();
    power
        .supply("BAT0", &[("type", "Battery"), ("charge_now", "2000000"), ("charge_full", "4000000")])
        .supply("BAT1", &[("type", "Battery"), ("charge_now", "1000000"), ("charge_full", "2000000")]);
    assert_eq!(system_battery(&power.0).unwrap(), battery(3_000_000, 6_000_000, false));
}

#[test]
fn a_lone_battery_gives_its_own_percentage() {
    let power = Fixture::new();
    power.supply(
        "BAT0",
        &[
            ("type", "Battery"),
            ("capacity", "87"),
            ("energy_now", "1"),
            ("energy_full", "2"),
            ("status", "Discharging"),
        ],
    );
    assert_eq!(system_battery(&power.0).unwrap(), battery(87, 100, false));
}

#[test]
fn a_lone_battery_without_a_percentage_gives_its_energy_else_its_charge() {
    let energy = Fixture::new();
    energy.supply("BAT0", &[("type", "Battery"), ("energy_now", "20"), ("energy_full", "80")]);
    assert_eq!(system_battery(&energy.0).unwrap(), battery(20, 80, false));

    let charge = Fixture::new();
    charge.supply("BAT0", &[("type", "Battery"), ("charge_now", "30"), ("charge_full", "40")]);
    assert_eq!(system_battery(&charge.0).unwrap(), battery(30, 40, false));
}

#[test]
fn batteries_whose_charge_does_not_add_up_are_an_error() {
    let mixed = Fixture::new();
    mixed
        .supply("BAT0", &[("type", "Battery"), ("energy_now", "30000000"), ("energy_full", "50000000")])
        .supply("BAT1", &[("type", "Battery"), ("charge_now", "1000000"), ("charge_full", "2000000")]);
    assert!(system_battery(&mixed.0).is_err());

    // Percentages alone weigh nothing, so two of them do not add up.
    let percentages = Fixture::new();
    percentages
        .supply("BAT0", &[("type", "Battery"), ("capacity", "80")])
        .supply("BAT1", &[("type", "Battery"), ("capacity", "20")]);
    assert!(system_battery(&percentages.0).is_err());

    let alone = Fixture::new();
    alone.supply("BAT0", &[("type", "Battery"), ("status", "Discharging")]);
    assert!(system_battery(&alone.0).is_err());
}

#[test]
fn an_empty_battery_bay_is_left_out() {
    let power = Fixture::new();
    power
        .supply("BAT0", &[("type", "Battery"), ("present", "1"), ("energy_now", "5"), ("energy_full", "10")])
        .supply("BAT1", &[("type", "Battery"), ("present", "0")]);
    assert_eq!(system_battery(&power.0).unwrap(), battery(5, 10, false));
}

#[test]
fn a_computer_without_its_own_battery_has_none() {
    let power = Fixture::new();
    power
        .supply("hidpp_battery_0", &[("type", "Battery"), ("scope", "Device"), ("capacity", "40")])
        .supply("AC", &[("type", "Mains"), ("online", "1")]);
    assert_eq!(system_battery(&power.0).unwrap(), None);
    assert_eq!(system_battery(&power.0.join("missing")).unwrap(), None);
}

#[test]
fn an_unknown_battery_status_is_an_error() {
    let power = Fixture::new();
    power.supply("BAT0", &[("type", "Battery"), ("capacity", "50"), ("status", "Sleeping")]);
    assert!(system_battery(&power.0).is_err());
}

/// Whether a computer with one battery in `status` and the external supply `external` (none when
/// empty) runs on AC power.
fn on_ac_power_with(status: &str, external: &[(&str, &str)]) -> bool {
    let power = Fixture::new();
    power.supply("BAT0", &[("type", "Battery"), ("capacity", "64"), ("status", status)]);
    if !external.is_empty() {
        power.supply("ADP1", external);
    }
    system_battery(&power.0).unwrap().expect("a battery").on_ac_power
}

#[test]
fn an_online_supply_or_a_charging_battery_is_ac_power() {
    assert!(on_ac_power_with("Charging", &[]));
    assert!(on_ac_power_with("Charging", &[("type", "Mains"), ("online", "0")]));
    assert!(on_ac_power_with("Discharging", &[("type", "Mains"), ("online", "1")]));
    assert!(on_ac_power_with("Unknown", &[("type", "USB"), ("online", "2")]));
    assert!(!on_ac_power_with("Discharging", &[("type", "Mains"), ("online", "0")]));
    assert!(!on_ac_power_with("Unknown", &[]));
}

#[test]
fn a_full_or_held_battery_is_ac_power_only_without_a_supply_to_ask() {
    assert!(on_ac_power_with("Full", &[]));
    assert!(on_ac_power_with("Not charging", &[]));
    assert!(!on_ac_power_with("Full", &[("type", "Mains"), ("online", "0")]));
    assert!(!on_ac_power_with("Not charging", &[("type", "USB"), ("online", "0")]));
    assert!(on_ac_power_with("Full", &[("type", "Mains"), ("online", "1")]));
}

#[test]
fn a_device_scoped_charger_is_neither_ac_power_nor_a_supply_to_ask() {
    let power = Fixture::new();
    power
        .supply("BAT0", &[("type", "Battery"), ("capacity", "50"), ("status", "Discharging")])
        .supply("dock-usb", &[("type", "USB"), ("scope", "Device"), ("online", "1")]);
    assert_eq!(system_battery(&power.0).unwrap(), battery(50, 100, false));

    let full = Fixture::new();
    full.supply("BAT0", &[("type", "Battery"), ("capacity", "100"), ("status", "Full")])
        .supply("dock-usb", &[("type", "USB"), ("scope", "Device"), ("online", "0")]);
    assert_eq!(system_battery(&full.0).unwrap(), battery(100, 100, true));
}

#[test]
fn the_batteries_status_combines_as_upower_combines_it() {
    use BatteryStatus::*;
    assert_eq!(BatteryStatus::combined(&[Discharging, Charging]), Charging);
    assert_eq!(BatteryStatus::combined(&[Full, Discharging]), Discharging);
    assert_eq!(BatteryStatus::combined(&[Full, Full]), Full);
    assert_eq!(BatteryStatus::combined(&[Full, NotCharging]), NotCharging);
    assert_eq!(BatteryStatus::combined(&[Unknown, Full]), Unknown);
    assert_eq!(BatteryStatus::combined(&[NotCharging, Unknown]), Unknown);
    assert_eq!(BatteryStatus::combined(&[]), Unknown);
}

#[test]
fn any_charging_battery_is_charging_and_a_discharging_one_is_not_ac_power() {
    let charging = Fixture::new();
    charging
        .supply("BAT0", &[("type", "Battery"), ("energy_now", "1"), ("energy_full", "2"), ("status", "Discharging")])
        .supply("BAT1", &[("type", "Battery"), ("energy_now", "1"), ("energy_full", "2"), ("status", "Charging")]);
    assert!(system_battery(&charging.0).unwrap().expect("batteries").on_ac_power);

    // The other battery idles while this one discharges, on a computer without a supply to ask.
    let discharging = Fixture::new();
    discharging
        .supply("BAT0", &[("type", "Battery"), ("energy_now", "1"), ("energy_full", "2"), ("status", "Discharging")])
        .supply("BAT1", &[("type", "Battery"), ("energy_now", "2"), ("energy_full", "2"), ("status", "Full")]);
    assert!(!system_battery(&discharging.0).unwrap().expect("batteries").on_ac_power);
}
