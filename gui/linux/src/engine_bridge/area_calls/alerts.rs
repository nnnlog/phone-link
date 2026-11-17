//! What shows the phone's calls, kept current.
//!
//! The engine's calls and contacts events read the current calls again, with the incoming-call window,
//! its ring and the call window that show them, and the waits for calls and contacts to come up to date.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use phonelink_engine::api::calling::CallingChange;
use phonelink_engine::api::contacts::ContactsChange;
use phonelink_engine::api::engine::Engine;
use tracing::warn;

use super::presentation::{POPUP_LIFETIME, PresentationChange};
use super::queries::read;
use super::state::State;
use super::sync_wait::{SYNC_WAIT, Synced};
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::images;

/// Runs `then` on the GUI thread after `delay`.
fn after(
    thread: CxxQtThread<EngineBridge>,
    delay: Duration,
    then: impl FnOnce(Pin<&mut EngineBridge>) + Send + 'static,
) {
    let spawned = std::thread::Builder::new().name("calls-timer".to_owned()).spawn(move || {
        std::thread::sleep(delay);
        let _ = thread.queue(then);
    });
    if let Err(e) = spawned {
        warn!(error = %e, "a calls timer did not start");
    }
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn contacts_changed(mut self: Pin<&mut Self>, change: ContactsChange) {
        match change {
            ContactsChange::RecordsChanged => {
                images::clear_kind("contacts");
                // Contacts name the calls, the current ones too.
                self.as_mut().reload_calls();
                self.as_mut().bump(Revision::Calls);
            }
            ContactsChange::StateChanged => {
                if let Some(engine) = self.engine.clone() {
                    self.as_mut().watch_sync(&engine);
                }
            }
        }
        self.bump(Revision::Contacts);
    }

    pub(in crate::engine_bridge) fn calling_changed(mut self: Pin<&mut Self>, change: CallingChange) {
        if let (CallingChange::HistoryChanged | CallingChange::StateChanged, Some(engine)) =
            (change, self.engine.clone())
        {
            self.as_mut().refresh_calls_if_empty_now(engine);
        }
        self.as_mut().reload_calls();
        self.bump(Revision::Calls);
    }

    /// The phone's session or the window changed: the phone's current calls are read again.
    ///
    /// The views read them again when what shows them changed.
    pub(in crate::engine_bridge) fn update_call_alerts(mut self: Pin<&mut Self>) {
        if self.as_mut().reload_calls() {
            self.bump(Revision::Calls);
        }
    }

    /// Reads the phone's current calls again and updates what shows them; whether any of it changed.
    ///
    /// What shows them is the incoming-call window and its ring, and the call window; the waits for
    /// calls and contacts to come up to date are updated too.
    pub(super) fn reload_calls(mut self: Pin<&mut Self>) -> bool {
        let Some(engine) = self.engine.clone() else { return false };
        let mut changed = false;
        if let Some(calls) = read("the current calls", engine.clone().calling().active_calls()) {
            let mut rust = self.as_mut().rust_mut();
            if rust.areas.calls.calls != calls {
                rust.areas.calls.calls = calls;
                changed = true;
            }
        }

        changed |= self.as_mut().present_calls();
        self.as_mut().watch_sync(&engine);
        changed
    }

    /// Opens or closes the incoming-call window and the call window for the current calls.
    ///
    /// It rings while the incoming-call window shows, and starts each newly shown ringing call's
    /// lifetime; whether any of that changed.
    pub(super) fn present_calls(mut self: Pin<&mut Self>) -> bool {
        let change = {
            let mut rust = self.as_mut().rust_mut();
            let State { presentation, ringer, calls, .. } = &mut rust.areas.calls;
            let change = presentation.update(calls);
            match change.ring {
                Some(true) => ringer.start(),
                Some(false) => ringer.stop(),
                None => {}
            }
            change
        };

        for (id, token) in &change.lifetimes {
            let (id, token) = (*id, *token);
            after(self.qt_thread(), POPUP_LIFETIME, move |bridge| bridge.popup_expired(id, token));
        }
        change != PresentationChange::default()
    }

    fn popup_expired(mut self: Pin<&mut Self>, id: u64, token: u64) {
        if self.as_mut().rust_mut().areas.calls.presentation.expire(id, token) {
            self.as_mut().present_calls();
            self.bump(Revision::Calls);
        }
    }

    /// Starts the waits for calls and contacts that are not up to date.
    fn watch_sync(mut self: Pin<&mut Self>, engine: &Arc<Engine>) {
        let calls = read("the call history state", engine.clone().calling().state()).map(|state| state.up_to_date);
        let contacts = read("the contacts state", engine.clone().contacts().state()).map(|state| state.up_to_date);

        let mut started = Vec::new();
        {
            let mut rust = self.as_mut().rust_mut();
            let state = &mut rust.areas.calls;
            // Calls that fall behind again are waited for again; contacts once.
            if let Some(generation) = calls.and_then(|up_to_date| state.calls_wait.observe(up_to_date, true)) {
                started.push((Synced::Calls, generation));
            }
            if let Some(generation) = contacts.and_then(|up_to_date| state.contacts_wait.observe(up_to_date, false)) {
                started.push((Synced::Contacts, generation));
            }
        }

        for (synced, generation) in started {
            after(self.qt_thread(), SYNC_WAIT, move |bridge| bridge.sync_wait_over(synced, generation));
        }
    }

    fn sync_wait_over(mut self: Pin<&mut Self>, synced: Synced, generation: u64) {
        let (over, revision) = {
            let mut rust = self.as_mut().rust_mut();
            let state = &mut rust.areas.calls;
            match synced {
                Synced::Calls => (state.calls_wait.expire(generation), Revision::Calls),
                Synced::Contacts => (state.contacts_wait.expire(generation), Revision::Contacts),
            }
        };
        if over {
            self.bump(revision);
        }
    }
}
