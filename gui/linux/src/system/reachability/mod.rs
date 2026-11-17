//! Internet reachability for the engine, from `NetworkManager`'s connectivity.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use phonelink_engine::edge::Reachability;
use zbus::blocking::Connection;
use zbus::blocking::proxy::SignalIterator;

use super::METHOD_TIMEOUT;
use super::supervise::supervise;
use crate::dbus::{
    Bus, PropertyChange, connect, left_the_bus, name_owner, property_change, property_changes, uncached,
};

#[cfg(test)]
mod tests;

const NETWORK_MANAGER: &str = "org.freedesktop.NetworkManager";
const NETWORK_MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";
/// `NM_CONNECTIVITY_UNKNOWN`: `NetworkManager` has not checked, or is not running.
const NM_CONNECTIVITY_UNKNOWN: u32 = 0;
/// `NM_CONNECTIVITY_NONE`: no network connection at all.
const NM_CONNECTIVITY_NONE: u32 = 1;

/// Internet reachability from `NetworkManager`'s connectivity, which a watcher thread keeps current from
/// `NetworkManager`'s property changes, so reading it never waits for `NetworkManager`.
pub struct NetworkManagerReachability {
    /// `NetworkManager`'s `Connectivity`; unknown while `NetworkManager` is not running, and while the
    /// watcher starts again.
    connectivity: Arc<AtomicU32>,
}

impl NetworkManagerReachability {
    /// Reads the connectivity before it returns, so the engine starts with it, and watches it from
    /// then on.
    pub fn new() -> Self {
        let connectivity = Arc::new(AtomicU32::new(NM_CONNECTIVITY_UNKNOWN));
        let mut first = Some(subscribe_connectivity(&connectivity));
        let watched = connectivity.clone();
        supervise("network", move || {
            let (bus, changes) = match first.take() {
                Some(subscribed) => subscribed?,
                None => subscribe_connectivity(&watched)?,
            };
            let followed = follow_connectivity(&bus, changes, &watched);
            watched.store(NM_CONNECTIVITY_UNKNOWN, Ordering::Relaxed);
            followed
        });
        Self { connectivity }
    }
}

impl Reachability for NetworkManagerReachability {
    fn is_internet_available(&self) -> bool {
        internet_available(self.connectivity.load(Ordering::Relaxed))
    }
}

/// Only `NetworkManager`'s "none" says there is no internet: an unknown connectivity counts as
/// available, so that no failure is put down to the network without a reason.
fn internet_available(connectivity: u32) -> bool {
    connectivity != NM_CONNECTIVITY_NONE
}

/// Subscribes to `NetworkManager`'s property changes and then reads its connectivity into
/// `connectivity`, in that order so that no change between the two is missed. A failed read leaves the
/// connectivity unknown.
fn subscribe_connectivity(connectivity: &AtomicU32) -> zbus::Result<(Connection, SignalIterator<'static>)> {
    let bus = connect(Bus::System, Some(METHOD_TIMEOUT))?;
    let changes = property_changes(&bus, NETWORK_MANAGER, NETWORK_MANAGER_PATH, NETWORK_MANAGER)?;

    match read_connectivity(&bus) {
        Ok(now) => {
            connectivity.store(now, Ordering::Relaxed);
            Ok((bus, changes))
        }
        Err(e) => {
            connectivity.store(NM_CONNECTIVITY_UNKNOWN, Ordering::Relaxed);
            Err(e)
        }
    }
}

/// `NetworkManager`'s connectivity now; unknown while `NetworkManager` is not on the bus.
fn read_connectivity(bus: &Connection) -> zbus::Result<u32> {
    let Some(owner) = name_owner(bus, NETWORK_MANAGER)? else { return Ok(NM_CONNECTIVITY_UNKNOWN) };
    match uncached(bus, owner, NETWORK_MANAGER_PATH, NETWORK_MANAGER)?.get_property("Connectivity") {
        Err(e) if left_the_bus(&e) => Ok(NM_CONNECTIVITY_UNKNOWN),
        read => read,
    }
}

/// Keeps `connectivity` current from `NetworkManager`'s property changes until the connection ends.
fn follow_connectivity(
    bus: &Connection,
    changes: SignalIterator<'static>,
    connectivity: &AtomicU32,
) -> zbus::Result<()> {
    for signal in changes {
        let now = match property_change(signal.body().deserialize()?, NETWORK_MANAGER, "Connectivity")? {
            PropertyChange::Changed(now) => now,
            PropertyChange::Invalidated => read_connectivity(bus)?,
            PropertyChange::Unchanged => continue,
        };
        connectivity.store(now, Ordering::Relaxed);
    }

    Ok(())
}
