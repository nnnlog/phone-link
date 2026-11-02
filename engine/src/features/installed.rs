//! The installed features and what the engine hands them: their configuration ([`FeaturesConfig`],
//! part of [`crate::config::EngineConfig`]) and their OS edges ([`FeatureEdges`]).
//!
//! The install order follows what the features read from one another: contacts before messaging
//! and calling (its [`super::ContactDirectory`]), messaging before notifications (its
//! [`super::MessagingAlerts`]), phone apps before notifications (its [`super::PhoneAppDirectory`]),
//! notifications before audio (its store holds the media sessions).

use crate::config::features::FeaturesConfig;
use std::sync::Arc;

use super::audio::Audio;
use super::bluetooth::{Bluetooth, PhoneBluetooth};
use super::calling::Calling;
use super::clipboard::{Clipboard, ClipboardImageCodec, ClipboardPasteboard};
use super::connected_camera::{CameraChannels, ConnectedCamera, PhoneCamera};
use super::contacts::Contacts;
use super::device_status::{DeviceStatus, PcStatusSource};
use super::file_transfer::FileTransfer;
use super::hotspot::{Hotspot, HotspotWifi, SideChannelAuth};
use super::messaging::{AttachmentImaging, Messaging, MessagingEdges, MessagingFileTypes, PhoneProximity};
use super::notifications::Notifications;
use super::phone_apps::{PhoneApps, PhoneAppsHost};
use super::phone_files::{FilesChannels, PhoneFiles};
use super::photos::Photos;
use super::remote_control::{DeviceListRefresh, PcPowerControl, RemoteControl};
use super::remote_snipping::RemoteSnipping;
use super::screen_mirroring::{MirrorConnector, MirrorPresenter, ScreenMirroring};
use super::shared::nano_channels::ShareChannels;
use super::wallpaper::{PcWallpaperSource, Wallpaper};
use super::{FeatureEnv, FeatureError, StoreLocation, SystemNotifier};
use crate::platform::PlatformRoutes;

/// The features' OS edges (the GUI implements them in Swift; the CLI and the tests use the
/// stand-ins of [`FeatureEdges::stand_ins`]) and where their stores live.
#[derive(Clone)]
pub struct FeatureEdges {
    pub stores: StoreLocation,
    pub notifier: Arc<dyn SystemNotifier>,
    pub pasteboard: Arc<dyn ClipboardPasteboard>,
    pub image_codec: Arc<dyn ClipboardImageCodec>,
    pub pc_status: Arc<dyn PcStatusSource>,
    pub attachment_imaging: Arc<dyn AttachmentImaging>,
    pub file_types: Arc<dyn MessagingFileTypes>,
    pub proximity: Arc<dyn PhoneProximity>,
    pub pc_power: Arc<dyn PcPowerControl>,
    pub pc_wallpaper: Arc<dyn PcWallpaperSource>,
    pub phone_bluetooth: Arc<dyn PhoneBluetooth>,
    pub hotspot_wifi: Arc<dyn HotspotWifi>,
    pub phone_camera: Arc<dyn PhoneCamera>,
    pub phone_apps_host: Arc<dyn PhoneAppsHost>,
    pub mirror_presenter: Arc<dyn MirrorPresenter>,
}

impl FeatureEdges {
    /// The in-memory stand-ins every feature tests with, stores at `stores`, and `notifier`: an
    /// empty pasteboard that never changes, a fake image codec, a desktop PC on Ethernet, image
    /// attachments sent as they are, fixed file types, no Bluetooth proximity, no desktop picture, PC
    /// power actions that are only recorded, no phone Bluetooth, no Wi-Fi for instant hotspot, no
    /// camera, no application that opens a recent app's document,
    /// nothing to present mirroring on.
    pub fn stand_ins(stores: StoreLocation, notifier: Arc<dyn SystemNotifier>) -> Self {
        use super::clipboard::image::standins::FakeCodec;
        use super::clipboard::pasteboard::standins::MemoryPasteboard;
        use super::device_status::edge::standins::{FixedPcStatus, PcState};
        use super::messaging::edge::standins::{FakeFileTypes, FixedProximity, PassThroughImaging};
        use super::remote_control::edge::standins::RecordingPowerControl;
        use super::wallpaper::PcWallpaper;
        use super::wallpaper::edge::standins::FixedPcWallpaper;

        Self {
            stores,
            notifier,
            pasteboard: MemoryPasteboard::new(),
            image_codec: FakeCodec::new(),
            pc_status: FixedPcStatus::new(PcState::default()),
            attachment_imaging: Arc::new(PassThroughImaging),
            file_types: Arc::new(FakeFileTypes),
            proximity: FixedProximity::new(super::messaging::ProximityState::PhoneNotPaired),
            pc_power: RecordingPowerControl::new(),
            pc_wallpaper: FixedPcWallpaper::new(PcWallpaper::NotSet),
            phone_bluetooth: Arc::new(super::bluetooth::NoBluetooth),
            hotspot_wifi: Arc::new(super::hotspot::NoWifi),
            phone_camera: Arc::new(super::connected_camera::NoCamera),
            phone_apps_host: Arc::new(super::phone_apps::PlainAppsHost),
            mirror_presenter: Arc::new(super::screen_mirroring::NoPresenter),
        }
    }
}

/// The installed features. Dropping a handle stops its tasks, so the engine keeps this for its
/// lifetime.
pub struct Features {
    pub audio: Arc<Audio>,
    pub calling: Arc<Calling>,
    pub clipboard: Arc<Clipboard>,
    pub connected_camera: Arc<ConnectedCamera>,
    pub contacts: Arc<Contacts>,
    pub device_status: Arc<DeviceStatus>,
    pub file_transfer: Arc<FileTransfer>,
    pub messaging: Arc<Messaging>,
    pub notifications: Arc<Notifications>,
    pub phone_apps: Arc<PhoneApps>,
    pub phone_files: Arc<PhoneFiles>,
    pub photos: Arc<Photos>,
    pub remote_control: Arc<RemoteControl>,
    pub wallpaper: Arc<Wallpaper>,
    pub remote_snipping: Arc<RemoteSnipping>,
    pub bluetooth: Arc<Bluetooth>,
    pub hotspot: Arc<Hotspot>,
    pub screen_mirroring: Arc<ScreenMirroring>,
}

/// The Cross Device side the CDEH features run on: the CDEH identity's environment (its connection and
/// the phones it shows), its platform routes, and the refresh of its device list.
#[derive(Clone, Copy)]
pub struct CdehSide<'a> {
    pub env: &'a FeatureEnv,
    pub routes: &'a PlatformRoutes,
    pub device_list: &'a Arc<dyn DeviceListRefresh>,
}

/// The connections the features open beyond the shared one, and the signing hotspot's side channel
/// uses.
pub struct FeatureConnections {
    pub side_channel_auth: Arc<dyn SideChannelAuth>,
    pub share_channels: Arc<dyn ShareChannels>,
    pub files_channels: Arc<dyn FilesChannels>,
    pub camera_channels: Arc<dyn CameraChannels>,
    pub mirror_connector: Arc<dyn MirrorConnector>,
}

impl Features {
    /// Installs every feature: the App-layer features on `env`; the CDEH features — the clipboard,
    /// the CDEH contracts of device status and wallpaper, and new-photo notifications — on
    /// `cdeh.env`'s connection and device list and `cdeh.routes`' MSAEP and DRM (the CDEH identity's
    /// stack, or the same stack as `env` when there is one identity). The Phone Link contracts of
    /// device status and wallpaper use `cdeh_env`'s App layer, which is `env`'s. Remote control has
    /// both: its processor on `env`'s App layer, its receiver on `cdeh_routes`' DRM, reading
    /// `cdeh_env`'s phones; `cdeh_device_list` refreshes the CDEH identity's device list. The phone's
    /// Bluetooth device runs on `env`'s App layer, for every feature that reaches the phone over
    /// Bluetooth; instant hotspot rides it, and signs and checks its side-channel JWTs with `auth`,
    /// the primary identity's crypto trust. File transfer runs on `cdeh_env` and `cdeh_routes` and opens
    /// its channels with `share_channels`; phone files runs there too and opens its channels with
    /// `files_channels`; the connected camera runs there too and opens its channels with
    /// `camera_channels`. Screen mirroring runs on `env`'s App layer and opens its own connection with
    /// `mirror_connector`.
    /// Call it once, inside the tokio runtime, after `PlatformRoutes::install` and before
    /// `AppLayer::start`.
    pub fn install(
        env: &FeatureEnv,
        cdeh: &CdehSide<'_>,
        config: &FeaturesConfig,
        edges: &FeatureEdges,
        connections: FeatureConnections,
    ) -> Result<Arc<Self>, FeatureError> {
        let CdehSide { env: cdeh_env, routes: cdeh_routes, device_list: cdeh_device_list } = *cdeh;
        let FeatureConnections {
            side_channel_auth: auth,
            share_channels,
            files_channels,
            camera_channels,
            mirror_connector,
        } = connections;

        let contacts = Contacts::install(env, config.contacts.clone())?;
        let messaging = install_messaging(env, config, edges, &contacts)?;
        let calling = Calling::install(env, config.calling.clone(), Some(contacts.directory()))?;
        let phone_apps = PhoneApps::install(env, config.phone_apps.clone(), edges.phone_apps_host.clone())?;
        let notifications = super::notifications::install(env, config.notifications.clone(), Some(messaging.alerts()))?;
        // The notification list and banners name and picture an app from the phone's app list.
        notifications.set_app_directory(phone_apps.directory())?;
        let audio = Audio::install(env, config.audio.clone(), notifications.clone())?;
        let photos = Photos::install(env, config.photos.clone())?;

        let device_status = install_device_status(cdeh_env, cdeh_routes, config, edges)?;
        let wallpaper = install_wallpaper(cdeh_env, cdeh_routes, config, edges)?;
        let clipboard = install_clipboard(cdeh_env, cdeh_routes, config, edges)?;
        let remote_control = RemoteControl::install(
            env,
            cdeh_env,
            config.remote_control.clone(),
            cdeh_routes.drm.clone(),
            edges.pc_power.clone(),
            cdeh_device_list.clone(),
        )?;
        let remote_snipping = RemoteSnipping::install(
            cdeh_env,
            config.remote_snipping.clone(),
            cdeh_routes.msaep.clone(),
            cdeh_routes.drm.clone(),
        )?;
        let file_transfer = FileTransfer::install(cdeh_env, config.file_transfer.clone(), cdeh_routes, share_channels)?;
        let phone_files = PhoneFiles::install(cdeh_env, config.phone_files.clone(), cdeh_routes, files_channels)?;
        let connected_camera = ConnectedCamera::install(
            cdeh_env,
            config.connected_camera.clone(),
            cdeh_routes,
            camera_channels,
            edges.phone_camera.clone(),
        )?;

        let bluetooth = Bluetooth::install(env, config.bluetooth.clone(), edges.phone_bluetooth.clone())?;
        let hotspot =
            Hotspot::install(env, config.hotspot.clone(), auth, bluetooth.clone(), edges.hotspot_wifi.clone())?;
        let screen_mirroring = ScreenMirroring::install(
            env,
            config.screen_mirroring.clone(),
            mirror_connector,
            edges.mirror_presenter.clone(),
        )?;

        // One photo, one notification: photos posts none for a phone remote snipping covers.
        photos.set_new_photo_gate(remote_snipping.gate())?;

        Ok(Arc::new(Self {
            audio,
            calling,
            clipboard,
            connected_camera,
            contacts,
            device_status,
            file_transfer,
            messaging,
            notifications,
            phone_apps,
            phone_files,
            photos,
            remote_control,
            wallpaper,
            remote_snipping,
            bluetooth,
            hotspot,
            screen_mirroring,
        }))
    }
}

/// Installs messaging on `env`, resolving names through `contacts`.
fn install_messaging(
    env: &FeatureEnv,
    config: &FeaturesConfig,
    edges: &FeatureEdges,
    contacts: &Arc<Contacts>,
) -> Result<Arc<Messaging>, FeatureError> {
    Messaging::install(
        env,
        config.messaging.clone(),
        Some(contacts.directory()),
        MessagingEdges {
            imaging: edges.attachment_imaging.clone(),
            file_types: edges.file_types.clone(),
            proximity: edges.proximity.clone(),
        },
    )
}

/// Installs device status on the CDEH side's environment and routes.
fn install_device_status(
    cdeh_env: &FeatureEnv,
    cdeh_routes: &PlatformRoutes,
    config: &FeaturesConfig,
    edges: &FeatureEdges,
) -> Result<Arc<DeviceStatus>, FeatureError> {
    DeviceStatus::install(
        cdeh_env,
        config.device_status.clone(),
        cdeh_routes.msaep.clone(),
        cdeh_routes.drm.clone(),
        edges.pc_status.clone(),
    )
}

/// Installs wallpaper on the CDEH side's environment and routes.
fn install_wallpaper(
    cdeh_env: &FeatureEnv,
    cdeh_routes: &PlatformRoutes,
    config: &FeaturesConfig,
    edges: &FeatureEdges,
) -> Result<Arc<Wallpaper>, FeatureError> {
    Wallpaper::install(
        cdeh_env,
        config.wallpaper.clone(),
        cdeh_routes.msaep.clone(),
        cdeh_routes.drm.clone(),
        edges.pc_wallpaper.clone(),
    )
}

/// Installs the clipboard on the CDEH side's environment and routes.
fn install_clipboard(
    cdeh_env: &FeatureEnv,
    cdeh_routes: &PlatformRoutes,
    config: &FeaturesConfig,
    edges: &FeatureEdges,
) -> Result<Arc<Clipboard>, FeatureError> {
    Clipboard::install(
        cdeh_env,
        config.clipboard.clone(),
        cdeh_routes.msaep.clone(),
        cdeh_routes.drm.clone(),
        edges.pasteboard.clone(),
        edges.image_codec.clone(),
    )
}
