//! This computer's own batteries from sysfs (`/sys/class/power_supply`), combined as `UPower`'s display
//! device combines them, and whether it runs on external power.

use std::fs;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use phonelink_device_status::edge::PcBattery;

#[cfg(test)]
mod tests;

/// A battery `status` this app does not know.
#[derive(Debug, thiserror::Error)]
#[error("status {0:?}")]
struct UnknownStatus(String);

/// A battery's `status`, and the batteries' combined one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatteryStatus {
    Unknown,
    Charging,
    Discharging,
    /// On external power but held below full, at a charge limit.
    NotCharging,
    Full,
}

impl BatteryStatus {
    /// The status a battery's `status` attribute names; unknown without one.
    fn parse(status: Option<&str>) -> Result<Self, UnknownStatus> {
        match status {
            None | Some("Unknown") => Ok(Self::Unknown),
            Some("Charging") => Ok(Self::Charging),
            Some("Discharging") => Ok(Self::Discharging),
            Some("Not charging") => Ok(Self::NotCharging),
            Some("Full") => Ok(Self::Full),
            Some(other) => Err(UnknownStatus(other.to_owned())),
        }
    }

    /// The batteries' statuses together, as `UPower` combines them: charging when any battery charges,
    /// else discharging when any discharges, else full when every battery is, else held at a charge
    /// limit when every battery is full or held; unknown otherwise.
    fn combined(statuses: &[Self]) -> Self {
        let every = |allowed: &[Self]| !statuses.is_empty() && statuses.iter().all(|status| allowed.contains(status));
        if statuses.contains(&Self::Charging) {
            Self::Charging
        } else if statuses.contains(&Self::Discharging) {
            Self::Discharging
        } else if every(&[Self::Full]) {
            Self::Full
        } else if every(&[Self::Full, Self::NotCharging]) {
            Self::NotCharging
        } else {
            Self::Unknown
        }
    }
}

/// What one battery reports of its charge and status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BatteryReading {
    /// `energy_now` and `energy_full`, in µWh.
    energy: Option<(i64, i64)>,
    /// `charge_now` and `charge_full`, in µAh.
    charge: Option<(i64, i64)>,
    /// `capacity`, the driver's own percentage.
    capacity: Option<i64>,
    status: BatteryStatus,
}

impl BatteryReading {
    fn read(battery: &Path) -> io::Result<Self> {
        let pair =
            |now, full| -> io::Result<Option<(i64, i64)>> { Ok(number(battery, now)?.zip(number(battery, full)?)) };
        let status = BatteryStatus::parse(attribute(battery, "status")?.as_deref())
            .map_err(|e| io::Error::other(format!("{}: {e}", battery.display())))?;
        Ok(Self {
            energy: pair("energy_now", "energy_full")?,
            charge: pair("charge_now", "charge_full")?,
            capacity: number(battery, "capacity")?,
            status,
        })
    }
}

/// The batteries' charge as `(now, full)`, as `UPower`'s display device reports it: a lone battery's own
/// percentage (else its energy, else its charge), and for several batteries the sum of their energy,
/// or of their charge where they report no energy. `None` when the batteries report nothing that adds
/// up.
fn combined_charge(batteries: &[BatteryReading]) -> Option<(i64, i64)> {
    let sum = |pair: fn(&BatteryReading) -> Option<(i64, i64)>| {
        batteries.iter().map(pair).try_fold((0, 0), |(now, full), pair| pair.map(|(n, f)| (now + n, full + f)))
    };

    match batteries {
        [] => None,
        [lone] => lone.capacity.map(|percent| (percent, 100)).or(lone.energy).or(lone.charge),
        _ => sum(|battery| battery.energy).or_else(|| sum(|battery| battery.charge)),
    }
}

/// Whether the computer runs on external power: a mains or USB supply is online, or a battery
/// charges. A battery that is full or held at a charge limit counts only on a computer without a mains
/// or USB supply, whose online state otherwise says whether external power is there.
fn on_ac_power(status: BatteryStatus, has_external_supply: bool, external_online: bool) -> bool {
    external_online
        || status == BatteryStatus::Charging
        || (!has_external_supply && matches!(status, BatteryStatus::Full | BatteryStatus::NotCharging))
}

/// This computer's own batteries among the power supplies under `root` (laid out like
/// `/sys/class/power_supply`), combined as `UPower`'s display device combines them; `None` without one.
/// Supplies whose scope is `Device` power a peripheral (a mouse, a headset) rather than the computer
/// and are left out, and so is a battery bay without its battery.
pub(super) fn system_battery(root: &Path) -> io::Result<Option<PcBattery>> {
    let mut batteries = Vec::new();
    let mut external = Vec::new();
    for supply in supplies(root)? {
        if attribute(&supply, "scope")?.as_deref() == Some("Device") {
            continue;
        }
        match attribute(&supply, "type")?.as_deref() {
            Some("Battery") => {
                // An empty bay keeps its supply, which then has no values to read.
                if number(&supply, "present")? != Some(0) {
                    batteries.push(BatteryReading::read(&supply)?);
                }
            }
            Some("Mains" | "USB") => external.push(supply),
            _ => {}
        }
    }

    if batteries.is_empty() {
        return Ok(None);
    }

    let (now, full) = combined_charge(&batteries)
        .ok_or_else(|| io::Error::other("the batteries report no percentage, energy or charge that adds up"))?;
    let in_range = |value: i64| {
        i32::try_from(value).map_err(|_| io::Error::other(format!("a battery total of {value} is out of range")))
    };
    let statuses: Vec<BatteryStatus> = batteries.iter().map(|battery| battery.status).collect();
    Ok(Some(PcBattery {
        current_capacity: in_range(now)?,
        max_capacity: in_range(full)?,
        on_ac_power: on_ac_power(BatteryStatus::combined(&statuses), !external.is_empty(), any_online(&external)?),
    }))
}

/// The power supplies under `root`, by name; none when the kernel has no power supply class.
fn supplies(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut supplies = Vec::new();
    match fs::read_dir(root) {
        Ok(entries) => {
            for entry in entries {
                supplies.push(entry?.path());
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(supplies),
        Err(e) => return Err(e),
    }

    supplies.sort();
    Ok(supplies)
}

/// Some supply among `supplies` is online. `online` is 1 for a fixed-voltage supply and 2 for a
/// programmable one (USB Power Delivery), 0 while offline.
fn any_online(supplies: &[PathBuf]) -> io::Result<bool> {
    for supply in supplies {
        if number(supply, "online")?.is_some_and(|online| online != 0) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// A sysfs attribute of `supply` without its line end; `None` when the supply does not have it.
fn attribute(supply: &Path, name: &str) -> io::Result<Option<String>> {
    let file = supply.join(name);

    match fs::read_to_string(&file) {
        Ok(value) => Ok(Some(value.trim().to_owned())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io::Error::new(e.kind(), format!("{}: {e}", file.display()))),
    }
}

/// A numeric sysfs attribute of `supply`; `None` when the supply does not have it.
fn number(supply: &Path, name: &str) -> io::Result<Option<i64>> {
    attribute(supply, name)?
        .map(|value| {
            value.parse().map_err(|e| io::Error::other(format!("{}: {name} {value:?}: {e}", supply.display())))
        })
        .transpose()
}
