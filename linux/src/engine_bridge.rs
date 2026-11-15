use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtThread, CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::engine::{Engine, EngineStatus};
use phonelink_engine::api::events::{EngineEvent, EventSink};
use phonelink_engine::engine::{EngineError, FeatureEdges, StoreLocation, SystemNotifier};
use serde_json::{Value, json};

use crate::bluetooth::BluezBluetooth;
use crate::camera::camera;
use crate::clipboard::{ImageCrateCodec, system_pasteboard};
use crate::edges::{LinuxHost, SecretServiceStore, StderrNotifier};
use crate::mirror::{Keyboard, presenter};
use crate::notifier::FreedesktopNotifier;
use crate::sign_in::{SignInPage, Step, page_script};
use crate::system::{Desktop, DesktopLock, DesktopPowerControl, NetworkManagerReachability, watch_system_events};
use crate::wifi::NetworkManagerWifi;
use features::Revision;
use phonelink_bluetooth::PhoneBluetooth;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, phase)]
        #[qproperty(QString, message)]
        #[qproperty(QString, sign_in_url, cxx_name = "signInUrl")]
        #[qproperty(QString, sign_in_user_agent, cxx_name = "signInUserAgent")]
        #[qproperty(QString, sign_in_script, cxx_name = "signInScript")]
        #[qproperty(QString, phones_json, cxx_name = "phonesJson")]
        #[qproperty(QString, phone_name, cxx_name = "phoneName")]
        #[qproperty(QString, notifications_json, cxx_name = "notificationsJson")]
        #[qproperty(QString, conversations_json, cxx_name = "conversationsJson")]
        #[qproperty(QString, messages_json, cxx_name = "messagesJson")]
        #[qproperty(i64, open_thread_id, cxx_name = "openThreadId")]
        #[qproperty(QString, status_json, cxx_name = "statusJson")]
        #[qproperty(bool, notifications_muted, cxx_name = "notificationsMuted")]
        #[qproperty(bool, phone_connected, cxx_name = "phoneConnected")]
        #[qproperty(i32, contacts_revision, cxx_name = "contactsRevision")]
        #[qproperty(i32, photos_revision, cxx_name = "photosRevision")]
        #[qproperty(i32, calls_revision, cxx_name = "callsRevision")]
        #[qproperty(i32, apps_revision, cxx_name = "appsRevision")]
        #[qproperty(i32, media_revision, cxx_name = "mediaRevision")]
        #[qproperty(i32, transfers_revision, cxx_name = "transfersRevision")]
        #[qproperty(i32, settings_revision, cxx_name = "settingsRevision")]
        #[qproperty(i32, wallpaper_revision, cxx_name = "wallpaperRevision")]
        #[qproperty(i32, mirroring_revision, cxx_name = "mirroringRevision")]
        type EngineBridge = super::EngineBridgeRust;

        #[qsignal]
        #[cxx_name = "mirrorRaise"]
        fn mirror_raise(self: Pin<&mut EngineBridge>, session_id: QString);

        #[qsignal]
        #[cxx_name = "showSection"]
        fn show_section(self: Pin<&mut EngineBridge>, section: QString);

        #[qsignal]
        #[cxx_name = "mirrorDrag"]
        fn mirror_drag(self: Pin<&mut EngineBridge>, session_id: QString, state: QString);

        #[qsignal]
        #[cxx_name = "mirrorPhoneDrag"]
        fn mirror_phone_drag(self: Pin<&mut EngineBridge>, session_id: QString, urls: QString);

        #[qinvokable]
        fn query(self: &EngineBridge, name: &QString, arg: &QString) -> QString;

        #[qinvokable]
        fn perform(self: Pin<&mut EngineBridge>, name: &QString, arg: &QString);

        #[qinvokable]
        #[cxx_name = "setNotificationPinned"]
        fn set_notification_pinned(self: &EngineBridge, key: &QString, pinned: bool);

        #[qinvokable]
        #[cxx_name = "muteNotifications"]
        fn mute_notifications(self: Pin<&mut EngineBridge>, muted: bool);

        #[qinvokable]
        #[cxx_name = "toggleDoNotDisturb"]
        fn toggle_do_not_disturb(self: &EngineBridge);

        #[qinvokable]
        #[cxx_name = "cycleRinger"]
        fn cycle_ringer(self: &EngineBridge);

        #[qinvokable]
        #[cxx_name = "findPhone"]
        fn find_phone(self: &EngineBridge);

        #[qinvokable]
        #[cxx_name = "dismissNotification"]
        fn dismiss_notification(self: &EngineBridge, key: &QString);

        #[qinvokable]
        #[cxx_name = "clearNotifications"]
        fn clear_notifications(self: &EngineBridge);

        #[qinvokable]
        #[cxx_name = "pressNotificationAction"]
        fn press_notification_action(self: &EngineBridge, key: &QString, index: i32);

        #[qinvokable]
        #[cxx_name = "replyToNotification"]
        fn reply_to_notification(self: &EngineBridge, key: &QString, index: i32, text: &QString);

        #[qinvokable]
        #[cxx_name = "openConversation"]
        fn open_conversation(self: Pin<&mut EngineBridge>, thread_id: i64);

        #[qinvokable]
        #[cxx_name = "closeConversation"]
        fn close_conversation(self: Pin<&mut EngineBridge>);

        #[qinvokable]
        #[cxx_name = "sendMessage"]
        fn send_message(self: &EngineBridge, text: &QString);

        #[qinvokable]
        fn start(self: Pin<&mut Self>);

        #[qinvokable]
        #[cxx_name = "selectPhone"]
        fn select_phone(self: Pin<&mut Self>, id: &QString);

        #[qinvokable]
        #[cxx_name = "signInConsoleMessage"]
        fn sign_in_console_message(self: Pin<&mut Self>, message: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "signInPageLoaded"]
        fn sign_in_page_loaded(self: Pin<&mut Self>, url: &QString) -> QString;

        #[qinvokable]
        #[cxx_name = "signInScriptResult"]
        fn sign_in_script_result(self: Pin<&mut Self>, result: &QString);

        #[qinvokable]
        #[cxx_name = "cancelSignIn"]
        fn cancel_sign_in(self: Pin<&mut Self>, declined: bool);
    }

    impl cxx_qt::Threading for EngineBridge {}
}

mod features;
mod mirroring;

use qobject::EngineBridge;

pub const NO_THREAD: i64 = -1;

pub struct EngineBridgeRust {
    phase: QString,
    message: QString,
    sign_in_url: QString,
    sign_in_user_agent: QString,
    sign_in_script: QString,
    phones_json: QString,
    phone_name: QString,
    notifications_json: QString,
    conversations_json: QString,
    messages_json: QString,
    open_thread_id: i64,
    status_json: QString,
    notifications_muted: bool,
    phone_connected: bool,
    contacts_revision: i32,
    photos_revision: i32,
    calls_revision: i32,
    apps_revision: i32,
    media_revision: i32,
    transfers_revision: i32,
    settings_revision: i32,
    wallpaper_revision: i32,
    mirroring_revision: i32,
    mirror_keyboard: Keyboard,
    phone_mount: Option<fuser::BackgroundSession>,
    runtime: tokio::runtime::Runtime,
    engine: Option<Arc<Engine>>,
    sign_in: Option<SignInPage>,
    starting: bool,
}

impl Default for EngineBridgeRust {
    fn default() -> Self {
        Self {
            phase: QString::from("idle"),
            message: QString::default(),
            sign_in_url: QString::default(),
            sign_in_user_agent: QString::default(),
            sign_in_script: QString::default(),
            phones_json: QString::from("[]"),
            phone_name: QString::default(),
            notifications_json: QString::from("[]"),
            conversations_json: QString::from("[]"),
            messages_json: QString::from("[]"),
            open_thread_id: NO_THREAD,
            status_json: QString::from("{}"),
            notifications_muted: false,
            phone_connected: false,
            contacts_revision: 0,
            photos_revision: 0,
            calls_revision: 0,
            apps_revision: 0,
            media_revision: 0,
            transfers_revision: 0,
            settings_revision: 0,
            wallpaper_revision: 0,
            mirroring_revision: 0,
            mirror_keyboard: Keyboard::default(),
            phone_mount: None,
            runtime: tokio::runtime::Runtime::new().expect("the engine's tokio runtime starts"),
            engine: None,
            sign_in: None,
            starting: false,
        }
    }
}

struct QtEventSink {
    thread: CxxQtThread<EngineBridge>,
}

impl EventSink for QtEventSink {
    fn on_event(&self, event: EngineEvent) {
        if self.thread.queue(move |bridge| bridge.handle_event(event)).is_err() {
            eprintln!("an engine event arrived after the window closed");
        }
    }
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_default()
        .join("phonelink")
}

fn build_engine(events: Arc<dyn EventSink>) -> Arc<Engine> {
    let data_dir = data_dir();
    let desktop_notifier = FreedesktopNotifier::connect()
        .map(Arc::new)
        .inspect_err(|e| eprintln!("no desktop notifications; they go to stderr: {e}"))
        .ok();
    let notifier: Arc<dyn SystemNotifier> = match &desktop_notifier {
        Some(desktop) => desktop.clone(),
        None => Arc::new(StderrNotifier),
    };
    let FeatureEdges {
        stores: _,
        notifier,
        pasteboard: memory_pasteboard,
        image_codec: _,
        pc_status,
        attachment_imaging,
        file_types,
        proximity,
        pc_power: _,
        pc_wallpaper,
        phone_bluetooth: no_bluetooth,
        hotspot_wifi: _,
        phone_camera: _,
        phone_apps_host,
        mirror_presenter: _,
    } = FeatureEdges::stand_ins(StoreLocation::Directory(data_dir.clone()), notifier);
    let pasteboard = system_pasteboard().unwrap_or_else(|e| {
        eprintln!("no system clipboard; clipboard sharing stays local: {e}");
        memory_pasteboard
    });
    let image_codec = Arc::new(ImageCrateCodec);
    let desktop = Desktop::detect();
    let pc_power = Arc::new(DesktopPowerControl::new(desktop));
    let phone_bluetooth: Arc<dyn PhoneBluetooth> = match BluezBluetooth::connect() {
        Ok(bluez) => Arc::new(bluez),
        Err(e) => {
            eprintln!("no BlueZ; calls and instant hotspot are unavailable: {e}");
            no_bluetooth
        }
    };
    let hotspot_wifi = Arc::new(NetworkManagerWifi::new());

    let engine = Engine::new(
        Arc::new(SecretServiceStore::default()),
        Arc::new(DesktopLock::new(desktop)),
        Arc::new(LinuxHost),
        Arc::new(NetworkManagerReachability::new()),
        events,
        notifier,
        pasteboard,
        image_codec,
        pc_status,
        attachment_imaging,
        file_types,
        proximity,
        pc_power,
        pc_wallpaper,
        phone_bluetooth,
        hotspot_wifi,
        camera().clone(),
        phone_apps_host,
        presenter().clone(),
        data_dir.to_string_lossy().into_owned(),
    );
    if let Some(desktop) = desktop_notifier {
        desktop.deliver_responses_to(&engine);
    }
    watch_system_events(engine.clone(), desktop);
    engine
}

impl EngineBridge {
    pub fn start(mut self: Pin<&mut Self>) {
        if self.starting {
            return;
        }
        let thread = self.qt_thread();
        crate::single_instance::on_show(move || {
            let _ = thread.queue(|bridge| bridge.show_section(QString::default()));
        });
        let engine = match self.engine.clone() {
            Some(engine) => engine,
            None => {
                let engine = build_engine(Arc::new(QtEventSink { thread: self.qt_thread() }));
                self.as_mut().rust_mut().engine = Some(engine.clone());
                engine
            }
        };

        self.as_mut().rust_mut().starting = true;
        self.as_mut().set_phase(QString::from("starting"));
        self.as_mut().set_message(QString::default());

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.start().await;
            let _ = thread.queue(move |bridge| bridge.started(result));
        });
    }

    fn started(mut self: Pin<&mut Self>, result: Result<EngineStatus, EngineError>) {
        self.as_mut().rust_mut().starting = false;
        self.as_mut().rust_mut().sign_in = None;
        match result {
            Ok(status) if status.needs_phone_selection => {
                self.as_mut().reload_phones();
                self.set_phase(QString::from("selectingPhone"));
            }
            Ok(_) => self.running(),
            Err(_) if self.phase == QString::from("accountRemoved") => {}
            Err(e) => self.failed(&e.to_string()),
        }
    }

    fn running(mut self: Pin<&mut Self>) {
        self.as_mut().reload_phone();
        self.as_mut().reload_phones();
        self.as_mut().reload_features();
        self.as_mut().sync_phone_mount();
        self.as_mut().watch_camera_readers();
        self.set_phase(QString::from("running"));
    }

    fn watch_camera_readers(self: Pin<&mut Self>) {
        static WATCHING: std::sync::Once = std::sync::Once::new();
        let Some(engine) = self.engine.clone() else { return };
        let thread = self.qt_thread();
        WATCHING.call_once(move || {
            std::thread::spawn(move || {
                let mut was_reading = false;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let Some(device) = camera().published() else { continue };
                    let reading = camera().reader().is_some();
                    if reading == std::mem::replace(&mut was_reading, reading) || reading == camera().in_use() {
                        continue;
                    }
                    camera().set_in_use(reading);
                    if let Err(e) = engine.clone().connected_camera().client_changed(device, reading) {
                        eprintln!("the camera's reader change was not sent: {e}");
                    }
                    let _ = thread.queue(|bridge| bridge.bump(Revision::Settings));
                }
            });
        });
    }

    fn sync_phone_mount(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let wanted = engine
            .clone()
            .phone_files()
            .state()
            .ok()
            .filter(|state| state.capable && state.enabled && state.running && state.permission_granted);
        match (wanted, self.phone_mount.is_some()) {
            (Some(state), false) => {
                let handle = self.runtime.handle().clone();
                self.as_mut().rust_mut().phone_mount = crate::phone_files::mount(engine, handle, state.domain);
            }
            (None, true) => self.as_mut().rust_mut().phone_mount = None,
            _ => {}
        }
    }

    fn failed(mut self: Pin<&mut Self>, message: &str) {
        self.as_mut().set_message(QString::from(message));
        self.set_phase(QString::from("failed"));
    }

    fn reload_phones(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let phones = engine.phone().phones().unwrap_or_default();
        let json: Vec<Value> = phones
            .into_iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "model": p.model, "selected": p.selected }))
            .collect();
        self.as_mut().set_phones_json(QString::from(Value::Array(json).to_string().as_str()));
    }

    fn reload_phone(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let info = engine.phone().info().ok();
        let name = info.as_ref().map(|info| info.name.clone()).unwrap_or_default();
        self.as_mut().set_phone_name(QString::from(name.as_str()));
        self.as_mut().set_phone_connected(info.is_some_and(|info| info.connected));
        self.reload_status();
    }

    pub fn select_phone(mut self: Pin<&mut Self>, id: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        if self.starting {
            return;
        }
        self.as_mut().rust_mut().starting = true;
        self.as_mut().set_phase(QString::from("starting"));

        let id = id.to_string();
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.phone().select(id).await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().starting = false;
                match result {
                    Ok(()) => bridge.running(),
                    Err(e) => bridge.failed(&e.to_string()),
                }
            });
        });
    }

    fn handle_event(mut self: Pin<&mut Self>, event: EngineEvent) {
        match event {
            EngineEvent::WebSignInRequested { request_id, request } => {
                self.as_mut().rust_mut().sign_in = Some(SignInPage::new(request_id, request.end_url_prefix));
                self.as_mut().set_sign_in_url(QString::from(request.url.as_str()));
                self.as_mut().set_sign_in_user_agent(QString::from(request.user_agent.as_str()));
                self.as_mut().set_sign_in_script(QString::from(page_script(&request.bridge_script).as_str()));
                self.set_phase(QString::from("signIn"));
            }
            EngineEvent::PhoneSession { .. } | EngineEvent::PhoneChanged => {
                self.as_mut().reload_phone();
                self.bump(Revision::Mirroring);
            }
            EngineEvent::ScreenMirroring { change } => self.mirroring_changed(change),
            EngineEvent::PhoneFiles { .. } => {
                self.as_mut().sync_phone_mount();
                self.bump(Revision::Settings);
            }
            EngineEvent::DeviceStatus { .. } => {
                self.as_mut().reload_status();
                self.bump(Revision::Settings);
            }
            EngineEvent::Notifications { change } => self.notifications_changed(change),
            EngineEvent::Messaging { change } => self.messaging_changed(change),
            EngineEvent::Contacts { .. } => self.bump(Revision::Contacts),
            EngineEvent::Calling { .. } => self.bump(Revision::Calls),
            EngineEvent::Photos { .. } => self.bump(Revision::Photos),
            EngineEvent::PhoneApps { .. } => self.bump(Revision::Apps),
            EngineEvent::Audio { .. } => self.bump(Revision::Media),
            EngineEvent::FileTransfer { .. } => self.bump(Revision::Transfers),
            EngineEvent::Clipboard { .. } | EngineEvent::RemoteControl { .. } | EngineEvent::ConnectedCamera { .. } => {
                self.bump(Revision::Settings)
            }
            EngineEvent::Wallpaper { .. } => self.bump(Revision::Wallpaper),
            EngineEvent::Resync { .. } => self.reload_features(),
            EngineEvent::AccountRemoved => self.set_phase(QString::from("accountRemoved")),
            _ => {}
        }
    }

    fn apply(mut self: Pin<&mut Self>, step: Step) -> QString {
        match step {
            Step::Nothing => QString::default(),
            Step::RunScript(script) => QString::from(script.as_str()),
            Step::Complete(json) => {
                if let (Some(engine), Some(page)) = (self.engine.clone(), self.as_mut().rust_mut().sign_in.take()) {
                    engine.complete_web_sign_in(page.request_id, json);
                }
                let _ = self.qt_thread().queue(|bridge| {
                    if bridge.sign_in.is_none() && bridge.phase == QString::from("signIn") {
                        bridge.set_phase(QString::from("starting"));
                    }
                });
                QString::default()
            }
        }
    }

    pub fn sign_in_console_message(mut self: Pin<&mut Self>, message: &QString) -> QString {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.console_message(&message.to_string()),
            None => Step::Nothing,
        };
        self.apply(step)
    }

    pub fn sign_in_page_loaded(mut self: Pin<&mut Self>, url: &QString) -> QString {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.page_loaded(&url.to_string()),
            None => Step::Nothing,
        };
        self.apply(step)
    }

    pub fn sign_in_script_result(mut self: Pin<&mut Self>, result: &QString) {
        let step = match self.as_mut().rust_mut().sign_in.as_mut() {
            Some(page) => page.script_result(&result.to_string()),
            None => Step::Nothing,
        };
        self.apply(step);
    }

    pub fn cancel_sign_in(mut self: Pin<&mut Self>, declined: bool) {
        if let (Some(engine), Some(page)) = (self.engine.clone(), self.as_mut().rust_mut().sign_in.take()) {
            engine.cancel_web_sign_in(page.request_id, declined);
        }
    }
}
