//! The features, on top of the App message layer ([`crate::app`]) and the platform layer's
//! MSAEP and `/DeviceResourceManager` ([`crate::platform`]).
//!
//! | Module | Implements |
//! |---|---|
//! | [`audio`] | media controls: the phone's media sessions from its stored media-style notifications, the `/command/audio` request, the per-phone audio-controls setting |
//! | [`bluetooth`] | the phone's Bluetooth Classic device for every feature that reaches the phone over Bluetooth: the phone app's Bluetooth facts from the App layer, which paired device is the phone, the pairing a feature runs when the user starts it and none is; the Bluetooth OS edge |
//! | [`clipboard`] | the cross-device clipboard: tag-9 announcements both ways, the `/clipboard` content pull and receiver, the feature-state sync; the pasteboard and image-codec OS edges |
//! | [`connected_camera`] | the phone's camera as a Mac camera: tags 2 and 3 and the context handle, the `CAMERA_METADATA` sync and auto-prompt, the camera `PERMISSIONS` request, the camera's lifecycle, the stream over a Nano `VideoStreaming` channel, its control messages and state; the camera OS edge |
//! | [`contacts`] | contacts: `contactsV2` sync, `/contacts/v2`, the contacts `/permission`, the connect provider, the lookup by address |
//! | [`shared::db`] | the app's own per-phone persistent stores |
//! | [`file_transfer`] | files to and from the phone over a Nano `NamedStreams` channel: tag 3 and the context handle, the files `PERMISSIONS` request, the `FILE_SHARE` probe, the `/fileshare_android2cdeh_phase1` receiver, the send and receive sessions, their notifications, the per-phone settings |
//! | [`device_status`] | device status: both contracts (CDEH pull, push and `/remotedevicestatus`; Phone Link connect flags, sync request, status content, commands), the status per phone, the DND and ringer controls, the low-battery notification |
//! | [`hotspot`] | instant hotspot: the phone's hotspot state from the App layer, the connect (pairing through [`bluetooth`] and the phone's consent when missing, the side-channel credentials request, the join); the Wi-Fi OS edge |
//! | [`installed`] | the installed set of features, their configuration and their OS edges; the install order |
//! | [`messaging`] | SMS, MMS and RCS: the `messaging` exchange, the connect provider, `/chat/v1` requests, the messages permission prompt, new-message notifications; the attachment-imaging, file-type and proximity OS edges |
//! | [`notifications`] | phone notifications mirrored on the Mac: the `notifications` content processor, `/notifications` and `/settings` requests and their triggers, the persistent action queue, the connect provider, the banner decision through [`shared::notifier`] |
//! | [`shared::notifier`] | the system-notification OS edge shared by notifications, device status, messaging and photos |
//! | [`shared::peers`] | the account's linked phones and their device-list entries, as the features read them |
//! | [`phone_files`] | the phone's storage in the Mac's file manager: tag 4 and the context handle, the config sync and the files `PERMISSIONS` request, metadata syncs into the golden state, thumbnails, downloads and uploads over Nano `NamedStreams` channels, create folder, rename, move and delete |
//! | [`phone_apps`] | the phone's apps and recent apps: the `phoneApps` sync, the connect provider, the per-phone store, the lists, favorites and icons, the gates the launching features share; the app-opening and name-order OS edge |
//! | [`shared::phone_number`] | E.164 normalization and the phone's country, for the local lookup by address |
//! | [`shared::phone_switch`] | the per-phone "enabled" switch of the Cross Device Experience Host features |
//! | [`photos`] | photos: the `mediaInfo` sync, the `/media/v1` fetches, delete and permission flows, the per-phone media cache |
//! | [`remote_control`] | the phone locks, sleeps, shuts down or restarts the Mac: the `/remotecontrol` receiver, the `remoteControl` processor, the per-phone toggle; the power OS edge |
//! | [`wallpaper`] | wallpaper: Phone Link's `wallpaper` content and connect provider, the Cross Device pull and `/wallpaper` receiver; the desktop-picture OS edge |
//! | [`remote_snipping`] | new-photo notifications of the Cross Device Experience Host: tag 6 and 3, the `MEDIA` capability, thumbnail and photo requests, the `PERMISSIONS` request, the gate of photos' own new-photo notification |
//!
//! A feature is installed once, after the platform routes and before the App layer starts, so its
//! content processors, connect provider, DRM receivers and MSAEP subscriptions are in place for the
//! engine-start connect. It receives a [`FeatureEnv`] (and, for the Cross Device Experience Host
//! features, the platform routes' MSAEP and DRM handles), registers on the App layer's and the
//! platform layer's extension points, and exposes the operations its UI needs.

// Every item of the features' shared crate at `features::`, the path the engine, the API and the
// tests name it by; the shared crate lists its own items, so a glob here adds no second list.
pub use phonelink_features::*;

pub use phonelink_audio as audio;
pub use phonelink_bluetooth as bluetooth;
pub use phonelink_clipboard as clipboard;
pub use phonelink_connected_camera as connected_camera;
pub use phonelink_contacts as contacts;
pub use phonelink_device_status as device_status;
pub use phonelink_file_transfer as file_transfer;
pub use phonelink_hotspot as hotspot;
pub mod installed;
pub use phonelink_messaging as messaging;
pub use phonelink_notifications as notifications;
pub use phonelink_phone_apps as phone_apps;
pub use phonelink_phone_files as phone_files;
pub use phonelink_photos as photos;
pub use phonelink_remote_control as remote_control;
pub use phonelink_remote_snipping as remote_snipping;
pub use phonelink_screen_mirroring as screen_mirroring;
pub use phonelink_wallpaper as wallpaper;

pub use installed::{FeatureEdges, Features};
