//! The clipboard feature over the platform test world (`support/platform_world`): the PC's platform
//! layer (routes, MSAEP, DRM) against the scripted phone, a `FeatureEnv` over it, the in-memory
//! pasteboard and the fake image codec. `tests/main.rs` declares it as `crate::support::clipboard_world`, next
//! to the platform fakes it uses (`crate::support::platform_world`).
#![allow(dead_code)]

use phonelink_engine::config::features::ClipboardConfig;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio::sync::broadcast;

use phonelink_engine::app::{AccountGate, AppLayer};
use phonelink_engine::auth::{PeerDevice, PeerEvent};
use phonelink_engine::edge::testing::{ManualClock, memory_edges};
use phonelink_engine::edge::{EdgeError, Edges};
use phonelink_engine::features::clipboard::image::standins::FakeCodec;
use phonelink_engine::features::clipboard::pasteboard::standins::MemoryPasteboard;
use phonelink_engine::features::clipboard::{
    Clipboard, ClipboardEvent, ClipboardPasteboard, ClipboardPasteboardData, ClipboardPasteboardWrite,
};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PeerDirectory, PhoneSwitches, StoreLocation};
use phonelink_engine::platform::{InboundMessage, route};
use phonelink_engine::proto::clipboard::v1 as cb;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::{
    ConnectOutcome, ConnectRequest, PartnerSessionControl, PlatformMessenger, PlatformResponse, SessionEvent,
};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, DeviceInfoItem, TransportMessage};

use crate::support::platform_world::{WakeScript, World, phone_id, platform, request, settle};

pub const CID: &str = "00000000-0000-4000-8000-000000000001";

/// The phone's device-list entry: Android, `CLIPBOARD`, a recent app version.
pub fn phone_device() -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone_id().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        name: Some("Pixel".into()),
        capabilities: Some(vec!["DEVICE_STATUS".into(), "CLIPBOARD".into()]),
        client_version: Some("1.24072.123.0".into()),
        ..Default::default()
    }
}

pub fn phone_peer(info: DeviceInfoItem) -> PeerDevice {
    PeerDevice { client: phone_id(), info }
}

struct Ready;

#[async_trait]
impl AccountGate for Ready {
    async fn is_ready(&self, _phone: &DcgClient) -> bool {
        true
    }
}

/// The feature's view of the phone's session: the world's sessions, unless the test overrides the
/// availability (to make the feature and the request wrapper disagree) or injects events.
pub struct FeatureSessions {
    inner: Arc<phonelink_engine::platform::PartnerSessions>,
    available: Mutex<Option<bool>>,
    events: broadcast::Sender<SessionEvent>,
}

impl FeatureSessions {
    pub fn new(inner: Arc<phonelink_engine::platform::PartnerSessions>) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        let mut from_inner = inner.subscribe();
        let forward = events.clone();

        tokio::spawn(async move {
            while let Ok(event) = from_inner.recv().await {
                let _ = forward.send(event);
            }
        });
        Arc::new(Self { inner, available: Mutex::new(None), events })
    }

    /// `Some(v)`: the feature sees the phone as `v`; `None`: the real session state.
    pub fn override_available(&self, available: Option<bool>) {
        *self.available.lock().unwrap() = available;
    }

    pub fn send_event(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }
}

#[async_trait]
impl PartnerSessionControl for FeatureSessions {
    async fn connect(&self, partner: &DcgClient, request: ConnectRequest) -> ConnectOutcome {
        self.inner.connect(partner, request).await
    }

    fn is_connected(&self, partner: &DcgClient) -> bool {
        self.available.lock().unwrap().unwrap_or_else(|| self.inner.is_connected(partner))
    }

    fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }
}

pub struct Setup {
    pub config: ClipboardConfig,
    pub stores: StoreLocation,
    pub device: DeviceInfoItem,
    /// The phone is in the account's device list when the feature is installed (else the list is
    /// empty).
    pub linked: bool,
    /// The phone is on the hub before the feature is installed.
    pub connected: bool,
    /// How the feature sees the phone's availability from the start (see
    /// [`FeatureSessions::override_available`]); `None`: the real session state.
    pub available: Option<bool>,
    /// The waker's first answer, scripted before the feature is installed.
    pub wake: Option<WakeScript>,
    /// How the phone's `/DeviceResourceManager` handler answers (see [`PhoneDrm`]).
    pub drm: PhoneDrm,
    /// The pasteboard as it is when the feature starts (a fresh empty one by default).
    pub pasteboard: Option<Arc<MemoryPasteboard>>,
    /// The feature reads the pasteboard through a [`GatedPasteboard`] (see [`ClipWorld::gate`]),
    /// whose read hook lets a test act in the middle of a read.
    pub gated: bool,
    /// The account is enrolled when the feature is installed.
    pub enrolled: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            config: ClipboardConfig::default(),
            stores: StoreLocation::Memory,
            device: phone_device(),
            linked: true,
            connected: true,
            available: None,
            wake: None,
            drm: PhoneDrm::Answer(None),
            pasteboard: None,
            gated: false,
            enrolled: true,
        }
    }
}

/// The account's devices as the feature sees them: the test's [`StaticPeers`], except that a phone
/// can be made to vanish from the device list after a number of lookups — to take it away between
/// two reads of one operation.
pub struct ClipPeers {
    inner: Arc<StaticPeers>,
    vanish: Mutex<Option<(DcgClient, usize)>>,
}

impl ClipPeers {
    pub fn new(inner: Arc<StaticPeers>) -> Arc<Self> {
        Arc::new(Self { inner, vanish: Mutex::new(None) })
    }

    /// `phone` is found by its next `lookups` lookups ([`PeerDirectory::peer`]), then it is no
    /// longer in the list (no device-list event is raised).
    pub fn vanish_after(&self, phone: &DcgClient, lookups: usize) {
        *self.vanish.lock().unwrap() = Some((phone.clone(), lookups));
    }

    fn vanished(&self) -> Option<DcgClient> {
        self.vanish.lock().unwrap().as_ref().filter(|(_, left)| *left == 0).map(|(p, _)| p.clone())
    }
}

impl PeerDirectory for ClipPeers {
    fn peers(&self) -> Vec<PeerDevice> {
        let vanished = self.vanished();
        self.inner.peers().into_iter().filter(|p| Some(&p.client) != vanished.as_ref()).collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.inner.subscribe()
    }

    fn is_enrolled(&self) -> bool {
        self.inner.is_enrolled()
    }

    fn peer(&self, phone: &DcgClient) -> Option<PeerDevice> {
        if let Some((vanishing, left)) = self.vanish.lock().unwrap().as_mut() {
            if vanishing == phone {
                if *left == 0 {
                    return None;
                }
                *left -= 1;
            }
        }
        self.inner.peer(phone)
    }
}

type ReadHook = Box<dyn FnOnce() + Send>;

/// The test's [`MemoryPasteboard`] with a hook on the next `types()` read — the first read of an
/// announcement or a content answer — so a test acts while the feature is in the middle of a read
/// (on the blocking thread that reads; paused tokio time cannot advance while it is blocked, so
/// the hook runs instead of blocking).
pub struct GatedPasteboard {
    inner: Arc<MemoryPasteboard>,
    hook: Mutex<Option<ReadHook>>,
    fired: Mutex<bool>,
}

impl GatedPasteboard {
    pub fn new(inner: Arc<MemoryPasteboard>) -> Arc<Self> {
        Arc::new(Self { inner, hook: Mutex::new(None), fired: Mutex::new(false) })
    }

    /// `hook` runs inside the next `types()` read, before it returns.
    pub fn on_next_read(&self, hook: impl FnOnce() + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(hook));
        *self.fired.lock().unwrap() = false;
    }

    /// The hook has run.
    pub fn fired(&self) -> bool {
        *self.fired.lock().unwrap()
    }
}

impl ClipboardPasteboard for GatedPasteboard {
    fn change_count(&self) -> Result<i64, EdgeError> {
        self.inner.change_count()
    }

    fn types(&self) -> Result<Vec<String>, EdgeError> {
        let hook = self.hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
            *self.fired.lock().unwrap() = true;
        }
        self.inner.types()
    }

    fn data(&self, uti: String) -> Result<Option<Vec<u8>>, EdgeError> {
        self.inner.data(uti)
    }

    fn write(&self, representations: Vec<ClipboardPasteboardData>) -> Result<ClipboardPasteboardWrite, EdgeError> {
        self.inner.write(representations)
    }
}

/// The scripted phone's answers on `/DeviceResourceManager`.
#[derive(Clone)]
pub enum PhoneDrm {
    /// Feature-state requests (and `STATUS`) get an empty `SUCCESS`; a `CONTENT` pull gets this
    /// clipboard payload (none: an empty payload).
    Answer(Option<Vec<u8>>),
    /// No answer at all.
    Silent,
}

pub struct ClipWorld {
    pub w: World,
    pub env: FeatureEnv,
    /// The device list; its edits raise the device-list events.
    pub peers: Arc<StaticPeers>,
    /// The device list as the feature reads it (see [`ClipPeers::vanish_after`]).
    pub directory: Arc<ClipPeers>,
    pub switches: Arc<PhoneSwitches>,
    pub sessions: Arc<FeatureSessions>,
    pub pasteboard: Arc<MemoryPasteboard>,
    /// The gate in front of `pasteboard` when [`Setup::gated`].
    pub gate: Option<Arc<GatedPasteboard>>,
    pub codec: Arc<FakeCodec>,
    pub clock: Arc<ManualClock>,
    pub clipboard: Arc<Clipboard>,
    pub events: broadcast::Receiver<ClipboardEvent>,
}

impl ClipWorld {
    pub async fn new() -> Self {
        Self::with(Setup::default()).await
    }

    pub async fn with(setup: Setup) -> Self {
        Self::try_with(setup).await.expect("clipboard installs")
    }

    /// As [`ClipWorld::with`], handing back an install failure.
    pub async fn try_with(setup: Setup) -> Result<Self, phonelink_engine::features::FeatureError> {
        let w = World::new(phonelink_engine::config::EngineConfig::default());
        phone_drm(&w, setup.drm.clone());
        if setup.connected {
            w.connect_phone().await;
        }
        if let Some(script) = setup.wake {
            w.waker.script(script);
        }

        let clock = Arc::new(ManualClock::fixed());
        let edges = Edges { lock: w.lock.clone(), clock: clock.clone(), ..memory_edges() };
        let sessions = FeatureSessions::new(w.sessions.clone());
        sessions.override_available(setup.available);

        let app = AppLayer::new(
            w.pair.config.clone(),
            edges.clone(),
            w.pair.pc.clone(),
            w.sessions.clone(),
            Arc::new(Ready),
            MemoryTap::new(),
        )
        .expect("App layer");

        let linked = if setup.linked { vec![phone_peer(setup.device)] } else { Vec::new() };
        let peers = StaticPeers::new(linked);
        peers.set_enrolled(setup.enrolled);
        let directory = ClipPeers::new(peers.clone());
        let switches = PhoneSwitches::open(&setup.stores).expect("phone switches");

        let env = FeatureEnv {
            config: w.pair.config.clone(),
            edges,
            app,
            sessions: sessions.clone(),
            peers: directory.clone(),
            switches: switches.clone(),
            notifier: RecordingNotifier::new(),
            responses: NotificationRouter::new(),
            stores: setup.stores,
            tap: MemoryTap::new(),
        };

        let pasteboard = setup.pasteboard.unwrap_or_default();
        let gate = setup.gated.then(|| GatedPasteboard::new(pasteboard.clone()));
        let edge: Arc<dyn ClipboardPasteboard> = match &gate {
            Some(gate) => gate.clone(),
            None => pasteboard.clone(),
        };

        let codec = FakeCodec::new();
        let clipboard =
            Clipboard::install(&env, setup.config, w.routes.msaep.clone(), w.routes.drm.clone(), edge, codec.clone())?;
        let events = clipboard.subscribe();
        settle().await;

        Ok(ClipWorld {
            w,
            env,
            peers,
            directory,
            switches,
            sessions,
            pasteboard,
            gate,
            codec,
            clock,
            clipboard,
            events,
        })
    }

    /// Turns the phone's toggle on and lets its feature-state request go out.
    pub async fn toggle_on(&self) {
        self.clipboard.set_phone_toggle(&phone_id(), true).unwrap();
        settle().await;
    }

    /// The events received so far.
    pub fn take_events(&mut self) -> Vec<ClipboardEvent> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            out.push(event);
        }
        out
    }

    /// The clipboard requests the PC sent to the phone so far.
    pub fn clipboard_requests(&self) -> Vec<(pb::DeviceResourceManagerMessage, cb::ClipboardRequestMessage)> {
        self.w.phone_drm.received().iter().map(decode_drm).collect()
    }

    /// The `request_type`s of the clipboard requests the PC sent so far.
    pub fn request_types(&self) -> Vec<i32> {
        self.clipboard_requests().into_iter().map(|(_, r)| r.request_type).collect()
    }

    /// The PC's tag-9 announcements so far.
    pub fn announcements(&self) -> Vec<Announcement> {
        self.w.phone_publish.received().iter().map(decode_announcement).collect()
    }

    pub fn up_to_date(&self) -> bool {
        self.clipboard.phone(&phone_id()).unwrap().expect("the phone").feature_status_up_to_date
    }
}

pub fn decode_drm(m: &InboundMessage) -> (pb::DeviceResourceManagerMessage, cb::ClipboardRequestMessage) {
    let drm = pb::DeviceResourceManagerMessage::decode(m.message.payload.as_slice()).expect("a DRM message");
    let request = cb::ClipboardRequestMessage::decode(drm.payload.as_slice()).expect("a clipboard request");
    (drm, request)
}

/// One PC announcement as the phone received it.
#[derive(Debug, Clone)]
pub struct Announcement {
    pub headers: Vec<(String, String)>,
    pub msaep: pb::MsaepMessage,
    pub payload: pb::PubSubPayload,
    pub message: cb::ClipboardResponseMessage,
}

pub fn decode_announcement(m: &InboundMessage) -> Announcement {
    let msaep = pb::MsaepMessage::decode(m.message.payload.as_slice()).expect("an MsaepMessage");
    let payload = pb::PubSubPayload::decode(msaep.payload.as_slice()).expect("a PubSubPayload");
    let message = cb::ClipboardResponseMessage::decode(payload.data.as_slice()).expect("a clipboard message");
    let headers = m.message.headers.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect();
    Announcement { headers, msaep, payload, message }
}

/// A `DeviceResourceManagerResponseMessage { payload, response_type = SUCCESS }` answer.
pub fn drm_answer(payload: &[u8]) -> TransportMessage {
    let body =
        pb::DeviceResourceManagerResponseMessage { payload: payload.to_vec(), response_type: Some(1) }.encode_to_vec();
    platform(&[("ms-content-type", "application/x-binary")], &body)
}

/// Scripts the phone's `/DeviceResourceManager` handler.
pub fn phone_drm(w: &World, drm: PhoneDrm) {
    match drm {
        PhoneDrm::Silent => w.phone_drm.set_reply(Box::new(|_| None)),
        PhoneDrm::Answer(content) => w.phone_drm.set_reply(Box::new(move |inbound| {
            let (_, request) = decode_drm(inbound);
            if request.request_type == cb::ClipboardRequestType::Content as i32 {
                Some(drm_answer(content.as_deref().unwrap_or_default()))
            } else {
                Some(drm_answer(b""))
            }
        })),
    }
}

/// `ClipboardResponseMessage { status, correlation_id }` as the phone writes it.
pub fn clipboard_message(status: cb::ClipboardResponseStatus, correlation_id: &str) -> Vec<u8> {
    cb::ClipboardResponseMessage { status: status as i32, correlation_id: correlation_id.into(), ..Default::default() }
        .encode_to_vec()
}

/// The phone's tag-9 publication carrying `additional` (and `data`).
pub fn phone_publication(message_id: &str, data: &[u8], additional: &[u8]) -> TransportMessage {
    phone_publication_tag(9, message_id, data, additional)
}

pub fn phone_publication_tag(tag: i32, message_id: &str, data: &[u8], additional: &[u8]) -> TransportMessage {
    let payload = pb::PubSubPayload { data: data.to_vec(), additional: additional.to_vec() }.encode_to_vec();
    let body = pb::MsaepMessage {
        message_tag: tag,
        dcg_client_id: phone_id().id,
        message_id: message_id.into(),
        payload,
        platform_major_version: 1,
        platform_minor_version: 1,
    }
    .encode_to_vec();
    platform(&[("_route", route::CONTEXT_PUBLISH), ("ms-content-type", "application/x-binary")], &body)
}

/// The phone publishes (one-way) and the PC processes it.
pub async fn phone_publishes(w: &World, message: TransportMessage) {
    let mut r = request(crate::support::platform_world::pc_id(), message);
    r.options.one_way = true;
    let _ = w.pair.phone.request(r).await;
    settle().await;
}

/// The phone announces a change with `correlation_id` in `additional`.
pub async fn phone_announces(w: &World, message_id: &str, correlation_id: &str) {
    let additional = clipboard_message(cb::ClipboardResponseStatus::ClipboardChange, correlation_id);
    phone_publishes(w, phone_publication(message_id, b"", &additional)).await;
}

/// A `ClipboardResponseMessage` content answer with one item.
pub fn content(item: cb::ClipboardItem) -> Vec<u8> {
    cb::ClipboardResponseMessage {
        clipboard_item: vec![item],
        status: cb::ClipboardResponseStatus::Ok as i32,
        correlation_id: CID.into(),
        ..Default::default()
    }
    .encode_to_vec()
}

pub fn item(item_type: cb::ClipboardItemType, text: Option<&str>, image: Option<&[u8]>) -> cb::ClipboardItem {
    cb::ClipboardItem {
        item_type: item_type as i32,
        text: text.map(str::to_owned),
        image_bytes: image.map(<[u8]>::to_vec),
        created_time: Some(prost_types::Timestamp { seconds: 1_760_000_000, nanos: 0 }),
    }
}

/// A phone `GET /clipboard` (path-addressed, wire type `UNKNOWN`) with `payload`.
pub fn phone_get(payload: &[u8]) -> TransportMessage {
    phone_drm_message(pb::ProtoDeviceResourceRequestType::Get as i32, "/clipboard", payload)
}

pub fn phone_drm_message(request_type: i32, path: &str, payload: &[u8]) -> TransportMessage {
    let body = pb::DeviceResourceManagerMessage {
        resource_type: pb::ProtoDeviceResourceType::Unknown as i32,
        request_type,
        payload: payload.to_vec(),
        resource_path: path.into(),
    }
    .encode_to_vec();
    platform(&[("_route", route::DEVICE_RESOURCE_MANAGER), ("ms-content-type", "application/x-binary")], &body)
}

/// A clipboard request from the phone.
pub fn clipboard_request(request_type: i32, correlation_id: &str) -> Vec<u8> {
    cb::ClipboardRequestMessage { request_type, correlation_id: correlation_id.into() }.encode_to_vec()
}

/// Sends `message` from the phone to the PC and waits up to 5 s for the answer.
pub async fn ask(w: &World, message: TransportMessage) -> PlatformResponse {
    let mut r = request(crate::support::platform_world::pc_id(), message);
    r.timeout = Duration::from_secs(5);
    w.pair.phone.request(r).await
}

/// The `ClipboardResponseMessage` inside a DRM answer (which must be a `SUCCESS` response).
pub fn answer_of(
    response: &PlatformResponse,
) -> (pb::DeviceResourceManagerResponseMessage, cb::ClipboardResponseMessage) {
    let message = response.message.as_ref().expect("an answer");
    let drm = pb::DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()).expect("a DRM response");
    let clipboard = cb::ClipboardResponseMessage::decode(drm.payload.as_slice()).expect("a clipboard response");
    (drm, clipboard)
}

/// Lets the watcher poll, debounce and announce.
pub async fn after_copy() {
    tokio::time::sleep(Duration::from_millis(600)).await;
    settle().await;
}
