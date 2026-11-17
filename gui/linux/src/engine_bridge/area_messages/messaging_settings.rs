//! The messaging settings.
//!
//! Messages turned on and allowed on the phone, the Settings window's switches, and its request for the
//! phone's contacts.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::engine::Engine;
use tracing::warn;

use super::arguments::Setting;
use super::refresh::LIST;
use super::reload::not_started;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision, fail_later};
use crate::user_text;

impl EngineBridge {
    /// Asks the phone to allow messages on this computer.
    pub(super) fn request_permission(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.messages.requesting_permission {
            return;
        }
        self.as_mut().msg().requesting_permission = true;
        self.as_mut().mark(LIST);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let failure = match engine.messaging().request_permission().await {
                Ok(outcome) => user_text::outcome_failure(&outcome),
                Err(e) => Some(user_text::feature_error(&e)),
            };
            if let Some(text) = failure {
                fail_later(&thread, Failure::new("Messages Not Allowed", text));
            }
            let _ = thread.queue(|mut bridge| {
                bridge.as_mut().msg().requesting_permission = false;
                bridge.as_mut().reload_state();
                bridge.mark(LIST);
            });
        });
    }

    /// Turns on "Show messages on this computer".
    pub(super) fn turn_on_messages(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        let Some(mut settings) = self.areas.messages.state.as_ref().map(|s| s.settings) else { return };
        settings.pc_consent = true;
        match engine.messaging().set_settings(settings) {
            Ok(()) => {
                self.as_mut().reload_state();
                self.as_mut().reload_snapshot();
            }
            Err(e) => self.as_mut().show_failure(Failure::feature("Messages Not Turned On", &e)),
        }
        self.bump(Revision::Settings);
    }

    /// The Settings window's switches.
    ///
    /// The engine takes the messaging settings whole, so they are written back with one field changed.
    pub(super) fn change_setting(mut self: Pin<&mut Self>, engine: Arc<Engine>, setting: &Setting) {
        let messaging = engine.messaging();
        let changed = messaging.state().and_then(|state| {
            let mut settings = state.settings;
            match setting.name.as_str() {
                "showMessages" => settings.pc_consent = setting.on,
                "notify" => settings.message_notifications = setting.on,
                "autoDownload" => settings.auto_download_mms = setting.on,
                other => {
                    warn!(action = other, "unknown messaging setting");
                    return Ok(());
                }
            }
            messaging.set_settings(settings)
        });

        // Before the engine starts the pane's switches are off; a `NotStarted` here only means it
        // stopped meanwhile.
        if let Err(e) = changed
            && !not_started(&e)
        {
            self.as_mut().show_failure(Failure::setting(&e));
        }
        self.as_mut().reload_state();
        self.as_mut().reload_snapshot();
        self.bump(Revision::Settings);
    }

    /// The Settings window asks the phone for contacts access; a grant is followed by a contacts sync.
    pub(super) fn request_contacts_access(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.messages.settings_requesting_contacts {
            return;
        }
        self.as_mut().msg().settings_requesting_contacts = true;
        self.as_mut().bump(Revision::Settings);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let failure = match engine.contacts().request_permission().await {
                Ok(result) if result.timed_out => user_text::outcome_failure(&RequestOutcome::TimedOut),
                Ok(result) => user_text::outcome_failure(&result.outcome),
                Err(e) => Some(user_text::feature_error(&e)),
            };
            if let Some(text) = failure {
                fail_later(&thread, Failure::new("Couldn't Request Access", text));
            }
            let _ = thread.queue(|mut bridge| {
                bridge.as_mut().msg().settings_requesting_contacts = false;
                bridge.as_mut().reload_contacts_state();
                bridge.bump(Revision::Settings);
            });
        });
    }
}
