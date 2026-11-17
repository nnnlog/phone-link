//! The engine's events: how they reach the GUI thread, and the part of the app each one goes to.

use std::pin::Pin;

use cxx_qt::{CxxQtThread, CxxQtType};
use cxx_qt_lib::QString;
use phonelink_engine::api::events::{EngineEvent, EventSink};
use phonelink_engine::api::phone_apps::PhoneAppsChange;
use tracing::info;

use super::EngineBridge;
use super::features::Revision;
use crate::images;
use crate::sign_in::{SignInPage, page_script};

/// Hands each engine event to the bridge on the GUI thread.
pub(super) struct QtEventSink {
    thread: CxxQtThread<EngineBridge>,
}

impl QtEventSink {
    pub(super) fn new(thread: CxxQtThread<EngineBridge>) -> Self {
        Self { thread }
    }
}

impl EventSink for QtEventSink {
    fn on_event(&self, event: EngineEvent) {
        if self.thread.queue(move |bridge| bridge.handle_event(event)).is_err() {
            info!("an engine event arrived after the window closed");
        }
    }
}

impl EngineBridge {
    fn handle_event(mut self: Pin<&mut Self>, event: EngineEvent) {
        match event {
            EngineEvent::WebSignInRequested { request_id, request } => {
                let page = SignInPage::new(
                    request_id,
                    request.kind,
                    request.url.clone(),
                    request.headers,
                    request.end_url_prefix,
                );
                self.as_mut().rust_mut().sign_in = Some(page);
                if !self.starting && self.phase != QString::from("signIn") {
                    let phase = self.phase.clone();
                    self.as_mut().rust_mut().phase_before_sign_in = Some(phase);
                }
                self.as_mut().set_sign_in_url(QString::from(request.url.as_str()));
                self.as_mut().set_sign_in_user_agent(QString::from(request.user_agent.as_str()));
                self.as_mut().set_sign_in_script(QString::from(page_script(&request.bridge_script).as_str()));
                self.set_phase(QString::from("signIn"));
            }
            EngineEvent::PhoneSession { .. } => {
                self.as_mut().reload_phone();
                self.as_mut().update_call_alerts();
                self.as_mut().bump(Revision::Settings);
                self.bump(Revision::Mirroring);
            }
            // The phone's record or permissions changed, or another phone was selected: what each part
            // reads from the phone is read again.
            EngineEvent::PhoneChanged => {
                self.as_mut().reload_phone();
                self.as_mut().update_call_alerts();
                self.as_mut().reload_notifications();
                self.as_mut().reload_notification_state();
                self.as_mut().apps_changed(PhoneAppsChange::StateChanged);
                for revision in [Revision::Settings, Revision::Wallpaper, Revision::Media, Revision::Mirroring] {
                    self.as_mut().bump(revision);
                }
            }
            EngineEvent::PhoneUnlinked => {
                images::clear_all();
                self.as_mut().drop_media_player();
                self.as_mut().reload_phones();
                self.set_phase(QString::from("selectingPhone"));
            }
            EngineEvent::AllFeaturesDisabled { disabled } => {
                self.as_mut().set_features_disabled(disabled);
                self.reload_status();
            }
            EngineEvent::BringWindowForward { .. } => self.show_section(QString::default()),
            EngineEvent::ScreenMirroring { change } => self.mirroring_changed(change),
            EngineEvent::PhoneFiles { change } => self.phone_files_changed(change),
            EngineEvent::DeviceStatus { change } => self.device_status_changed(change),
            EngineEvent::Notifications { change } => self.notifications_changed(change),
            EngineEvent::Messaging { change } => self.messaging_changed(change),
            EngineEvent::Contacts { change } => self.contacts_changed(change),
            EngineEvent::Calling { change } => self.calling_changed(change),
            EngineEvent::Photos { change } => self.photos_changed(change),
            EngineEvent::PhoneApps { change } => self.apps_changed(change),
            EngineEvent::RemoteSnipping { change } => self.snipping_changed(change),
            EngineEvent::Audio { change } => self.audio_changed(change),
            EngineEvent::FileTransfer { change } => self.file_transfer_changed(change),
            EngineEvent::Clipboard { .. } | EngineEvent::RemoteControl { .. } => self.shell_settings_changed(),
            EngineEvent::ConnectedCamera { change } => self.camera_changed(change),
            EngineEvent::Wallpaper { change } => self.wallpaper_changed(change),
            EngineEvent::Pairing { .. } => self.pairing_changed(),
            EngineEvent::ConnectCompleted { immediate, outcome } => self.connect_completed(immediate, outcome),
            EngineEvent::Resync { feature } => self.resync(feature),
            EngineEvent::AccountRemoved => {
                images::clear_all();
                self.as_mut().drop_media_player();
                self.set_phase(QString::from("accountRemoved"));
            }
        }
    }
}
