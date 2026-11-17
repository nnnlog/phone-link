//! Low power mode from power-profiles-daemon: whether its power saver profile is on.

use phonelink_engine::edge::EdgeError;

use super::DesktopPcStatus;
use crate::dbus::{left_the_bus, name_owner, uncached};
use crate::edges::unexpected;

/// power-profiles-daemon's bus names, each with its object path; each name is also the interface.
/// Newer releases own both names, older ones only `net.hadess.PowerProfiles`.
const POWER_PROFILES: [(&str, &str); 2] = [
    ("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles"),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];
const POWER_SAVER: &str = "power-saver";

impl DesktopPcStatus {
    /// The power saver profile is on; off while power-profiles-daemon does not run.
    pub(super) fn power_saver_on(&self) -> Result<bool, EdgeError> {
        let bus = self.bus()?;

        for (name, path) in POWER_PROFILES {
            // Only a running daemon is asked, through its own connection rather than its name: both
            // names are D-Bus activatable, and reading the status must not start power-profiles-daemon
            // on a computer whose power something else manages.
            let profile = name_owner(&bus, name).and_then(|owner| {
                owner
                    .map(|owner| uncached(&bus, owner, path, name)?.get_property::<String>("ActiveProfile"))
                    .transpose()
            });
            match profile {
                Ok(Some(profile)) => return Ok(profile == POWER_SAVER),
                Ok(None) => {}
                // It stopped while it was asked.
                Err(e) if left_the_bus(&e) => {}
                Err(e) => {
                    self.bus.after_error(&e);
                    return Err(unexpected(format!("{name}: {e}")));
                }
            }
        }

        Ok(false)
    }
}
