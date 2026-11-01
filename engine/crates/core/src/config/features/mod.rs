//! The features' options and timers, one module per feature, and [`FeaturesConfig`], which holds
//! them all.

mod audio;
mod bluetooth;
mod clipboard;
mod connected_camera;
mod contacts;
mod device_status;
mod file_transfer;
mod hotspot;
mod messaging;
mod notifications;
mod phone_apps;
mod phone_files;
mod photos;
mod remote_control;
mod remote_snipping;
mod screen_mirroring;
mod wallpaper;

pub use audio::AudioConfig;
pub use bluetooth::BluetoothConfig;
pub use clipboard::{ClipboardConfig, MAX_PASSES, PNG_LIMIT};
pub use connected_camera::ConnectedCameraConfig;
pub use contacts::ContactsConfig;
pub use device_status::DeviceStatusConfig;
pub use file_transfer::FileTransferConfig;
pub use hotspot::HotspotConfig;
pub use messaging::MessagingConfig;
pub use notifications::NotificationsConfig;
pub use phone_apps::PhoneAppsConfig;
pub use phone_files::PhoneFilesConfig;
pub use photos::PhotosConfig;
pub use remote_control::RemoteControlConfig;
pub use remote_snipping::RemoteSnippingConfig;
pub use screen_mirroring::LinkTimers;
pub use screen_mirroring::ScreenMirroringConfig;
pub use wallpaper::WallpaperConfig;

/// Every feature's flags, timers and limits, at their defaults.
#[derive(Debug, Clone, Default)]
pub struct FeaturesConfig {
    pub audio: AudioConfig,
    pub clipboard: ClipboardConfig,
    pub connected_camera: ConnectedCameraConfig,
    pub contacts: ContactsConfig,
    pub device_status: DeviceStatusConfig,
    pub file_transfer: FileTransferConfig,
    pub messaging: MessagingConfig,
    pub notifications: NotificationsConfig,
    pub phone_apps: PhoneAppsConfig,
    pub phone_files: PhoneFilesConfig,
    pub photos: PhotosConfig,
    pub remote_control: RemoteControlConfig,
    pub wallpaper: WallpaperConfig,
    pub remote_snipping: RemoteSnippingConfig,
    pub bluetooth: BluetoothConfig,
    pub hotspot: HotspotConfig,
    pub screen_mirroring: ScreenMirroringConfig,
}
