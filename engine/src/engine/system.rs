//! The Mac's lock, unlock, sleep and wake, the account's removal, and notification responses.

use super::{CdehTrigger, CoreEvent, EngineCore, SystemEvent};
use crate::app::ConnectTrigger;
use crate::features::NotificationResponse;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use tracing::{info, warn};

impl EngineCore {
    /// The Mac's session was locked or unlocked, or it is going to sleep or woke (macOS:
    /// `com.apple.screenIsLocked` / `com.apple.screenIsUnlocked`, `NSWorkspace` will-sleep /
    /// did-wake). Lock and sleep are the disconnect trigger: an in-flight connect is cancelled.
    /// Unlock and wake reset every wake circuit breaker, remove every partner from the partner set with
    /// reason `SystemReset`, which re-wakes it, and send the background connect — once the App layer
    /// has started: while this core is still starting the connect is not sent (its start sends the
    /// app-start connect). Once started, a lock also stops the CDEH identity's session — with it the
    /// App layer's messages, which share its connection — and an unlock starts it again; a wake starts
    /// it unless the Mac is still locked, a start that then starts nothing but still counts as a
    /// change, as every start CDEH makes while locked does.
    pub fn system_event(&self, event: SystemEvent) {
        let _runtime = self.runtime.enter();
        info!(?event, "system event");
        let started = self.started.load(Ordering::SeqCst);

        match event {
            SystemEvent::ScreenLocked | SystemEvent::WillSleep => {
                self.app.trigger(ConnectTrigger::LockOrSleep);

                if event == SystemEvent::ScreenLocked {
                    if let Some(features) = self.features() {
                        features.connected_camera.screen_lock_changed(true);
                        features.screen_mirroring.screen_lock_changed(true);
                    }
                }

                if event == SystemEvent::ScreenLocked && started {
                    self.stop_cdeh_session();
                }
            }
            SystemEvent::ScreenUnlocked | SystemEvent::DidWake => {
                if event == SystemEvent::ScreenUnlocked {
                    if let Some(features) = self.features() {
                        features.connected_camera.screen_lock_changed(false);
                    }
                }

                self.waker.reset_breakers();
                self.relay.system_reset();
                self.app.trigger(ConnectTrigger::UnlockOrWake);

                match (event, started) {
                    // The unlock event is the unlock: the session dictionary may not show it yet.
                    (SystemEvent::ScreenUnlocked, true) => self.start_cdeh_session(CdehTrigger::SessionUnlocked),
                    (_, true) => self.connect_cdeh(CdehTrigger::ProcessSessionChanges),
                    (_, false) => {}
                }
            }
        }
    }

    /// The account was removed (the identity was deprovisioned) while the features ran: in-flight
    /// connects end, the clipboard stops and sends nothing, and the core needs a rebuild. App requests
    /// already fail locally (the account gate), and so do the device-status pulls (no enrolled
    /// account). Runs once.
    pub(super) fn on_account_removed(&self) {
        if self.account_removed.swap(true, Ordering::SeqCst) {
            return;
        }

        warn!("the account was removed; the features stop and the engine must be rebuilt");
        self.app.cancel_connects();
        if let Some(features) = self.features() {
            features.clipboard.shutdown();
        }
        self.events.core_event(CoreEvent::AccountRemoved);
    }

    /// Routes what the user did with a posted system notification to the feature that owns it. The
    /// foreign side calls this on its own thread; it runs inside the engine's runtime.
    pub fn notification_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let _runtime = self.runtime.enter();
        self.responses.route(id, user_info, response);
    }
}
