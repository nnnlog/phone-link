//! Phone Link for Linux: a Qt Quick app over the engine, with the Linux desktop's services as the
//! engine's OS edges.

mod app_log;
// cxx-qt registers every bridge of the QML module from one folder (see `engine_bridge`).
#[expect(unsafe_code, reason = "a cxx-qt bridge declares functions that cross the C++ boundary")]
#[path = "bridges/application.rs"]
mod application;
mod apps_host;
mod audio;
mod autostart;
mod bluetooth;
mod camera;
mod clipboard;
mod dbus;
mod edges;
mod engine_bridge;
mod image_io;
mod images;
mod l10n;
mod messaging_edges;
mod mirror;
mod mirror_video;
mod mpris;
mod notifier;
mod pc_status;
mod pc_wallpaper;
mod phone_files;
mod sign_in;
mod single_instance;
mod system;
mod user_text;
mod views;
mod wifi;

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};

fn main() {
    app_log::install();

    let single_instance::Claim::First(_instance) = single_instance::claim() else { return };
    app_log::launched();

    application::create_application(&std::env::args().collect());
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/PhoneLink/qml/Main.qml"));
    }

    application::exec_application();
    // The QML engine (and the bridge it owns) goes before the application object, and both before main
    // returns: Qt's own statics are gone once the program's static destructors run.
    drop(engine);
    application::destroy_application();
}
