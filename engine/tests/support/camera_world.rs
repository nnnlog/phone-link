//! The connected camera over the platform `World` (`support/platform_world`): an App layer on the PC
//! router, the World's phone in the account's device list, a recording camera, a scripted phone for
//! `/DeviceResourceManager`, and stream channels that need no connection: each channel the feature
//! opens is a fake whose sends are recorded and whose inbound side the test drives.
//! `tests/main.rs` declares it as `crate::support::camera_world`.
#![allow(dead_code)]

use phonelink_engine::config::features::ConnectedCameraConfig;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio::sync::mpsc;

use phonelink_engine::app::{AccountGate, AppLayer};
use phonelink_engine::auth::PeerDevice;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::Edges;
use phonelink_engine::edge::testing::{ManualClock, memory_edges};
use phonelink_engine::features::connected_camera::channel::StreamSendError;
use phonelink_engine::features::connected_camera::edge::testing::RecordingCamera;
use phonelink_engine::features::connected_camera::{
    CameraChannels, ConnectedCamera, ConnectedCameraEvent, OpenedStream, StreamChannel, StreamInbound, StreamOpenError,
};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PhoneSwitches, StoreLocation};
use phonelink_engine::platform::{ContextHandle, route};
use phonelink_engine::proto::permissions::v1 as perm;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::proto::streaming::v1::StreamingConfigurationMessage;
use phonelink_engine::seams::{PartnerSessionControl, PlatformMessenger};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, DeviceInfoItem, TraceContext, TransportMessage};

use crate::support::platform_world::{DrmAnswer, World, phone_id, platform, settle};
use crate::support::platform_world::{drm_answer, from_phone};

/// The World phone's device-list entry: Android, a current Link to Windows, camera streaming, "Pixel".
pub fn phone_item() -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone_id().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        os_version: Some("14".into()),
        consent_version: Some(0),
        client_version: Some("1.24072.123.0".into()),
        capabilities: Some(vec!["DEVICE_STATUS".into(), "CAMERA_STREAMING".into()]),
        name: Some("Pixel".into()),
        ..Default::default()
    }
}

struct ReadyAccount;

#[async_trait]
impl AccountGate for ReadyAccount {
    async fn is_ready(&self, _phone: &DcgClient) -> bool {
        true
    }
}

/// A fake stream channel: what the PC sent on it, and its video calls.
#[derive(Default)]
pub struct FakeStream {
    pub sent: Mutex<Vec<(String, serde_json::Value)>>,
    pub video_started: AtomicUsize,
    pub acks: Mutex<Vec<u32>>,
    pub closed: AtomicBool,
    /// The real-time metrics the channel reports.
    pub metrics: Mutex<Option<String>>,
}

impl FakeStream {
    pub fn sent(&self) -> Vec<(String, serde_json::Value)> {
        self.sent.lock().unwrap().clone()
    }

    /// The actions sent: `(action, cameraError)`.
    pub fn actions(&self) -> Vec<(i64, serde_json::Value)> {
        self.sent()
            .into_iter()
            .filter(|(s, _)| s == "/streaming/action")
            .map(|(_, v)| (v["action"].as_i64().unwrap(), v["cameraError"].clone()))
            .collect()
    }
}

impl StreamChannel for FakeStream {
    fn send(&self, scope: &str, body: &[u8]) -> Result<(), StreamSendError> {
        let v = serde_json::from_slice(body).map_err(|e| StreamSendError(Box::new(e)))?;
        self.sent.lock().unwrap().push((scope.to_owned(), v));
        Ok(())
    }
    fn start_video(&self) {
        self.video_started.fetch_add(1, Ordering::SeqCst);
    }
    fn frame_presented(&self, frame_id: u32, _time_us: u64) {
        self.acks.lock().unwrap().push(frame_id);
    }
    fn realtime_metrics(&self) -> Option<String> {
        self.metrics.lock().unwrap().clone()
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// A stream the feature opened: the fake and the phone's side of its inbound.
pub struct Stream {
    pub fake: Arc<FakeStream>,
    pub phone: mpsc::UnboundedSender<StreamInbound>,
}

impl Stream {
    pub fn message(&self, scope: &str, body: serde_json::Value) {
        let body = serde_json::to_vec(&body).unwrap();
        self.phone.send(StreamInbound::Message { scope: scope.into(), body }).unwrap();
    }

    pub fn raw(&self, scope: &str, body: &[u8]) {
        self.phone.send(StreamInbound::Message { scope: scope.into(), body: body.to_vec() }).unwrap();
    }

    pub fn action(&self, action: i32, error: Option<i32>, config: Option<serde_json::Value>) {
        self.message(
            "/streaming/action",
            serde_json::json!({"scope":"/streaming/action","correlationId":"c","action":action,
                "cameraError":error,"cameraConfigPayload":config}),
        );
    }

    pub fn frame(&self, id: u32, data: &[u8]) {
        self.phone
            .send(StreamInbound::Frame(phonelink_engine::nano::channel::video::VideoFrame {
                frame_id: id,
                timestamp_us: 0,
                keyframe: true,
                stream_reset: false,
                data: data.to_vec(),
                metadata: Vec::new(),
            }))
            .unwrap();
    }

    pub fn close(&self) {
        let _ = self.phone.send(StreamInbound::Closed);
    }
}

/// [`CameraChannels`] over fakes.
pub struct FakeChannels {
    opened: mpsc::UnboundedSender<Stream>,
    /// The next opens fail with these, in order.
    pub refusals: Mutex<Vec<StreamOpenError>>,
    pub handles: Mutex<Vec<ContextHandle>>,
}

#[async_trait]
impl CameraChannels for FakeChannels {
    async fn open(&self, handle: &ContextHandle, _trace: &TraceContext) -> Result<OpenedStream, StreamOpenError> {
        self.handles.lock().unwrap().push(handle.clone());

        let refusal = {
            let mut r = self.refusals.lock().unwrap();
            if r.is_empty() { None } else { Some(r.remove(0)) }
        };
        if let Some(e) = refusal {
            return Err(e);
        }

        let fake = Arc::new(FakeStream::default());
        let (tx, inbound) = mpsc::unbounded_channel();
        let _ = self.opened.send(Stream { fake: fake.clone(), phone: tx });
        Ok(OpenedStream { channel: fake, inbound })
    }
}

pub struct CameraWorld {
    pub w: World,
    pub app: Arc<AppLayer>,
    pub peers: Arc<StaticPeers>,
    pub switches: Arc<PhoneSwitches>,
    pub env: FeatureEnv,
    pub camera: Arc<RecordingCamera>,
    pub channels: Arc<FakeChannels>,
    pub opened: tokio::sync::Mutex<mpsc::UnboundedReceiver<Stream>>,
    answer: Arc<Mutex<DrmAnswer>>,
}

impl CameraWorld {
    /// The World with an App layer and the phone (`item`) in the device list, connected; the feature
    /// is not installed. The phone answers the sync `SUCCESS` and grants the camera permission.
    pub async fn new(item: DeviceInfoItem) -> Self {
        let w = World::new(EngineConfig::default());
        let edges = Edges { lock: w.lock.clone(), clock: Arc::new(ManualClock::fixed()), ..memory_edges() };

        let app = AppLayer::new(
            w.pair.config.clone(),
            edges.clone(),
            w.pair.pc.clone() as Arc<dyn PlatformMessenger>,
            w.sessions.clone() as Arc<dyn PartnerSessionControl>,
            Arc::new(ReadyAccount),
            MemoryTap::new(),
        )
        .unwrap();
        w.pair.pc.set_app_inbound(app.clone());
        app.start().unwrap();

        let peers = StaticPeers::new(vec![PeerDevice { client: phone_id(), info: item }]);
        let switches = PhoneSwitches::open(&StoreLocation::Memory).unwrap();
        let env = FeatureEnv {
            config: w.pair.config.clone(),
            edges,
            app: app.clone(),
            sessions: w.sessions.clone(),
            peers: peers.clone(),
            switches: switches.clone(),
            notifier: RecordingNotifier::new(),
            responses: NotificationRouter::new(),
            stores: StoreLocation::Memory,
            tap: MemoryTap::new(),
        };

        let answer: Arc<Mutex<DrmAnswer>> = Arc::new(Mutex::new(Box::new(phone_answers)));
        let a = answer.clone();
        w.phone_drm.set_reply(Box::new(move |inbound| {
            let request = pb::DeviceResourceManagerMessage::decode(inbound.message.payload.as_slice()).ok()?;
            (a.lock().unwrap())(&request)
        }));

        let (tx, rx) = mpsc::unbounded_channel();
        let channels = Arc::new(FakeChannels { opened: tx, refusals: Mutex::default(), handles: Mutex::default() });
        w.connect_phone().await;

        CameraWorld {
            w,
            app,
            peers,
            switches,
            env,
            camera: RecordingCamera::new(),
            channels,
            opened: tokio::sync::Mutex::new(rx),
            answer,
        }
    }

    /// Installs the feature with short client-signal pauses.
    pub fn install(&self) -> Arc<ConnectedCamera> {
        self.install_with(ConnectedCameraConfig {
            client_signal_pause: Duration::from_millis(10),
            ..ConnectedCameraConfig::default()
        })
    }

    pub fn install_with(&self, config: ConnectedCameraConfig) -> Arc<ConnectedCamera> {
        ConnectedCamera::install(&self.env, config, &self.w.routes, self.channels.clone(), self.camera.clone())
            .expect("connected camera installs")
    }

    /// How the phone answers the PC's `/DeviceResourceManager` requests from now on.
    pub fn answer(
        &self,
        f: impl Fn(&pb::DeviceResourceManagerMessage) -> Option<TransportMessage> + Send + Sync + 'static,
    ) {
        *self.answer.lock().unwrap() = Box::new(f);
    }

    /// The PC's `/DeviceResourceManager` requests: `(message, ypTriggerId)`.
    pub fn requests(&self) -> Vec<(pb::DeviceResourceManagerMessage, String)> {
        self.w
            .phone_drm
            .received()
            .iter()
            .map(|m| {
                let trigger = m.trace.trace_state.iter().find(|(k, _)| k == "ypTriggerId").map(|(_, v)| v.clone());
                (
                    pb::DeviceResourceManagerMessage::decode(m.message.payload.as_slice()).unwrap(),
                    trigger.unwrap_or_default(),
                )
            })
            .collect()
    }

    /// The `CAMERA_METADATA` syncs sent.
    pub fn syncs(&self) -> usize {
        self.requests()
            .iter()
            .filter(|(m, _)| {
                m.resource_type == pb::ProtoDeviceResourceType::CameraMetadata as i32
                    && m.request_type == pb::ProtoDeviceResourceRequestType::Sync as i32
            })
            .count()
    }

    /// The phone publishes `tag` with these payload fields.
    pub async fn publish(&self, tag: i32, data: Vec<u8>, additional: Vec<u8>) {
        let payload = pb::PubSubPayload { data, additional }.encode_to_vec();
        let body = pb::MsaepMessage {
            message_tag: tag,
            dcg_client_id: "self-declared".into(),
            message_id: phonelink_engine::types::new_guid_lower(),
            payload,
            platform_major_version: 1,
            platform_minor_version: 1,
        }
        .encode_to_vec();

        let mut request = from_phone(platform(
            &[("_route", route::CONTEXT_PUBLISH), ("ms-content-type", "application/x-binary")],
            &body,
        ));
        request.options.one_way = true;

        self.w.pair.phone.request(request).await;
        settle().await;
    }

    /// The phone publishes tag 2.
    pub async fn publish_camera(&self, capable: bool, camera: bool, notifications: bool) {
        let m = StreamingConfigurationMessage {
            is_capable: capable as u32,
            permissions_granted: camera as u32,
            core_permissions_granted: notifications as u32,
            permissions_permanently_denied: false,
        };
        self.publish(2, m.encode_to_vec(), Vec::new()).await;
    }

    /// The phone publishes tag 3: `ACCEPTED` with these grants.
    pub async fn publish_permissions(&self, core: bool, camera: bool) {
        let m = perm::PermissionsResponseMessage {
            request_status: perm::PermissionsRequestStatus::Accepted as i32,
            core_permissions: if core { vec![1] } else { vec![] },
            feature_permissions: if camera { vec![1] } else { vec![] },
            ..Default::default()
        };
        self.publish(3, Vec::new(), m.encode_to_vec()).await;
    }

    /// The next channel the feature opens.
    pub async fn next_stream(&self) -> Stream {
        tokio::time::timeout(Duration::from_secs(30), self.opened.lock().await.recv()).await.unwrap().unwrap()
    }

    /// No channel was opened.
    pub fn no_stream(&self) -> bool {
        self.opened.try_lock().unwrap().try_recv().is_err()
    }

    /// Events received so far.
    pub fn events(rx: &mut tokio::sync::broadcast::Receiver<ConnectedCameraEvent>) -> Vec<ConnectedCameraEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }
}

/// The phone answers the sync and the auto-prompt `SUCCESS` (`CannotBeEmpty`) and grants the camera
/// and notification permissions.
pub fn phone_answers(m: &pb::DeviceResourceManagerMessage) -> Option<TransportMessage> {
    if m.resource_type == pb::ProtoDeviceResourceType::Permissions as i32 {
        Some(permission_answer(true, true, None))
    } else if m.resource_type == pb::ProtoDeviceResourceType::CameraMetadata as i32 {
        Some(drm_answer(b"CannotBeEmpty", Some(1)))
    } else {
        None
    }
}

/// An `ACCEPTED` permission reply.
pub fn permission_answer(notifications: bool, camera: bool, qr: Option<&str>) -> TransportMessage {
    let reply = perm::PermissionsResponseMessage {
        request_status: perm::PermissionsRequestStatus::Accepted as i32,
        core_permissions: if notifications { vec![1] } else { vec![] },
        feature_permissions: if camera { vec![1] } else { vec![] },
        qr_code_uri: qr.map(str::to_owned),
        ..Default::default()
    };
    drm_answer(&reply.encode_to_vec(), Some(1))
}
