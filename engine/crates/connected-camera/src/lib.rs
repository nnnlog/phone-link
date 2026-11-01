//! The connected camera, a Cross Device Experience Host feature on the CDEH identity's connection: the
//! phone's camera as a camera on the Mac.
//!
//! The relay carries the set-up — the `CAMERA_METADATA` sync
//! that makes the phone publish its context handle on tag 2, and the permission exchanges — and the
//! video and its control messages travel on a Nano `VideoStreaming` channel created against that handle.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the requests and their replies, the tag-2 and tag-3 publications, the JSON control messages |
//! | [`store`] | the per-phone state (capability, permissions, the "use as a camera" switch, battery) |
//! | [`channel`] | creating and opening a stream's channel |
//! | [`edge`] | the Mac's camera: publishing it, still images, decoding frames onto it |
//! | this module | gating, listening, the context handle, permissions, the camera's lifecycle, the stream and its state |

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod channel;
pub mod edge;
pub mod store;
pub mod wire;

use phonelink_core::config::features::ConnectedCameraConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use phonelink_core::seams::SessionEvent;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::{FeatureEnv, FeatureError, PhoneSwitchChanged};
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceType;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use phonelink_transport::platform::{
    ContextHandle, DeviceFilter, Drm, DrmSendOutcome, PlatformRoutes, Publication, PublicationObserver,
    SessionValidator, Subscription,
};

pub use channel::{CameraChannels, NanoCameraChannels, OpenedStream, StreamChannel, StreamInbound, StreamOpenError};
pub use edge::{CameraPlaceholder, NoCamera, PhoneCamera};
pub use store::{FEATURE, PhoneData};
pub use wire::{CameraError, CameraFace, OemExtension};

use phonelink_core::sync::MutexExt;
use wire::{Action, CameraConfig, Grant, Inbound};

mod device;
mod handle;
mod permissions;
mod requests;
mod stream;
mod triggers;

/// The camera's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraState {
    Unregistered,
    Registered,
    Connecting,
    Streaming,
    Paused,
    Error,
    TerminalError,
    StartStreamNotification,
}

impl CameraState {
    /// No stream is wanted: `Registered` or `Unregistered`.
    pub fn is_idle(self) -> bool {
        matches!(self, Self::Registered | Self::Unregistered)
    }
}

/// One entry of the effect list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// "None".
    None,
    /// A built-in effect: its `DefaultCameraExtensions` flag.
    BuiltIn(i32),
    /// An effect of the phone's maker.
    Oem(OemExtension),
}

/// An effect label the UI holds a string for, by its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EffectLabel {
    NoFilter,
    Automatic,
    Blur,
    FaceRetouch,
    Hdr,
    Night,
    AutoFraming,
    SoftFocus,
    Stabilization,
}

impl EffectLabel {
    const KEYS: [(&str, Self); 9] = [
        ("CameraEffectNoFilter", Self::NoFilter),
        ("CameraEffectAutomatic", Self::Automatic),
        ("CameraEffectBlur", Self::Blur),
        ("CameraEffectFaceRetouch", Self::FaceRetouch),
        ("CameraEffectHdr", Self::Hdr),
        ("CameraEffectNight", Self::Night),
        ("CameraEffectAutoFraming", Self::AutoFraming),
        ("CameraEffectSoftFocus", Self::SoftFocus),
        ("CameraEffectStabilization", Self::Stabilization),
    ];

    /// The label of an OEM effect's resource name: `camera_effect_auto_framing` is key
    /// `CameraEffectAutoFraming`. A name with an empty `_`-separated part has no key.
    fn for_resource(resource: &str) -> Option<Self> {
        let mut key = String::new();
        for part in resource.split('_') {
            let mut chars = part.chars();
            key.extend(chars.next()?.to_uppercase());
            key.push_str(chars.as_str());
        }
        Self::KEYS.iter().find(|(k, _)| *k == key).map(|(_, label)| *label)
    }
}

/// An OEM effect's label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OemLabel {
    /// The UI's string for its resource name.
    Known(EffectLabel),
    /// Its name's text for the UI locale, else its resource name.
    Text(String),
}

impl Effect {
    /// An OEM effect's label for the UI locale named `locale`: the UI's string for its resource name,
    /// else its name's text for that locale (the first text when `locale` is empty), else its resource name.
    pub fn oem_label(&self, locale: &str) -> Option<OemLabel> {
        let Self::Oem(o) = self else { return None };
        if let Some(label) = EffectLabel::for_resource(&o.name.resource) {
            return Some(OemLabel::Known(label));
        }

        let fallbacks = &o.name.fallbacks;
        let text = if locale.is_empty() { fallbacks.first() } else { fallbacks.iter().find(|f| f.locale == locale) };
        Some(OemLabel::Text(text.map_or_else(|| o.name.resource.clone(), |f| f.value.clone())))
    }
}

/// What the error screen offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorAction {
    /// Refresh: open the stream again.
    Refresh,
    /// Ask the phone for its permissions again.
    RequestPermissions,
    /// Close: tells the phone and leaves the error.
    Close,
    None,
}

impl ErrorAction {
    pub fn for_error(error: CameraError) -> Self {
        match error {
            CameraError::ChannelDisconnected
            | CameraError::MeteredConnection
            | CameraError::LtwDisabled
            | CameraError::PostNotificationPermissionNeeded => Self::Refresh,
            e if e.is_permission() => Self::RequestPermissions,
            CameraError::CameraDeviceFatal | CameraError::CameraServiceFatal => Self::Close,
            _ => Self::None,
        }
    }
}

/// The feature's view of one phone.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraStatus {
    /// The Mac can publish cameras.
    pub mac_available: bool,
    /// The phone can stream its camera.
    pub capable: bool,
    /// The phone is enabled and capable: the setting shows.
    pub supported: bool,
    /// The "use as a camera" switch.
    pub camera_enabled: bool,
    pub registered: bool,
    /// Publishing the camera failed; the setting offers a retry.
    pub registration_failed: bool,
    /// The phone lacks the camera or the notification permission.
    pub permission_required: bool,
    pub camera_permission: bool,
    pub state: CameraState,
    pub error: CameraError,
    pub error_action: ErrorAction,
    pub face: Option<CameraFace>,
    pub effects: Vec<Effect>,
    /// The selected effect's position in `effects`, when it is listed.
    pub selected_effect: Option<usize>,
    /// `-1` before the phone reported it.
    pub battery_level: i32,
    pub is_charging: bool,
    /// The network is poor (shown while streaming).
    pub low_network: bool,
    /// The QR code that leads to the phone's notification permission.
    pub qr_code_uri: Option<String>,
    /// The camera control window shows.
    pub control_window: bool,
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectedCameraEvent {
    /// What [`ConnectedCamera::status`] shows for `phone` may have changed.
    Changed { phone: DcgClient },
    /// Show or hide `phone`'s camera control window.
    ControlWindow { phone: DcgClient, shown: bool },
    /// Turning the camera on for `phone` turned it off for `replaced` (named `name`).
    Replaced { phone: DcgClient, replaced: DcgClient, name: String },
}

/// An operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum ConnectedCameraError {
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    #[error("no stream channel is open")]
    NoStream,
    #[error("no such effect")]
    UnknownEffect,
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// Why a sync left no context handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleFailure {
    Metered,
    AllSyncDisabled,
    ExpectedNoHandle,
    RequestFailed,
}

/// Why no context handle was obtained for a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
enum RetrieveError {
    #[error("the stream was cancelled")]
    Cancelled,
    #[error("no camera context handle within the handle timeout")]
    TimedOut,
    #[error("the camera context handle was invalidated while the stream waited for it")]
    Invalidated,
    /// The sync task logged the request's outcome.
    #[error("the camera metadata sync left no context handle ({0:?})")]
    Failed(HandleFailure),
}

/// A phone's context-handle result.
#[derive(Debug, Clone, Default)]
enum HandleSlot {
    /// No sync was sent.
    #[default]
    Empty,
    Pending,
    Done(Result<ContextHandle, HandleFailure>),
    /// Invalidated: the handle dropped or the stream channel closed.
    Discarded,
}

/// A stream being opened or open.
struct Session {
    id: u64,
    cancel: CancellationToken,
    channel: Option<Arc<dyn StreamChannel>>,
}

/// What the phone's configuration set.
#[derive(Debug, Clone, Default)]
struct Applied {
    face: Option<CameraFace>,
    effects: Vec<Effect>,
    selected: Option<Effect>,
}

/// One phone's in-memory state.
struct PhoneRuntime {
    listening: bool,
    registered: bool,
    registration_failed: bool,
    sync_in_flight: bool,
    /// Bumped whenever the handle result is replaced or discarded.
    handle_gen: u64,
    handle: HandleSlot,
    /// A permission request was sent since the camera was registered.
    permission_requested: bool,
    state: CameraState,
    error: CameraError,
    /// The last still image shown.
    placeholder: Option<CameraPlaceholder>,
    session: Option<Session>,
    applied: Applied,
    /// The network quality, while it is watched.
    low_network: Option<bool>,
    /// A frame's decode panicked, or the connection metrics did not parse, during this stream: the
    /// repeats (every frame, every poll) are not logged.
    decode_panic_logged: bool,
    metrics_failure_logged: bool,
    qr_code_uri: Option<String>,
    control_window: bool,
    /// The client start/stop queue (one waiting signal) while the camera is registered.
    signals: Option<mpsc::Sender<bool>>,
    signals_stop: CancellationToken,
}

impl PhoneRuntime {
    fn new() -> Self {
        Self {
            listening: false,
            registered: false,
            registration_failed: false,
            sync_in_flight: false,
            handle_gen: 0,
            handle: HandleSlot::Empty,
            permission_requested: false,
            state: CameraState::Unregistered,
            error: CameraError::None,
            placeholder: None,
            session: None,
            applied: Applied::default(),
            low_network: None,
            decode_panic_logged: false,
            metrics_failure_logged: false,
            qr_code_uri: None,
            control_window: false,
            signals: None,
            signals_stop: CancellationToken::new(),
        }
    }
}

/// A phone's map and store key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The camera's id on the Mac: the phone's id.
fn device_id(phone: &DcgClient) -> String {
    key(phone).id
}

/// The still image for `state` and `error`; `None` for `Streaming`.
fn placeholder_for(state: CameraState, error: CameraError, auto_prompt: bool) -> Option<CameraPlaceholder> {
    Some(match state {
        CameraState::Streaming => return None,
        CameraState::Registered | CameraState::Unregistered => CameraPlaceholder::Black,
        CameraState::Connecting => CameraPlaceholder::Connecting,
        CameraState::Paused => CameraPlaceholder::Paused,
        CameraState::StartStreamNotification => CameraPlaceholder::StartStreamNotification,
        CameraState::Error | CameraState::TerminalError => match error {
            CameraError::ScreenLocked => CameraPlaceholder::PhoneLocked,
            CameraError::CameraInUse
            | CameraError::Disconnected
            | CameraError::LtwDisabled
            | CameraError::MeteredConnection
            | CameraError::ChannelDisconnected => CameraPlaceholder::Disconnected,
            CameraError::CallInProgress => CameraPlaceholder::Paused,
            e if e.is_permission() => {
                if auto_prompt {
                    CameraPlaceholder::StartStreamNotification
                } else {
                    CameraPlaceholder::PhoneLocked
                }
            }
            _ => CameraPlaceholder::Error,
        },
    })
}

/// The network-quality reading of real-time metrics: `true` when poor; `None` when they do not parse.
fn low_network(metrics: &str, low_loss_rate: f64) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(metrics).ok()?;
    let loss: f64 = v.get("packetlossrate")?.as_str()?.parse().ok()?;
    let received: f64 = v.get("packetsreceivedintimewindow")?.as_str()?.parse().ok()?;
    Some(loss >= low_loss_rate || received <= 0.0)
}

#[derive(Default)]
struct State {
    phones: HashMap<DcgClient, PhoneRuntime>,
    next_session: u64,
}

/// The feature's shared state; tasks and callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    config: ConnectedCameraConfig,
    drm: Arc<Drm>,
    validator: Arc<SessionValidator>,
    channels: Arc<dyn CameraChannels>,
    camera: Arc<dyn PhoneCamera>,
    store: PhoneStores,
    state: Mutex<State>,
    /// Serializes read-modify-write of a phone's row.
    data_lock: Mutex<()>,
    events: broadcast::Sender<ConnectedCameraEvent>,
    /// Bumped whenever a handle result changes.
    handle_changed: watch::Sender<u64>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscriptions: Mutex<Vec<Subscription>>,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The connected-camera feature.
pub struct ConnectedCamera {
    inner: Arc<Inner>,
}

impl ConnectedCamera {
    /// Installs the feature on the CDEH identity's `env` and `routes`: the tag-2 and tag-3
    /// subscriptions, the triggers, and listening to every phone now. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: ConnectedCameraConfig,
        routes: &PlatformRoutes,
        channels: Arc<dyn CameraChannels>,
        camera: Arc<dyn PhoneCamera>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current().map_err(|_| {
            FeatureError::Programming("the connected camera must be installed inside a tokio runtime".into())
        })?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            drm: routes.drm.clone(),
            validator: routes.validator.clone(),
            channels,
            camera,
            store: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
            state: Mutex::new(State::default()),
            data_lock: Mutex::new(()),
            events: broadcast::channel(64).0,
            handle_changed: watch::channel(0).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscriptions: Mutex::new(Vec::new()),
            weak: weak.clone(),
        });

        let triggers = Triggers {
            peers: env.peers.subscribe(),
            sessions: env.sessions.subscribe(),
            switches: env.switches.subscribe(),
        };

        let camera_sub = routes.msaep.subscribe(
            wire::CAMERA_TAG,
            DeviceFilter::Any,
            Arc::new(CameraObserver(Arc::downgrade(&inner))),
        );
        let permission_sub = routes.msaep.subscribe(
            wire::PERMISSIONS_TAG,
            DeviceFilter::Any,
            Arc::new(PermissionObserver(Arc::downgrade(&inner))),
        );
        inner.subscriptions.lock_or_panic().extend([camera_sub, permission_sub]);

        inner.spawn_loop(triggers);
        for peer in inner.env.peers.peers() {
            inner.start_listening(&peer, false);
        }

        info!("feature installed");
        Ok(Arc::new(Self { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<ConnectedCameraEvent> {
        self.inner.events.subscribe()
    }

    fn peer(&self, phone: &DcgClient) -> Result<PeerDevice, ConnectedCameraError> {
        self.inner.env.peers.peer(phone).ok_or(ConnectedCameraError::UnknownPhone)
    }

    /// `phone`'s state.
    pub fn status(&self, phone: &DcgClient) -> Result<CameraStatus, ConnectedCameraError> {
        let peer = self.peer(phone)?;
        let data = self.inner.data(phone)?;
        let inner = &self.inner;
        let capable = inner.capable(&peer, &data);
        let supported = inner.phone_enabled(phone) && capable;
        let mac_available = inner.camera.is_available();

        let status = inner.with_phone(phone, |p| {
            let selected_effect =
                p.applied.selected.as_ref().and_then(|s| p.applied.effects.iter().position(|e| e == s));
            CameraStatus {
                mac_available,
                capable,
                supported,
                camera_enabled: data.camera_enabled,
                registered: p.registered,
                registration_failed: p.registration_failed,
                permission_required: capable && !(data.camera_permission && data.notification_permission),
                camera_permission: data.camera_permission,
                state: p.state,
                error: p.error,
                error_action: ErrorAction::for_error(p.error),
                face: p.applied.face,
                effects: p.applied.effects.clone(),
                selected_effect,
                battery_level: data.battery_level,
                is_charging: data.is_charging,
                low_network: p.low_network.unwrap_or(false),
                qr_code_uri: p.qr_code_uri.clone(),
                control_window: p.control_window,
            }
        });
        Ok(status)
    }

    /// The "use as a camera" switch.
    pub fn set_camera_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), ConnectedCameraError> {
        let peer = self.peer(phone)?;
        self.inner.set_camera_enabled(&peer, enabled)?;
        Ok(())
    }

    /// Publishes the camera again after a failure.
    pub fn retry_registration(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        let peer = self.peer(phone)?;
        self.inner.with_phone(phone, |p| p.registration_failed = false);
        self.inner.register_if_needed(&peer);
        Ok(())
    }

    /// A client of `phone`'s camera started (`true`) or stopped its stream.
    pub fn client_changed(&self, phone: &DcgClient, started: bool) {
        let queue = self.inner.with_phone(phone, |p| p.signals.clone());
        match queue {
            Some(q) => {
                if q.try_send(started).is_err() {
                    debug!(%phone, started, "camera client signal dropped; one is waiting");
                }
            }
            None => debug!(%phone, started, "camera client signal for a camera that is not registered"),
        }
    }

    /// The Mac's camera became available or unavailable.
    pub fn availability_changed(&self) {
        let available = self.inner.camera.is_available();
        info!(available, "Mac camera availability changed");
        for peer in self.inner.env.peers.peers() {
            if available {
                self.inner.start_listening(&peer, false);
            } else {
                self.inner.stop_listening(&peer.client);
            }
        }
    }

    /// The Mac's session locked (`true`) or unlocked.
    pub fn screen_lock_changed(&self, locked: bool) {
        for peer in self.inner.env.peers.peers() {
            let registered = self.inner.with_phone(&peer.client, |p| p.registered);
            if locked {
                if registered {
                    self.inner.unregister(&peer.client);
                }
            } else {
                self.inner.unregister(&peer.client);
                self.inner.register_if_needed(&peer);
            }
        }
    }

    /// The user paused the video.
    pub fn pause(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        self.inner.send_action(phone, Action::Pause, None, None)?;
        self.inner.set_state(phone, CameraState::Paused, CameraError::None);
        Ok(())
    }

    /// The user resumed the video.
    pub fn resume(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        self.inner.send_action(phone, Action::Resume, None, None)?;
        self.inner.set_state(phone, CameraState::Streaming, CameraError::None);
        Ok(())
    }

    /// The user switched between the front and back cameras; the phone confirms with its `Update`.
    pub fn switch_camera(&self, phone: &DcgClient, face: CameraFace) -> Result<(), ConnectedCameraError> {
        self.inner.send_action(phone, Action::Update, None, Some(wire::face_config(face)))
    }

    /// The user picked effect `index` of the list.
    pub fn select_effect(&self, phone: &DcgClient, index: usize) -> Result<(), ConnectedCameraError> {
        let (effect, face) = self.inner.with_phone(phone, |p| (p.applied.effects.get(index).cloned(), p.applied.face));
        let effect = effect.ok_or(ConnectedCameraError::UnknownEffect)?;

        let mut config =
            CameraConfig { camera_face: face.map(CameraFace::wire).unwrap_or(0), ..CameraConfig::default() };
        match &effect {
            Effect::None => config.selected_default_extension = 0,
            Effect::BuiltIn(flag) => config.selected_default_extension = *flag,
            Effect::Oem(o) => {
                config.oem_extension_enabled = true;
                config.selected_oem_extension = Some(o.clone());
            }
        }

        self.inner.send_action(phone, Action::Update, None, Some(config))?;
        self.inner.with_phone(phone, |p| p.applied.selected = Some(effect));
        self.inner.changed(phone);
        Ok(())
    }

    /// The user closed the fatal-error screen.
    pub fn close_fatal_error(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        let error = self.inner.with_phone(phone, |p| p.error);
        self.inner.send_action(phone, Action::Error, Some(error), None)?;
        self.inner.set_state(phone, CameraState::Error, error);
        Ok(())
    }

    /// The user closed the camera control window: the stream closes.
    pub fn close_control_window(&self, phone: &DcgClient) {
        self.inner.close_stream(phone);
        self.inner.set_control_window(phone, false);
    }

    /// The error screen's refresh.
    pub fn refresh(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        let peer = self.peer(phone)?;
        self.inner.refresh(&peer);
        Ok(())
    }

    /// The permission error screen's retry: asks for the permissions when any is missing, else
    /// refreshes.
    pub fn request_permissions(&self, phone: &DcgClient) -> Result<(), ConnectedCameraError> {
        let peer = self.peer(phone)?;
        self.inner.clone().request_permissions_if_needed(peer);
        Ok(())
    }

    /// The setting's "grant permission" action: asks the phone for the camera permission; `true` when
    /// it holds it after the request.
    pub async fn settings_request_permission(&self, phone: &DcgClient) -> Result<bool, ConnectedCameraError> {
        let peer = self.peer(phone)?;
        let outcome = self
            .inner
            .drm_send(
                &peer,
                ProtoDeviceResourceType::Permissions,
                wire::permission_request(),
                wire::trace_name::SETTINGS_PERMISSION,
            )
            .await;

        if let DrmSendOutcome::Success { payload } = outcome {
            match wire::permission_reply(&payload) {
                Some(reply) => {
                    self.inner.update(phone, |d| d.camera_permission = reply.camera)?;
                    self.inner.changed(phone);
                }
                None => {
                    warn!(%phone, "camera permission reply from the setting's request does not parse; the stored permission is returned");
                }
            }
        } else {
            warn!(%phone, ?outcome, "camera permission request from the setting failed; the stored permission is returned");
        }

        Ok(self.inner.data(phone)?.camera_permission)
    }
}

/// The trigger streams, subscribed before any state is read.
struct Triggers {
    peers: broadcast::Receiver<PeerEvent>,
    sessions: broadcast::Receiver<SessionEvent>,
    switches: broadcast::Receiver<PhoneSwitchChanged>,
}

impl Inner {
    fn emit(&self, event: ConnectedCameraEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn changed(&self, phone: &DcgClient) {
        self.emit(ConnectedCameraEvent::Changed { phone: key(phone) });
    }

    fn with_phone<R>(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneRuntime) -> R) -> R {
        let mut state = self.state.lock_or_panic();
        f(state.phones.entry(key(phone)).or_insert_with(PhoneRuntime::new))
    }

    fn data(&self, phone: &DcgClient) -> Result<PhoneData, StoreError> {
        store::load(&*self.store.get(&key(phone))?)
    }

    /// Applies `f` to `phone`'s row; `true` when it changed.
    fn update(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneData)) -> Result<bool, StoreError> {
        let _guard = self.data_lock.lock_or_panic();
        let db = self.store.get(&key(phone))?;
        let before = store::load(&db)?;
        let mut after = before.clone();
        f(&mut after);

        if after != before {
            store::save(&db, &after)?;
        }
        Ok(after != before)
    }

    /// [`Inner::data`], with an unreadable row logged and read as the defaults.
    fn data_or_default(&self, phone: &DcgClient) -> PhoneData {
        self.data(phone).unwrap_or_else(|e| {
            warn!(%phone, error = %e, "camera state unreadable; read as the defaults");
            PhoneData::default()
        })
    }

    /// [`Inner::update`], with a store failure logged.
    fn update_logged(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneData)) -> bool {
        self.update(phone, f).unwrap_or_else(|e| {
            error!(%phone, error = %e, "camera state not saved");
            false
        })
    }

    /// The per-phone switch of the CDEH features; an unreadable switch counts as off.
    fn phone_enabled(&self, phone: &DcgClient) -> bool {
        self.env.switches.is_enabled(phone).unwrap_or_else(|e| {
            warn!(%phone, error = %e, "phone switch unreadable; counted as off");
            false
        })
    }

    fn capable(&self, peer: &PeerDevice, data: &PhoneData) -> bool {
        peer.info.has_capability(wire::CAPABILITY) || data.capable
    }

    fn supported(&self, peer: &PeerDevice, data: &PhoneData) -> bool {
        self.phone_enabled(&peer.client) && self.capable(peer, data)
    }

    fn should_register(&self, peer: &PeerDevice, data: &PhoneData) -> bool {
        self.supported(peer, data) && data.camera_enabled
    }

    fn permission_required(&self, peer: &PeerDevice, data: &PhoneData) -> bool {
        self.capable(peer, data) && !(data.camera_permission && data.notification_permission)
    }

    /// Phones other than `phone` whose camera is registered.
    fn other_registered(&self, phone: &DcgClient) -> Vec<DcgClient> {
        let me = key(phone);
        self.state
            .lock_or_panic()
            .phones
            .iter()
            .filter(|(k, p)| **k != me && p.registered)
            .map(|(k, _)| k.clone())
            .collect()
    }
}

struct CameraObserver(Weak<Inner>);

impl PublicationObserver for CameraObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_camera(publication);
        }
    }
}

struct PermissionObserver(Weak<Inner>);

impl PublicationObserver for PermissionObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_permissions(publication);
        }
    }
}

#[cfg(test)]
mod tests;
