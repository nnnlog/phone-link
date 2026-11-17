//! The routing of the bridge's `query` and `perform` names, its `areaEvent` signal and its events'
//! resync to the part of the app that owns each.

use std::pin::Pin;

use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureKind;
use phonelink_engine::api::audio::AudioChange;
use phonelink_engine::api::calling::CallingChange;
use phonelink_engine::api::connected_camera::ConnectedCameraChange;
use phonelink_engine::api::contacts::ContactsChange;
use phonelink_engine::api::device_status::DeviceStatusChange;
use phonelink_engine::api::file_transfer::FileTransferChange;
use phonelink_engine::api::messaging::MessagingChange;
use phonelink_engine::api::notifications::NotificationsChange;
use phonelink_engine::api::phone_apps::PhoneAppsChange;
use phonelink_engine::api::phone_files::PhoneFilesChange;
use phonelink_engine::api::photos::PhotosChange;
use phonelink_engine::api::remote_snipping::RemoteSnippingChange;
use phonelink_engine::api::screen_mirroring::ScreenMirroringChange;
use phonelink_engine::api::wallpaper::WallpaperChange;
use serde_json::Value;
use tracing::warn;

use super::Revision;
use crate::engine_bridge::{EngineBridge, mirroring};

impl EngineBridge {
    /// Every part reads what it shows again, and every view re-reads.
    pub(in crate::engine_bridge) fn reload_features(mut self: Pin<&mut Self>) {
        self.as_mut().reload_notifications();
        self.as_mut().reload_notification_state();
        self.as_mut().reload_conversations();
        self.as_mut().reload_messages();
        for revision in Revision::ALL {
            self.as_mut().bump(revision);
        }
    }

    /// A feature's events were lost: its part reads everything it shows again, through the handlers of
    /// the changes that cover it.
    pub(in crate::engine_bridge) fn resync(mut self: Pin<&mut Self>, feature: FeatureKind) {
        match feature {
            FeatureKind::Audio => self.audio_changed(AudioChange::Changed),
            FeatureKind::Notifications => {
                for change in [
                    NotificationsChange::ListChanged,
                    NotificationsChange::StateChanged,
                    NotificationsChange::SettingsChanged,
                ] {
                    self.as_mut().notifications_changed(change);
                }
            }
            FeatureKind::Clipboard => self.shell_settings_changed(),
            FeatureKind::DeviceStatus => self.device_status_changed(DeviceStatusChange::Changed),
            FeatureKind::Messaging => {
                // An empty list means every conversation changed.
                self.as_mut().messaging_changed(MessagingChange::ContentChanged { threads: Vec::new() });
                self.messaging_changed(MessagingChange::StateChanged);
            }
            FeatureKind::Contacts => {
                self.as_mut().contacts_changed(ContactsChange::RecordsChanged);
                self.contacts_changed(ContactsChange::StateChanged);
            }
            FeatureKind::Calling => {
                self.as_mut().calling_changed(CallingChange::StateChanged);
                self.calling_changed(CallingChange::HistoryChanged);
            }
            FeatureKind::Photos => {
                self.as_mut().photos_changed(PhotosChange::GalleryChanged);
                self.photos_changed(PhotosChange::StateChanged);
            }
            FeatureKind::Wallpaper => self.wallpaper_changed(WallpaperChange::Changed),
            FeatureKind::RemoteSnipping => self.snipping_changed(RemoteSnippingChange::SettingsChanged),
            FeatureKind::FileTransfer => self.file_transfer_changed(FileTransferChange::Changed),
            FeatureKind::PhoneFiles => self.phone_files_changed(PhoneFilesChange::StateChanged),
            FeatureKind::ConnectedCamera => self.camera_changed(ConnectedCameraChange::Changed),
            FeatureKind::PhoneApps => {
                self.as_mut().apps_changed(PhoneAppsChange::AppsChanged);
                self.apps_changed(PhoneAppsChange::RecentChanged);
            }
            FeatureKind::ScreenMirroring => self.mirroring_changed(ScreenMirroringChange::Changed),
        }
    }

    /// Tells the QML of `area` that `name` happened, with a JSON `payload` (the `areaEvent` signal).
    pub(in crate::engine_bridge) fn emit_area_event(self: Pin<&mut Self>, area: &str, name: &str, payload: &Value) {
        self.area_event(QString::from(area), QString::from(name), QString::from(payload.to_string().as_str()));
    }

    /// The JSON answer to `name`, from the part of the app that owns it.
    ///
    /// It is `null` for a name nobody owns, or for one that needs the engine before it exists.
    pub fn query(&self, name: &QString, arg: &QString) -> QString {
        let (name, arg) = (name.to_string(), arg.to_string());
        let value = self.query_local(&name, &arg).or_else(|| {
            let engine = self.engine.clone()?;
            self.query_messages(&engine, &name, &arg)
                .or_else(|| self.query_notifications(&engine, &name, &arg))
                .or_else(|| self.query_calls(&engine, &name, &arg))
                .or_else(|| self.query_photos(&engine, &name, &arg))
                .or_else(|| self.query_files(&engine, &name, &arg))
                .or_else(|| self.query_playback(&engine, &name, &arg))
                .or_else(|| self.query_camera(&engine, &name, &arg))
                .or_else(|| self.query_mirror(&engine, &name, &arg))
                .or_else(|| self.query_shell(&engine, &name, &arg))
        });
        QString::from(value.unwrap_or(Value::Null).to_string().as_str())
    }

    /// Runs the action `name` with `arg` in the part of the app that owns it.
    pub fn perform(mut self: Pin<&mut Self>, name: &QString, arg: &QString) {
        let (name, arg) = (name.to_string(), arg.to_string());
        if name == "mirror.display" {
            mirroring::set_display();
            return;
        }
        if self.as_mut().perform_local(&name, &arg) {
            return;
        }

        let Some(engine) = self.engine.clone() else { return };
        let handled = self.as_mut().perform_messages(engine.clone(), &name, &arg)
            || self.as_mut().perform_notifications(engine.clone(), &name, &arg)
            || self.as_mut().perform_calls(engine.clone(), &name, &arg)
            || self.as_mut().perform_photos(engine.clone(), &name, &arg)
            || self.as_mut().perform_files(engine.clone(), &name, &arg)
            || self.as_mut().perform_playback(engine.clone(), &name, &arg)
            || self.as_mut().perform_camera(engine.clone(), &name, &arg)
            || self.as_mut().perform_mirror(engine.clone(), &name, &arg)
            || self.as_mut().perform_shell(engine, &name, &arg);
        if !handled {
            warn!(action = %name, "unknown action");
        }
    }
}
