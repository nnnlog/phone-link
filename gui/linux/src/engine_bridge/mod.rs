//! The bridge between the QML window and the engine: the `EngineBridge` singleton's state, and the
//! modules that add its invokables and handlers.
//!
//! The engine's start and the window's phase are in `lifecycle`, the engine built with this
//! computer's edges in `engine`, the linked phones in `phones`, the engine's events in `events` and
//! the sign-in page in `web_sign_in`. Each part of the app (`area_*`, `mirroring`) adds the
//! invokables and handlers of its own, and `features` routes `query` and `perform` names to them.

use std::sync::Arc;

use cxx_qt_lib::QString;
use phonelink_engine::api::engine::Engine;

use crate::mirror::Keyboard;
use crate::notifier::FreedesktopNotifier;
use crate::sign_in::SignInPage;

// cxx-qt registers every bridge of the QML module from one folder, `src/bridges/`, so each module that
// owns a bridge includes it from there.
#[expect(unsafe_code, reason = "a cxx-qt bridge declares functions that cross the C++ boundary")]
#[path = "../bridges/engine_bridge.rs"]
mod bridge;

mod area_calls;
mod area_camera;
mod area_files;
mod area_messages;
mod area_notifications;
mod area_photos;
mod area_playback;
mod area_shell;
mod engine;
mod events;
mod features;
mod lifecycle;
mod mirroring;
mod phones;
mod web_sign_in;

use bridge::qobject;
use engine::data_home;
use qobject::{EngineBridge, QObject};

/// The Rust side of the `EngineBridge` singleton: its properties and what the bridge keeps.
pub struct EngineBridgeRust {
    phase: QString,
    message: QString,
    message_detail: QString,
    sign_in_url: QString,
    sign_in_user_agent: QString,
    sign_in_script: QString,
    phones_json: QString,
    phone_name: QString,
    notifications_json: QString,
    conversations_json: QString,
    messages_json: QString,
    status_json: QString,
    notifications_muted: bool,
    phone_connected: bool,
    features_disabled: bool,
    contacts_revision: i32,
    photos_revision: i32,
    calls_revision: i32,
    apps_revision: i32,
    media_revision: i32,
    transfers_revision: i32,
    settings_revision: i32,
    wallpaper_revision: i32,
    mirroring_revision: i32,
    pairing_revision: i32,
    messaging_revision: i32,
    mirror_keyboard: Keyboard,
    phone_mount: Option<fuser::BackgroundSession>,
    runtime: tokio::runtime::Runtime,
    engine: Option<Arc<Engine>>,
    notifier: Option<Arc<FreedesktopNotifier>>,
    sign_in: Option<SignInPage>,
    /// The phase a sign-in page interrupted while no start was under way (the engine asked for a sign-in
    /// or consent during the session): the window returns to it when the page is done.
    phase_before_sign_in: Option<QString>,
    starting: bool,
    /// Whether the main window is the active window (the messages' mark-read rules need it).
    window_focused: bool,
    /// What each part of the app keeps between calls.
    areas: Areas,
}

/// The state of each part of the app, which its `area_*` module (or `mirroring`) defines.
#[derive(Default)]
struct Areas {
    messages: area_messages::State,
    notifications: area_notifications::State,
    calls: area_calls::State,
    photos: area_photos::State,
    files: area_files::State,
    playback: area_playback::State,
    camera: area_camera::State,
    mirror: mirroring::State,
    shell: area_shell::State,
}

impl Default for EngineBridgeRust {
    fn default() -> Self {
        Self {
            phase: QString::from("idle"),
            message: QString::default(),
            message_detail: QString::default(),
            sign_in_url: QString::default(),
            sign_in_user_agent: QString::default(),
            sign_in_script: QString::default(),
            phones_json: QString::from("[]"),
            phone_name: QString::default(),
            notifications_json: QString::from("[]"),
            conversations_json: QString::from("[]"),
            messages_json: QString::from("[]"),
            status_json: QString::from("{}"),
            notifications_muted: false,
            phone_connected: false,
            features_disabled: false,
            contacts_revision: 0,
            photos_revision: 0,
            calls_revision: 0,
            apps_revision: 0,
            media_revision: 0,
            transfers_revision: 0,
            settings_revision: 0,
            wallpaper_revision: 0,
            mirroring_revision: 0,
            pairing_revision: 0,
            messaging_revision: 0,
            mirror_keyboard: Keyboard::default(),
            phone_mount: None,
            runtime: tokio::runtime::Runtime::new().expect("the engine's tokio runtime starts"),
            engine: None,
            notifier: None,
            sign_in: None,
            phase_before_sign_in: None,
            starting: false,
            window_focused: false,
            areas: Areas::default(),
        }
    }
}
