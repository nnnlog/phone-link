//! The session's lock state, read when the engine asks.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use phonelink_core::sync::MutexExt;
use phonelink_engine::edge::{LockState, SessionLock};
use tracing::{info, warn};
use zbus::blocking::Proxy;

use super::METHOD_TIMEOUT;
use super::desktop::Desktop;
use super::endpoint::{LOGIN1_SESSION_ENDPOINT, endpoint_proxy};
use crate::dbus::{
    Bus, NAME_HAS_NO_OWNER, SERVICE_UNKNOWN, UNKNOWN_INTERFACE, UNKNOWN_METHOD, UNKNOWN_OBJECT, connect,
    connection_lost, dbus_error_is,
};

#[cfg(test)]
mod tests;

/// The desktop has no screen-lock service at all (nothing owns the name, or the object lacks the
/// interface), as opposed to one that failed to answer. Without a service there is no lock to see, and
/// treating the screen as always locked would turn off every feature that needs it unlocked.
pub(super) fn no_lock_service(error: &zbus::Error) -> bool {
    dbus_error_is(error, &[SERVICE_UNKNOWN, NAME_HAS_NO_OWNER, UNKNOWN_METHOD, UNKNOWN_INTERFACE, UNKNOWN_OBJECT])
}

/// The session's lock state, read on every call: logind's `LockedHint` on desktops that set it, whether
/// the desktop's screensaver is active on the others.
pub struct DesktopLock {
    desktop: Desktop,
    /// The proxy every read goes through, on a connection whose calls time out: made on the first
    /// read, and again after its connection was lost.
    reader: Mutex<Option<Proxy<'static>>>,
    /// Reads are failing: a failure is logged when they start to, not on every read.
    failing: AtomicBool,
}

impl DesktopLock {
    pub fn new(desktop: Desktop) -> Self {
        Self { desktop, reader: Mutex::new(None), failing: AtomicBool::new(false) }
    }

    fn reader(&self) -> zbus::Result<Proxy<'static>> {
        let mut reader = self.reader.lock_or_recover();
        if let Some(proxy) = reader.as_ref() {
            return Ok(proxy.clone());
        }
        let proxy = if self.desktop.sets_locked_hint() {
            endpoint_proxy(&connect(Bus::System, Some(METHOD_TIMEOUT))?, &LOGIN1_SESSION_ENDPOINT)?
        } else {
            endpoint_proxy(&connect(Bus::Session, Some(METHOD_TIMEOUT))?, &self.desktop.screensaver())?
        };
        Ok(reader.insert(proxy).clone())
    }

    fn locked(&self) -> zbus::Result<bool> {
        let read = self.reader().and_then(|reader| {
            if self.desktop.sets_locked_hint() {
                reader.get_property("LockedHint")
            } else {
                reader.call("GetActive", &())
            }
        });
        if read.as_ref().is_err_and(connection_lost) {
            *self.reader.lock_or_recover() = None;
        }
        read
    }
}

impl SessionLock for DesktopLock {
    /// A read that fails or times out is unknown, which every rule that gates on the lock counts as
    /// locked; where the screensaver is read, no screen-lock service at all counts as unlocked.
    fn lock_state(&self) -> LockState {
        let state = match self.locked() {
            Ok(true) => LockState::Locked,
            Ok(false) => LockState::Unlocked,
            Err(e) if !self.desktop.sets_locked_hint() && no_lock_service(&e) => {
                static WARNED: std::sync::Once = std::sync::Once::new();
                WARNED.call_once(|| {
                    warn!(error = %e, "this desktop offers no screen-lock service; the screen counts as unlocked");
                });
                LockState::Unlocked
            }
            Err(e) => {
                if !self.failing.swap(true, Ordering::Relaxed) {
                    warn!(error = %e, "the screen lock state was not read; it counts as unknown");
                }
                return LockState::Unknown;
            }
        };
        if self.failing.swap(false, Ordering::Relaxed) {
            info!("the screen lock state is read again");
        }
        state
    }
}
