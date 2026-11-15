mod application;
mod audio;
mod bluetooth;
mod camera;
mod catalog;
mod clipboard;
mod edges;
mod engine_bridge;
mod images;
mod l10n;
mod mirror;
mod mirror_video;
mod notifier;
mod phone_files;
mod sign_in;
mod single_instance;
mod system;
mod views;
mod wifi;

use cxx_qt_lib::{QQmlApplicationEngine, QUrl};
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let single_instance::Claim::First(_instance) = single_instance::claim() else { return };

    application::create_application(&std::env::args().collect());
    let mut engine = QQmlApplicationEngine::new();

    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/PhoneLink/qml/Main.qml"));
    }

    application::exec_application();
}
