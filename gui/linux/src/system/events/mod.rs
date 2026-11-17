//! The screen lock's changes and sleep and wake, as the engine's system events.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::engine::SystemEvent;
use tracing::warn;
use zbus::blocking::Proxy;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

use super::METHOD_TIMEOUT;
use super::desktop::Desktop;
use super::endpoint::{
    Endpoint, LOGIN1, LOGIN1_MANAGER_ENDPOINT, LOGIN1_SESSION, LOGIN1_SESSION_ENDPOINT, endpoint_proxy,
};
use super::lock::no_lock_service;
use super::supervise::supervise;
use crate::dbus::{Bus, PropertyChange, connect, property_change, property_changes, uncached};

#[cfg(test)]
mod tests;

/// What logind lists for the delay lock this app holds on sleep.
const SLEEP_DELAY_WHO: &str = "Phone Link";
const SLEEP_DELAY_WHY: &str = "Stop connecting to the phone before sleep";

/// Watches the screen lock and sleep for the engine, each on a thread of its own for as long as the
/// app runs. Each watcher makes its own connection, and starts again with back-off when that
/// connection ends or the watcher fails.
pub fn watch_system_events(engine: Arc<Engine>, desktop: Desktop) {
    let lock_engine = engine.clone();
    if desktop.sets_locked_hint() {
        let mut locked = None;
        supervise("lock", move || watch_locked_hint(&lock_engine, &mut locked));
    } else {
        let screensaver = desktop.screensaver();
        let mut locked = None;
        supervise("lock", move || watch_screensaver(&lock_engine, &screensaver, &mut locked));
    }
    supervise("sleep", move || watch_sleep(&engine));
}

/// The event for the screen turning `locked` or not.
fn lock_event(locked: bool) -> SystemEvent {
    if locked { SystemEvent::ScreenLocked } else { SystemEvent::ScreenUnlocked }
}

/// The event for a lock state a watcher sees, given the one it saw last: none for its first state,
/// which the engine reads through `DesktopLock` itself, or for one that did not change.
fn lock_change(last: Option<bool>, now: bool) -> Option<SystemEvent> {
    match last {
        Some(was) if was != now => Some(lock_event(now)),
        _ => None,
    }
}

/// Records the lock state `now` and tells the engine when it changed. `last` outlives each run of the
/// watcher, so a change while the watcher started again is still told.
fn report_lock(engine: &Engine, last: &mut Option<bool>, now: bool) {
    if let Some(event) = lock_change(*last, now) {
        engine.system_event(event);
    }
    *last = Some(now);
}

/// Reports logind's `LockedHint` of the session and its changes.
fn watch_locked_hint(engine: &Engine, locked: &mut Option<bool>) -> zbus::Result<()> {
    let bus = connect(Bus::System, Some(METHOD_TIMEOUT))?;
    // The session announces changes on its own object, not on the `auto` alias.
    let id: String = endpoint_proxy(&bus, &LOGIN1_SESSION_ENDPOINT)?.get_property("Id")?;
    let path: OwnedObjectPath = endpoint_proxy(&bus, &LOGIN1_MANAGER_ENDPOINT)?.call("GetSession", &(id,))?;
    let changes = property_changes(&bus, LOGIN1, path.clone(), LOGIN1_SESSION)?;
    let session = uncached(&bus, LOGIN1, path, LOGIN1_SESSION)?;

    report_lock(engine, locked, session.get_property("LockedHint")?);

    for signal in changes {
        match property_change(signal.body().deserialize()?, LOGIN1_SESSION, "LockedHint")? {
            PropertyChange::Changed(now) => report_lock(engine, locked, now),
            PropertyChange::Invalidated => report_lock(engine, locked, session.get_property("LockedHint")?),
            PropertyChange::Unchanged => {}
        }
    }

    Ok(())
}

/// Reports whether the screensaver at `endpoint` is active, and its `ActiveChanged` signals.
fn watch_screensaver(engine: &Engine, endpoint: &Endpoint, locked: &mut Option<bool>) -> zbus::Result<()> {
    let bus = connect(Bus::Session, Some(METHOD_TIMEOUT))?;
    let screensaver = endpoint_proxy(&bus, endpoint)?;
    let changes = screensaver.receive_signal("ActiveChanged")?;

    match screensaver.call("GetActive", &()) {
        Ok(active) => report_lock(engine, locked, active),
        // As `DesktopLock` reads it: without a lock service the screen counts as unlocked. Its signals
        // are still watched, for a service that starts later.
        Err(e) if no_lock_service(&e) => report_lock(engine, locked, false),
        Err(e) => return Err(e),
    }

    for signal in changes {
        report_lock(engine, locked, signal.body().deserialize()?);
    }

    Ok(())
}

/// Tells the engine of sleep and wake. A logind delay lock makes sleep wait until the engine has
/// handled will-sleep (or until logind's `InhibitDelayMaxSec`): it is released once the engine
/// returns, and taken again on wake.
fn watch_sleep(engine: &Engine) -> zbus::Result<()> {
    let bus = connect(Bus::System, Some(METHOD_TIMEOUT))?;
    let manager = endpoint_proxy(&bus, &LOGIN1_MANAGER_ENDPOINT)?;
    // Subscribed before the lock is taken: a sleep that starts in between is still seen.
    let changes = manager.receive_signal("PrepareForSleep")?;
    let mut delay = sleep_delay(&manager);
    for signal in changes {
        if signal.body().deserialize()? {
            engine.system_event(SystemEvent::WillSleep);
            drop(delay.take());
        } else {
            delay = sleep_delay(&manager);
            engine.system_event(SystemEvent::DidWake);
        }
    }

    Ok(())
}

/// A logind delay lock on sleep, held while the descriptor stays open. Without one, sleep and wake are
/// still told, but will-sleep may then be handled only after the computer wakes.
fn sleep_delay(manager: &Proxy) -> Option<OwnedFd> {
    match manager.call("Inhibit", &("sleep", SLEEP_DELAY_WHO, SLEEP_DELAY_WHY, "delay")) {
        Ok(lock) => Some(lock),
        Err(e) => {
            warn!(error = %e, "logind gave no delay lock on sleep; will-sleep may be handled after wake");
            None
        }
    }
}
