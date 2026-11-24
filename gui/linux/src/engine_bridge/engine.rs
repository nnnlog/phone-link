//! The engine, built with this computer's edges (desktop notifications, the clipboard, `BlueZ`, a
//! call's voice, `NetworkManager`, the camera, the mirroring presenter), and the folder it keeps the
//! phone's data in.

use std::path::PathBuf;
use std::sync::Arc;

use cxx_qt::CxxQtThread;
use phonelink_bluetooth::PhoneBluetooth;
use phonelink_calling::control::voice::PhoneVoice;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::events::EventSink;
use phonelink_engine::engine::{FeatureEdges, StoreLocation, SystemNotifier};
use phonelink_messaging::PhoneProximity;
use tracing::warn;

use super::EngineBridge;
use super::features::Revision;
use crate::bluetooth::{BluezBluetooth, NoBluetoothProximity};
use crate::camera::camera;
use crate::clipboard::{ImageCrateCodec, system_pasteboard};
use crate::edges::{LinuxHost, SecretServiceStore, StderrNotifier};
use crate::messaging_edges::{ImageCrateImaging, file_types as messaging_file_types};
use crate::mirror::presenter;
use crate::notifier::FreedesktopNotifier;
use crate::pc_status::DesktopPcStatus;
use crate::pc_wallpaper::DesktopWallpaper;
use crate::system::{Desktop, DesktopLock, DesktopPowerControl, NetworkManagerReachability, watch_system_events};
use crate::voice::BluezVoice;
use crate::wifi::NetworkManagerWifi;

/// The user's data folder: `$XDG_DATA_HOME`, else `~/.local/share`.
///
/// A relative `XDG_DATA_HOME` is ignored, as the XDG base directory rules say; with no absolute folder
/// there is none.
pub(super) fn data_home() -> Result<PathBuf, &'static str> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share"))
                .filter(|dir| dir.is_absolute())
        })
        .ok_or("neither XDG_DATA_HOME nor HOME names an absolute folder")
}

/// The folder the engine keeps the phone's data in: `phonelink` in [`data_home`].
///
/// There is none without a data folder, or with a path the engine cannot take as text.
pub(super) fn data_dir() -> Result<String, &'static str> {
    data_home()?.join("phonelink").into_os_string().into_string().map_err(|_| "the data folder's path is not UTF-8")
}

/// The engine with this computer's edges, and the desktop notifier when the session bus is there.
pub(super) fn build_engine(
    data_dir: String,
    events: Arc<dyn EventSink>,
    thread: CxxQtThread<EngineBridge>,
) -> (Arc<Engine>, Option<Arc<FreedesktopNotifier>>) {
    // Only a missing session bus falls back to stderr: a notification server that is not up yet is
    // looked for again with each notification.
    let desktop_notifier = FreedesktopNotifier::connect()
        .map(Arc::new)
        .inspect_err(|e| warn!(error = %e, "no session bus for desktop notifications; they go to stderr"))
        .ok();
    let notifier: Arc<dyn SystemNotifier> = match &desktop_notifier {
        Some(desktop) => desktop.clone(),
        None => Arc::new(StderrNotifier),
    };

    let FeatureEdges {
        stores,
        notifier,
        pasteboard: memory_pasteboard,
        image_codec: _,
        pc_status: _,
        attachment_imaging: _,
        file_types: _,
        proximity: _,
        pc_power: _,
        pc_wallpaper: _,
        phone_bluetooth: no_bluetooth,
        phone_voice: no_voice,
        hotspot_wifi: _,
        phone_camera: _,
        phone_apps_host: _,
        mirror_presenter: _,
    } = FeatureEdges::stand_ins(StoreLocation::Directory(PathBuf::from(data_dir)), notifier);
    let pasteboard = system_pasteboard().unwrap_or_else(|e| {
        warn!(error = %e, "no system clipboard; clipboard sharing stays local");
        memory_pasteboard
    });

    let image_codec = Arc::new(ImageCrateCodec);
    let attachment_imaging = Arc::new(ImageCrateImaging);
    let file_types = messaging_file_types().clone();
    let desktop = Desktop::detect();
    let pc_power = Arc::new(DesktopPowerControl::new(desktop));
    let pc_status = Arc::new(DesktopPcStatus::new());
    let pc_wallpaper = Arc::new(DesktopWallpaper::detect());
    let (phone_bluetooth, proximity, phone_voice): (
        Arc<dyn PhoneBluetooth>,
        Arc<dyn PhoneProximity>,
        Arc<dyn PhoneVoice>,
    ) = match BluezBluetooth::connect() {
        Ok(bluez) => {
            let bluez = Arc::new(bluez);
            (bluez.clone(), bluez.clone(), Arc::new(BluezVoice::new(bluez)))
        }
        Err(e) => {
            warn!(error = %e, "no BlueZ; calls, instant hotspot and the messages' Bluetooth check are unavailable");
            (no_bluetooth, Arc::new(NoBluetoothProximity), no_voice)
        }
    };

    let hotspot_wifi = Arc::new(NetworkManagerWifi::new());
    // The hotspot's "joined" is read again when the computer's Wi-Fi network changes.
    let settings = thread.clone();
    hotspot_wifi.watch_network(move || {
        let _ = settings.queue(|bridge| bridge.bump(Revision::Settings));
    });
    crate::bluetooth::on_pairing_question(move |code| {
        let _ = thread.queue(move |bridge| bridge.bluetooth_pairing_question(code));
    });

    let feature_edges = FeatureEdges {
        stores,
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
        phone_voice,
        hotspot_wifi,
        phone_camera: camera().clone(),
        phone_apps_host: Arc::new(crate::apps_host::LinuxAppsHost::for_session()),
        mirror_presenter: presenter().clone(),
    };
    let engine = Engine::production(
        Arc::new(SecretServiceStore::default()),
        Arc::new(DesktopLock::new(desktop)),
        Arc::new(LinuxHost),
        Arc::new(NetworkManagerReachability::new()),
        events,
        feature_edges,
    );
    if let Some(desktop) = &desktop_notifier {
        desktop.deliver_responses_to(&engine);
    }
    watch_system_events(engine.clone(), desktop);
    (engine, desktop_notifier)
}
