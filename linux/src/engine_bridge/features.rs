use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
use phonelink_engine::api::connected_camera::PhoneCameraFace;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::file_transfer::FileSendOrigin;
use phonelink_engine::api::messaging::{MessageTarget, MessagingChange};
use phonelink_engine::api::notifications::NotificationsChange;
use phonelink_engine::api::phone_files::PhoneFilesPermissionOrigin;

use super::{EngineBridge, NO_THREAD};
use crate::catalog;
use crate::views::{conversations_json, messages_json, notifications_json, status_json};

fn report(what: &str, error: impl std::fmt::Display) {
    tracing::warn!("{what}: {error}");
}

impl EngineBridge {
    pub(super) fn reload_features(mut self: Pin<&mut Self>) {
        self.as_mut().reload_notifications();
        self.as_mut().reload_notification_state();
        self.as_mut().reload_conversations();
        self.as_mut().reload_messages();
        for revision in Revision::ALL {
            self.as_mut().bump(revision);
        }
    }

    pub(super) fn bump(mut self: Pin<&mut Self>, revision: Revision) {
        match revision {
            Revision::Contacts => {
                let next = self.contacts_revision + 1;
                self.as_mut().set_contacts_revision(next);
            }
            Revision::Photos => {
                let next = self.photos_revision + 1;
                self.as_mut().set_photos_revision(next);
            }
            Revision::Calls => {
                let next = self.calls_revision + 1;
                self.as_mut().set_calls_revision(next);
            }
            Revision::Apps => {
                let next = self.apps_revision + 1;
                self.as_mut().set_apps_revision(next);
            }
            Revision::Media => {
                let next = self.media_revision + 1;
                self.as_mut().set_media_revision(next);
            }
            Revision::Transfers => {
                let next = self.transfers_revision + 1;
                self.as_mut().set_transfers_revision(next);
            }
            Revision::Settings => {
                let next = self.settings_revision + 1;
                self.as_mut().set_settings_revision(next);
            }
            Revision::Wallpaper => {
                let next = self.wallpaper_revision + 1;
                self.as_mut().set_wallpaper_revision(next);
            }
            Revision::Mirroring => {
                let next = self.mirroring_revision + 1;
                self.as_mut().set_mirroring_revision(next);
            }
        }
    }

    pub fn query(&self, name: &QString, arg: &QString) -> QString {
        let Some(engine) = self.engine.clone() else { return QString::from("null") };
        QString::from(catalog::query(&engine, &name.to_string(), &arg.to_string()).to_string().as_str())
    }

    pub fn perform(self: Pin<&mut Self>, name: &QString, arg: &QString) {
        let (name, arg) = (name.to_string(), arg.to_string());
        if name == "mirror.display" {
            super::mirroring::set_display(&arg);
            return;
        }
        let Some(engine) = self.engine.clone() else { return };
        if name.starts_with("mirror.") {
            self.perform_mirror(engine, &name, &arg);
            return;
        }
        let flag = arg == "true";
        let done = match name.as_str() {
            "app.foreground" => {
                engine.set_foreground(flag);
                Ok(())
            }
            "media.playPause" => engine.audio().play_pause().map_err(|e| e.to_string()),
            "media.next" => engine.audio().next().map_err(|e| e.to_string()),
            "media.previous" => engine.audio().previous().map_err(|e| e.to_string()),
            "settings.clipboard" => engine.clipboard().set_phone_toggle(flag).map_err(|e| e.to_string()),
            "settings.remoteControl" => engine.remote_control().set_phone_toggle(flag).map_err(|e| e.to_string()),
            "settings.fileSharing" => engine.file_transfer().set_share_enabled(flag).map_err(|e| e.to_string()),
            "calls.markSeen" => engine.calling().mark_seen().map_err(|e| e.to_string()),
            "files.cancel" => engine.file_transfer().cancel_send().map_err(|e| e.to_string()),
            "photos.save" => save_photo(&engine, &arg),
            "photos.layout" => {
                let visible: Vec<i64> = serde_json::from_str(&arg).unwrap_or_default();
                engine.photos().grid_layout(visible).map_err(|e| e.to_string())
            }
            "photos.left" => engine.photos().gallery_left().map_err(|e| e.to_string()),
            "photos.shown" | "photos.hidden" => match arg.parse::<i64>() {
                Ok(id) if name == "photos.shown" => engine.photos().item_visible(id).map_err(|e| e.to_string()),
                Ok(id) => engine.photos().item_hidden(id).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            },
            "files.send" => {
                let paths: Vec<String> = serde_json::from_str::<Vec<String>>(&arg)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|url| url.strip_prefix("file://").map_or(url.clone(), str::to_owned))
                    .collect();
                self.runtime.spawn(async move {
                    if let Err(e) = engine.file_transfer().send(paths, FileSendOrigin::Window).await {
                        report("the files were not sent", e);
                    }
                });
                Ok(())
            }
            "phoneFiles.enable" => engine.clone().phone_files().set_enabled(flag).map_err(|e| e.to_string()),
            "phoneFiles.requestPermission" => {
                let files = engine.clone().phone_files();
                let thread = self.qt_thread();
                self.runtime.spawn(async move {
                    if let Err(e) = files.request_permission(PhoneFilesPermissionOrigin::StatusUi).await {
                        report("the files permission was not asked", e);
                    }
                    let _ = thread.queue(|bridge| bridge.bump(Revision::Settings));
                });
                Ok(())
            }
            "camera.enable" => engine.clone().connected_camera().set_camera_enabled(flag).map_err(|e| e.to_string()),
            "camera.use" => match crate::camera::camera().published() {
                Some(device) => {
                    crate::camera::camera().set_in_use(flag);
                    engine.clone().connected_camera().client_changed(device, flag).map_err(|e| e.to_string())
                }
                None => Err("no camera is published".to_owned()),
            },
            "camera.pause" => engine.clone().connected_camera().pause().map_err(|e| e.to_string()),
            "camera.resume" => engine.clone().connected_camera().resume().map_err(|e| e.to_string()),
            "camera.switch" => {
                let face = if arg == "Front" { PhoneCameraFace::Front } else { PhoneCameraFace::Back };
                engine.clone().connected_camera().switch_camera(face).map_err(|e| e.to_string())
            }
            "camera.refresh" => engine.clone().connected_camera().refresh().map_err(|e| e.to_string()),
            "camera.retry" => engine.clone().connected_camera().retry_registration().map_err(|e| e.to_string()),
            "camera.requestPermissions" => {
                engine.clone().connected_camera().request_permissions().map_err(|e| e.to_string())
            }
            "camera.closeControlWindow" => {
                engine.clone().connected_camera().close_control_window().map_err(|e| e.to_string())
            }
            "camera.closeError" => engine.clone().connected_camera().close_fatal_error().map_err(|e| e.to_string()),
            "calls.enable" => engine.calling().set_control_enabled(flag).map_err(|e| e.to_string()),
            "calls.turnOn" => engine.calling().set_pc_consent(true).map_err(|e| e.to_string()),
            "calls.connect"
            | "calls.pair"
            | "calls.dial"
            | "calls.answer"
            | "calls.decline"
            | "calls.end"
            | "calls.requestPermission"
            | "calls.refreshIfEmpty"
            | "hotspot.connect"
            | "hotspot.disconnect" => {
                let thread = self.qt_thread();
                let action = name.clone();
                self.runtime.spawn(async move {
                    let id = arg.parse::<u64>().unwrap_or_default();
                    let result = match action.as_str() {
                        "calls.connect" => {
                            engine.calling().connect_control().await.map(|_| ()).map_err(|e| e.to_string())
                        }
                        "calls.pair" => {
                            engine.calling().pair_for_control().await.map(|_| ()).map_err(|e| e.to_string())
                        }
                        "calls.dial" => engine.calling().dial(arg).await.map_err(|e| e.to_string()),
                        "calls.answer" => engine.calling().answer(id).await.map_err(|e| e.to_string()),
                        "calls.decline" => engine.calling().decline(id).await.map_err(|e| e.to_string()),
                        "calls.end" => engine.calling().end_call(id).await.map_err(|e| e.to_string()),
                        "calls.requestPermission" => {
                            engine.calling().request_permission().await.map(|_| ()).map_err(|e| e.to_string())
                        }
                        "calls.refreshIfEmpty" => {
                            engine.calling().refresh_if_empty().await.map(|_| ()).map_err(|e| e.to_string())
                        }
                        "hotspot.connect" => engine.hotspot().connect().await.map(|_| ()).map_err(|e| e.to_string()),
                        _ => engine.hotspot().disconnect().await.map_err(|e| e.to_string()),
                    };
                    if let Err(e) = result {
                        report(&format!("{action} failed"), e);
                    }
                    let _ = thread.queue(|mut bridge| {
                        bridge.as_mut().bump(Revision::Calls);
                        bridge.bump(Revision::Settings);
                    });
                });
                Ok(())
            }
            "signOut" => {
                let thread = self.qt_thread();
                self.runtime.spawn(async move {
                    if let Err(e) = engine.sign_out().await {
                        report("the sign-out failed", e);
                    }
                    let _ = thread.queue(|bridge| bridge.start());
                });
                Ok(())
            }
            other => Err(format!("unknown action {other}")),
        };
        if let Err(e) = done {
            report(&format!("{name} failed"), e);
        }
        if name.starts_with("settings.") || name.starts_with("camera.") || name.starts_with("phoneFiles.") {
            self.bump(Revision::Settings);
        } else if name == "calls.turnOn" {
            self.bump(Revision::Calls);
        }
    }
}

fn save_photo(engine: &Arc<Engine>, arg: &str) -> Result<(), String> {
    let request: serde_json::Value = serde_json::from_str(arg).map_err(|e| e.to_string())?;
    let id = request["id"].as_i64().ok_or("no photo id")?;
    let path = request["path"].as_str().ok_or("no path")?;
    let path = path.strip_prefix("file://").unwrap_or(path);
    let bytes =
        engine.clone().photos().full_image(id).map_err(|e| e.to_string())?.ok_or("the photo is not downloaded")?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Revision {
    Contacts,
    Photos,
    Calls,
    Apps,
    Media,
    Transfers,
    Settings,
    Wallpaper,
    Mirroring,
}

impl Revision {
    const ALL: [Self; 9] = [
        Self::Contacts,
        Self::Photos,
        Self::Calls,
        Self::Apps,
        Self::Media,
        Self::Transfers,
        Self::Settings,
        Self::Wallpaper,
        Self::Mirroring,
    ];
}

impl EngineBridge {
    fn reload_notifications(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let json = match engine.notifications().list() {
            Ok(list) => notifications_json(&list),
            Err(e) => {
                report("the notification list was not read", e);
                return;
            }
        };
        self.as_mut().set_notifications_json(QString::from(json.as_str()));
    }

    fn reload_conversations(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let json = match engine.messaging().conversations() {
            Ok(list) => conversations_json(&list),
            Err(e) => {
                report("the conversations were not read", e);
                return;
            }
        };
        self.as_mut().set_conversations_json(QString::from(json.as_str()));
    }

    fn reload_messages(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let thread_id = self.open_thread_id;
        if thread_id == NO_THREAD {
            self.as_mut().set_messages_json(QString::from("[]"));
            return;
        }
        let json = match engine.messaging().messages(thread_id) {
            Ok(list) => messages_json(&list),
            Err(e) => {
                report("the messages were not read", e);
                return;
            }
        };
        self.as_mut().set_messages_json(QString::from(json.as_str()));
    }

    pub(super) fn reload_status(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let json = match engine.device_status().status() {
            Ok(status) => status_json(&status),
            Err(_) => "{}".to_owned(),
        };
        self.as_mut().set_status_json(QString::from(json.as_str()));
    }

    fn reload_notification_state(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let muted = engine.notifications().state().is_ok_and(|state| state.muted_all_until_ms.is_some());
        self.as_mut().set_notifications_muted(muted);
    }

    pub(super) fn notifications_changed(self: Pin<&mut Self>, change: NotificationsChange) {
        match change {
            NotificationsChange::ListChanged => self.reload_notifications(),
            NotificationsChange::StateChanged | NotificationsChange::SettingsChanged => {
                self.reload_notification_state()
            }
            NotificationsChange::ActionFinished { .. } | NotificationsChange::OpenRequested { .. } => {}
        }
    }

    pub fn set_notification_pinned(&self, key: &QString, pinned: bool) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.notifications().set_pinned(key.to_string(), pinned) {
            report("the notification was not pinned", e);
        }
    }

    pub fn mute_notifications(mut self: Pin<&mut Self>, muted: bool) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.notifications().set_mute_all(muted) {
            report("the notification banners were not muted", e);
        }
        self.as_mut().reload_notification_state();
    }

    pub fn toggle_do_not_disturb(&self) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.device_status().toggle_do_not_disturb() {
            report("Do Not Disturb was not changed", e);
        }
    }

    pub fn cycle_ringer(&self) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.device_status().cycle_ringer_mode() {
            report("the ringer was not changed", e);
        }
    }

    pub fn find_phone(&self) {
        let Some(engine) = self.engine.clone() else { return };
        let status = engine.device_status();
        let ringing = status.status().ok().and_then(|s| s.ring_my_phone).is_some_and(|ring| ring.can_stop);
        let result = if ringing { status.stop_ringing() } else { status.ring_my_phone() };
        if let Err(e) = result {
            report("the phone was not rung", e);
        }
    }

    pub(super) fn messaging_changed(mut self: Pin<&mut Self>, change: MessagingChange) {
        match change {
            MessagingChange::ContentChanged { threads } => {
                self.as_mut().reload_conversations();
                if threads.contains(&self.open_thread_id) {
                    self.reload_messages();
                }
            }
            MessagingChange::SendChanged { .. } => self.reload_messages(),
            MessagingChange::OpenConversation { thread_id } => self.open_conversation(thread_id),
            MessagingChange::StateChanged | MessagingChange::PermissionChanged { .. } => {}
        }
    }

    pub fn dismiss_notification(&self, key: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        let key = key.to_string();
        self.runtime.spawn(async move {
            if let Err(e) = engine.notifications().dismiss(key).await {
                report("the notification was not dismissed", e);
            }
        });
    }

    pub fn clear_notifications(&self) {
        let Some(engine) = self.engine.clone() else { return };
        self.runtime.spawn(async move {
            if let Err(e) = engine.notifications().clear_all().await {
                report("the notifications were not cleared", e);
            }
        });
    }

    pub fn press_notification_action(&self, key: &QString, index: i32) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.notifications().press_action(key.to_string(), index) {
            report("the notification action was not sent", e);
        }
    }

    pub fn reply_to_notification(&self, key: &QString, index: i32, text: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        if let Err(e) = engine.notifications().reply(key.to_string(), index, text.to_string()) {
            report("the reply was not sent", e);
        }
    }

    pub fn open_conversation(mut self: Pin<&mut Self>, thread_id: i64) {
        let Some(engine) = self.engine.clone() else { return };
        self.as_mut().set_open_thread_id(thread_id);
        let messaging = engine.messaging();
        if let Err(e) =
            messaging.messages_view_changed(true).and_then(|()| messaging.conversation_selected(Some(thread_id)))
        {
            report("the open conversation was not reported", e);
        }
        self.reload_messages();
    }

    pub fn close_conversation(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        self.as_mut().set_open_thread_id(NO_THREAD);
        if let Err(e) = engine.messaging().conversation_selected(None) {
            report("the closed conversation was not reported", e);
        }
        self.reload_messages();
    }

    pub fn send_message(&self, text: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        let thread_id = self.open_thread_id;
        if thread_id == NO_THREAD {
            return;
        }
        let text = text.to_string();
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.messaging().send(MessageTarget::Conversation { thread_id }, text, Vec::new()).await;
            if let Err(e) = result {
                report("the message was not sent", e);
            }
            let _ = thread.queue(|bridge| bridge.reload_messages());
        });
    }
}
