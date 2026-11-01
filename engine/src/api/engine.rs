//! The handle `gui/` holds: [`Engine`] builds the engine's core on its first start (and a new one
//! when the core needs a rebuild), forwards the app's calls to it, and hands out the feature objects.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::api::audio::AudioApi;
use crate::api::clipboard::ClipboardApi;
use crate::api::connected_camera::ConnectedCameraApi;
use crate::api::contacts::ContactsApi;
use crate::api::device_status::DeviceStatusApi;
use crate::api::events::{EventBridge, EventSink, EventWebSignIn, WebSignInPending};
use crate::api::file_transfer::FileTransferApi;
use crate::api::hotspot::HotspotApi;
use crate::api::messaging::MessagingApi;
use crate::api::notifications::NotificationsApi;
use crate::api::pairing::PairingApi;
use crate::api::phone::PhoneApi;
use crate::api::phone_apps::PhoneAppsApi;
use crate::api::phone_files::PhoneFilesApi;
use crate::api::photos::PhotosApi;
use crate::api::remote_control::RemoteControlApi;
use crate::api::remote_snipping::RemoteSnippingApi;
use crate::api::screen_mirroring::ScreenMirroringApi;
use crate::api::wallpaper::WallpaperApi;
use crate::app::ConnectTrigger;
use crate::config::EngineConfig;
use crate::edge::{Edges, HostInfo, Reachability, SecureStore, SessionLock, SystemClock};
use crate::engine::{EngineCore, EngineError, SystemEvent, build_broker, sign_out_stored};
use crate::features::bluetooth::PhoneBluetooth;
use crate::features::clipboard::{ClipboardImageCodec, ClipboardPasteboard};
use crate::features::connected_camera::PhoneCamera;
use crate::features::device_status::PcStatusSource;
use crate::features::hotspot::HotspotWifi;
use crate::features::messaging::{AttachmentImaging, MessagingFileTypes, PhoneProximity};
use crate::features::phone_apps::PhoneAppsHost;
use crate::features::remote_control::PcPowerControl;
use crate::features::screen_mirroring::MirrorPresenter;
use crate::features::wallpaper::PcWallpaperSource;
use crate::features::{FeatureEdges, NotificationResponse, StoreLocation, SystemNotifier};
use crate::sync::MutexExt;
use crate::tap::{TAP_DIR_VAR, tap_from_env};

fn setup_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::Setup { message: e.to_string() }
}

/// A snapshot for the UI.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EngineStatus {
    pub own_device_id: Option<String>,
    pub phone_id: Option<String>,
    pub phone_connected: bool,
    /// Signed in and enrolled, and no phone is selected: the UI asks the user to select one of
    /// `PhoneApi::phones`.
    pub needs_phone_selection: bool,
}

/// The engine handle `gui/` holds.
#[derive(uniffi::Object)]
pub struct Engine {
    config: EngineConfig,
    edges: Edges,
    feature_edges: FeatureEdges,
    /// Where the UI hears what changed.
    events: Arc<dyn EventSink>,
    /// The app's foreground state as last reported; a core applies it when it starts.
    foreground: AtomicBool,
    core: Mutex<Option<Arc<EngineCore>>>,
    /// Web sign-ins awaiting the UI's answer (empty unless `edges.web_sign_in` is the event-driven
    /// one built by the `uniffi` constructor).
    web_sign_in_pending: Arc<WebSignInPending>,
}

impl Engine {
    /// An engine whose cores are built with `config` — for Rust embedders and tests; the `uniffi`
    /// constructor passes the production configuration. `edges.web_sign_in` is used as given.
    pub fn with_config(
        config: EngineConfig,
        edges: Edges,
        feature_edges: FeatureEdges,
        events: Arc<dyn EventSink>,
    ) -> Arc<Self> {
        Self::with_config_pending(config, edges, feature_edges, events, Arc::new(WebSignInPending::new()))
    }

    fn with_config_pending(
        config: EngineConfig,
        edges: Edges,
        feature_edges: FeatureEdges,
        events: Arc<dyn EventSink>,
        web_sign_in_pending: Arc<WebSignInPending>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            edges,
            feature_edges,
            events,
            foreground: AtomicBool::new(false),
            core: Mutex::new(None),
            web_sign_in_pending,
        })
    }

    /// The current core, once a start has built one (Rust embedders, tests; not exported).
    pub fn current_core(&self) -> Option<Arc<EngineCore>> {
        self.core.lock_or_recover().clone()
    }

    fn core(&self) -> Result<Arc<EngineCore>, EngineError> {
        self.current_core().ok_or(EngineError::NotStarted)
    }
}

/// The machine's device-bound registration, for the Settings › Advanced view.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceRegistration {
    /// The device PUID the account service assigned.
    pub puid: String,
    /// This Mac's `distinguishedDeviceId` on each of its enrolled records (as the phone pairs them).
    pub distinguished_device_ids: Vec<String>,
}

#[uniffi::export(async_runtime = "tokio")]
impl Engine {
    /// `data_dir` holds the features' stores: a directory of the engine's own (macOS: the app's
    /// subdirectory of Application Support); created `0700` when missing, an existing one keeps its
    /// mode.
    #[uniffi::constructor]
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub fn new(
        store: Arc<dyn SecureStore>,
        lock: Arc<dyn SessionLock>,
        host: Arc<dyn HostInfo>,
        reachability: Arc<dyn Reachability>,
        events: Arc<dyn EventSink>,
        notifier: Arc<dyn SystemNotifier>,
        pasteboard: Arc<dyn ClipboardPasteboard>,
        image_codec: Arc<dyn ClipboardImageCodec>,
        pc_status: Arc<dyn PcStatusSource>,
        attachment_imaging: Arc<dyn AttachmentImaging>,
        file_types: Arc<dyn MessagingFileTypes>,
        proximity: Arc<dyn PhoneProximity>,
        pc_power: Arc<dyn PcPowerControl>,
        pc_wallpaper: Arc<dyn PcWallpaperSource>,
        phone_bluetooth: Arc<dyn PhoneBluetooth>,
        hotspot_wifi: Arc<dyn HotspotWifi>,
        phone_camera: Arc<dyn PhoneCamera>,
        phone_apps_host: Arc<dyn PhoneAppsHost>,
        mirror_presenter: Arc<dyn MirrorPresenter>,
        data_dir: String,
    ) -> Arc<Self> {
        let pending = Arc::new(WebSignInPending::new());
        let web_sign_in = Arc::new(EventWebSignIn::new(events.clone(), pending.clone()));
        let edges =
            Edges { store, lock, host, reachability, clock: Arc::new(SystemClock), web_sign_in, token_transport: None };

        let feature_edges = FeatureEdges {
            stores: StoreLocation::Directory(data_dir.into()),
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
            phone_camera,
            phone_apps_host,
            mirror_presenter,
        };

        Self::with_config_pending(EngineConfig::production(), edges, feature_edges, events, pending)
    }

    /// The UI's answer to a [`EngineEvent::WebSignInRequested`]: the JSON the sign-in page posted.
    pub fn complete_web_sign_in(&self, request_id: String, result_json: String) {
        self.web_sign_in_pending.resolve(&request_id, Ok(result_json));
    }

    /// The UI could not complete a web sign-in: `declined` when the user closed it, else a failure.
    pub fn cancel_web_sign_in(&self, request_id: String, declined: bool) {
        let err = if declined {
            crate::edge::WebSignInError::Declined
        } else {
            crate::edge::WebSignInError::Failed { reason: "the web sign-in was cancelled".into() }
        };
        self.web_sign_in_pending.resolve(&request_id, Err(err));
    }

    /// The engine crate version.
    pub fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    /// Builds the engine (the production configuration) and runs the start sequence. A core that
    /// needs a rebuild — a feature install failed part-way, or the account was removed — is stopped
    /// and replaced by a new one first. The app's reported foreground state reaches the core while it
    /// starts and once more after: the App layer's start sends the activation connect when the app is
    /// in the foreground, and a state reported later sends it then.
    pub async fn start(&self) -> Result<EngineStatus, EngineError> {
        let stale = {
            let mut slot = self.core.lock_or_recover();
            if slot.as_ref().is_some_and(|core| core.needs_rebuild()) { slot.take() } else { None }
        };

        if let Some(stale) = stale {
            stale.stop().await;
        }

        let core = {
            let mut slot = self.core.lock_or_recover();
            match slot.as_ref() {
                Some(core) => core.clone(),
                None => {
                    let tap = tap_from_env().map_err(|e| setup_err(format!("opening the tap ({TAP_DIR_VAR}): {e}")))?;
                    let core = EngineCore::build(
                        self.config.clone(),
                        self.edges.clone(),
                        self.feature_edges.clone(),
                        tap,
                        EventBridge::new(self.events.clone()),
                    )?;

                    // The foreground state reported so far; later reports reach the core in the slot.
                    core.app.set_foreground(self.foreground.load(Ordering::SeqCst));
                    *slot = Some(core.clone());
                    core
                }
            }
        };

        if let Err(e) = core.start(None).await {
            if core.needs_rebuild() {
                // Some features stay registered on this core: the next start builds a new one.
                *self.core.lock_or_recover() = None;
            }
            return Err(e);
        }

        core.set_foreground(self.foreground.load(Ordering::SeqCst));
        Ok(self.status())
    }

    /// A user-initiated refresh of the phone connection (an immediate connect message); while the
    /// core is still starting it sends nothing (the start sends its own connects).
    pub fn refresh(&self) -> Result<(), EngineError> {
        let core = self.core()?;
        let _runtime = core.enter_runtime();
        core.app.trigger(ConnectTrigger::Refresh);
        Ok(())
    }

    /// The app came to the foreground (`true`) or left it (`false`) — macOS: the application became
    /// or resigned active. Sets `isForeground` of the trace state, and coming to the foreground sends
    /// an immediate connect (a connect trigger).
    /// Before `start` the state is kept and handed to the core the start builds; during the start the
    /// core records it, and its App layer's start sends the immediate connect when the app is in the
    /// foreground.
    pub fn set_foreground(&self, foreground: bool) {
        self.foreground.store(foreground, Ordering::SeqCst);
        if let Some(core) = self.core.lock_or_recover().clone() {
            core.set_foreground(foreground);
        }
    }

    /// Ends the phone session.
    pub async fn stop(&self) -> Result<(), EngineError> {
        self.core()?.stop().await;
        Ok(())
    }

    /// The user's sign-out, local only: the current core, if any, ends its phone session and is
    /// dropped; then both identities forget the Microsoft
    /// account and their state, and the selected phone and every phone's feature stores are deleted
    /// ([`sign_out_stored`]). The next [`Engine::start`] signs in from the first prompt, as any
    /// account.
    pub async fn sign_out(&self) -> Result<(), EngineError> {
        let core = self.core.lock_or_recover().take();
        if let Some(core) = core {
            core.retire().await;
        }
        sign_out_stored(&self.config, &self.edges, &self.feature_edges.stores)
    }

    /// The machine's device registration: the device PUID the account service assigned, and this
    /// Mac's `distinguishedDeviceId` on each of its enrolled records (empty when not yet listed).
    /// `None` when the machine is not registered.
    pub fn device_registration(&self) -> Option<DeviceRegistration> {
        let read: Result<_, Box<dyn std::error::Error + Send + Sync>> = match build_broker(&self.edges) {
            Ok(broker) => broker.device_puid().map_err(Into::into),
            Err(e) => Err(e.into()),
        };
        let puid = match read {
            Ok(puid) => puid?,
            Err(e) => {
                tracing::warn!(error = %e, "the device registration could not be read; shown as not registered");
                return None;
            }
        };

        // Each identity's own entry of the device list it last applied (the peer lists exclude it).
        let distinguished = self
            .core
            .lock_or_recover()
            .as_ref()
            .map(|core| {
                let cdeh = core.cdeh.as_ref().and_then(|s| s.auth.own_entry());
                let mut ids: Vec<String> = core
                    .auth
                    .own_entry()
                    .into_iter()
                    .chain(cdeh)
                    .filter_map(|e| e.distinguished_device_id)
                    .filter(|id| !id.is_empty())
                    .collect();
                ids.dedup();
                ids
            })
            .unwrap_or_default();

        Some(DeviceRegistration { puid, distinguished_device_ids: distinguished })
    }

    /// Resets the machine's device registration: deletes this Mac's own records from the account
    /// (best-effort), signs out locally, and forgets the device credential and ticket. The next
    /// sign-in registers the machine afresh and receives a new `distinguishedDeviceId`. The MSA
    /// device identity created on the account service cannot be removed and stays.
    pub async fn reset_device_registration(&self) -> Result<(), EngineError> {
        let core = self.core.lock_or_recover().take();
        if let Some(core) = core {
            // Best-effort server delete of both identities' own records before dropping the core.
            if let Err(e) = core.auth.delete_own_device_info().await {
                tracing::warn!(error = %e, "this Mac's Phone Link record not deleted from the account; it stays");
            }
            if let Some(cdeh) = &core.cdeh
                && let Err(e) = cdeh.auth.delete_own_device_info().await
            {
                tracing::warn!(error = %e, "this Mac's Cross Device record not deleted from the account; it stays");
            }

            core.retire().await;
        }

        sign_out_stored(&self.config, &self.edges, &self.feature_edges.stores)?;
        build_broker(&self.edges)?.reset().map_err(|e| setup_err(format!("reset device registration: {e}")))?;
        Ok(())
    }

    /// What the user did with a notification the engine posted through [`SystemNotifier`]
    /// (macOS: the `UNUserNotificationCenter` delegate's response): `id` and `user_info` as posted.
    pub fn notification_response(
        &self,
        id: String,
        user_info: HashMap<String, String>,
        response: NotificationResponse,
    ) -> Result<(), EngineError> {
        self.core()?.notification_response(&id, &user_info, response);
        Ok(())
    }

    /// The Mac's session was locked or unlocked, or it sleeps or woke ([`EngineCore::system_event`]).
    /// Before a start has built a core it is ignored. During a start the core gets it: unlock and
    /// wake reset the wake breakers and the partner set at once, but their connect is not sent
    /// until the App layer has started — the start sends its own.
    pub fn system_event(&self, event: SystemEvent) {
        if let Some(core) = self.core.lock_or_recover().clone() {
            core.system_event(event);
        }
    }

    /// The notifications feature: the phone's mirrored notifications, their actions and settings.
    ///
    /// Like every feature handle, it can be taken before `Engine::start`; its calls fail with
    /// `FeatureApiError::NotStarted` until a start has installed the features.
    pub fn notifications(self: Arc<Self>) -> Arc<NotificationsApi> {
        NotificationsApi::new(self)
    }

    /// The clipboard feature: the global switch and the selected phone's clipboard sharing.
    pub fn clipboard(self: Arc<Self>) -> Arc<ClipboardApi> {
        ClipboardApi::new(self)
    }

    /// The device-status feature: the selected phone's battery, signal and similar state.
    pub fn device_status(self: Arc<Self>) -> Arc<DeviceStatusApi> {
        DeviceStatusApi::new(self)
    }

    /// The messaging feature: the selected phone's conversations and sending messages.
    pub fn messaging(self: Arc<Self>) -> Arc<MessagingApi> {
        MessagingApi::new(self)
    }

    /// The contacts feature: the selected phone's contacts.
    pub fn contacts(self: Arc<Self>) -> Arc<ContactsApi> {
        ContactsApi::new(self)
    }

    /// The photos feature: the selected phone's recent photos.
    pub fn photos(self: Arc<Self>) -> Arc<PhotosApi> {
        PhotosApi::new(self)
    }

    /// The audio feature: media controls for the selected phone's media sessions.
    pub fn audio(self: Arc<Self>) -> Arc<AudioApi> {
        AudioApi::new(self)
    }

    /// The remote-control feature of the selected phone.
    pub fn remote_control(self: Arc<Self>) -> Arc<RemoteControlApi> {
        RemoteControlApi::new(self)
    }

    /// The instant hotspot feature: joining the selected phone's hotspot.
    pub fn hotspot(self: Arc<Self>) -> Arc<HotspotApi> {
        HotspotApi::new(self)
    }

    /// The wallpaper feature: the selected phone's wallpaper.
    pub fn wallpaper(self: Arc<Self>) -> Arc<WallpaperApi> {
        WallpaperApi::new(self)
    }

    /// The new-photo notifications feature of the selected phone.
    pub fn remote_snipping(self: Arc<Self>) -> Arc<RemoteSnippingApi> {
        RemoteSnippingApi::new(self)
    }

    /// The file-transfer feature: sending files to and receiving files from the selected phone.
    pub fn file_transfer(self: Arc<Self>) -> Arc<FileTransferApi> {
        FileTransferApi::new(self)
    }

    /// The phone-files feature: browsing the selected phone's files from this Mac.
    pub fn phone_files(self: Arc<Self>) -> Arc<PhoneFilesApi> {
        PhoneFilesApi::new(self)
    }

    /// The connected-camera feature: using the selected phone's camera.
    pub fn connected_camera(self: Arc<Self>) -> Arc<ConnectedCameraApi> {
        ConnectedCameraApi::new(self)
    }

    /// The phone-apps feature: the selected phone's apps and recent apps.
    pub fn phone_apps(self: Arc<Self>) -> Arc<PhoneAppsApi> {
        PhoneAppsApi::new(self)
    }

    /// The screen-mirroring feature: the selected phone's screen and apps in windows on this Mac.
    pub fn screen_mirroring(self: Arc<Self>) -> Arc<ScreenMirroringApi> {
        ScreenMirroringApi::new(self)
    }

    /// The selected phone, the account's phones to select from, and the account's PCs.
    pub fn phone(self: Arc<Self>) -> Arc<PhoneApi> {
        PhoneApi::new(self)
    }

    /// Pairing sessions that link a phone to the signed-in account.
    pub fn pairing(self: Arc<Self>) -> Arc<PairingApi> {
        PairingApi::new(self)
    }

    /// This Mac's device id, the selected phone, and whether that phone is connected.
    ///
    /// Before a start (or while the core is being rebuilt) every field is empty or `false`.
    pub fn status(&self) -> EngineStatus {
        match self.core.lock_or_recover().clone() {
            None => EngineStatus {
                own_device_id: None,
                phone_id: None,
                phone_connected: false,
                needs_phone_selection: false,
            },
            Some(core) => EngineStatus {
                own_device_id: core.auth.own_client().map(|c| c.id),
                phone_id: core.phone().map(|p| p.id),
                phone_connected: core.phone_connected(),
                needs_phone_selection: core.auth.own_client().is_some() && core.phone().is_none(),
            },
        }
    }
}
