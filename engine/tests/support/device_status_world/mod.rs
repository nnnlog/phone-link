//! Both device-status contracts on one session: the platform `World` (`support/platform_world`)
//! with an `AppLayer` on top of its PC router and per-partner connection, the World's phone selected,
//! and a phone-side App responder, so Phone Link's App traffic and CDEH's platform traffic travel
//! the same loopback. `tests/main.rs` declares it as `crate::support::device_status_world`, next to the `crate::support::platform_world` it
//! uses.
#![allow(dead_code)]

pub mod fixture;

use phonelink_engine::config::features::DeviceStatusConfig;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;

use phonelink_engine::app::{AccountGate, AppLayer};
use phonelink_engine::auth::PeerDevice;
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::Edges;
use phonelink_engine::edge::testing::{ManualClock, MemoryStore, memory_edges};
use phonelink_engine::features::device_status::edge::standins::{FixedPcStatus, PcState};
use phonelink_engine::features::device_status::{DeviceStatus, DeviceStatusEvent};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PhoneSwitches, StoreLocation};
use phonelink_engine::platform::{Router, route};
use phonelink_engine::proto::device_info::v1::{DeviceStatus as PbDeviceStatus, DeviceStatusResponseMessage};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::{AppInbound, PartnerSessionControl, PlatformMessenger, PlatformResponse};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, DeviceInfoItem, HeaderMap, MessageType, TraceContext, TransportMessage};

use crate::support::platform_world::{World, drm_answer, from_phone, phone_id, platform, settle};

/// The World phone's device-list entry: Android 14, `AsyncTrust`, `DEVICE_STATUS`, a current Link to
/// Windows.
pub fn phone_item() -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone_id().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        os_version: Some("14".into()),
        consent_version: Some(0),
        client_version: Some("1.24072.123.0".into()),
        capabilities: Some(vec!["DEVICE_STATUS".into(), "CLIPBOARD".into()]),
        custom_data: Some(r#"{"yppCapabilities":32}"#.into()),
        name: Some("Galaxy".into()),
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

type Answer = Box<dyn Fn(&str, &ValueSet) -> Option<ValueSet> + Send + Sync>;

/// The phone's App side: records every App request the PC sends and answers it.
pub struct PhoneApp {
    router: Weak<Router>,
    received: Mutex<Vec<(String, ValueSet)>>,
    answer: Mutex<Answer>,
}

impl PhoneApp {
    pub fn received(&self) -> Vec<(String, ValueSet)> {
        self.received.lock().unwrap().clone()
    }

    pub fn received_on(&self, route: &str) -> Vec<ValueSet> {
        self.received().into_iter().filter(|(r, _)| r == route).map(|(_, v)| v).collect()
    }

    /// How the phone answers `(route, payload)`; `None` sends nothing.
    pub fn set_answer(&self, answer: impl Fn(&str, &ValueSet) -> Option<ValueSet> + Send + Sync + 'static) {
        *self.answer.lock().unwrap() = Box::new(answer);
    }
}

impl AppInbound for PhoneApp {
    fn on_app_message(
        &self,
        source: DcgClient,
        _connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        let route = message.route().unwrap_or_default().to_string();
        let Ok(values) = valueset::decode(&message.payload) else { return };
        self.received.lock().unwrap().push((route.clone(), values.clone()));

        let Some(answer) = (self.answer.lock().unwrap())(&route, &values) else { return };
        let Some(router) = self.router.upgrade() else { return };

        tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            if let Some(r) = answer.i32("result") {
                headers.insert("result", r.to_string());
            }

            let response = TransportMessage::new(MessageType::App, headers, valueset::encode(&answer));
            router.respond(&source, &message, response, 50, trace).await;
        });
    }
}

pub struct StatusWorld {
    pub w: World,
    pub app: Arc<AppLayer>,
    pub phone_app: Arc<PhoneApp>,
    pub peers: Arc<StaticPeers>,
    pub switches: Arc<PhoneSwitches>,
    pub notifier: Arc<RecordingNotifier>,
    pub clock: Arc<ManualClock>,
    pub pc: Arc<FixedPcStatus>,
    pub env: FeatureEnv,
}

impl StatusWorld {
    /// The World with the App layer started over it and the World's phone (`item`) selected; the
    /// phone is not connected yet, and device status is not installed.
    pub fn new(item: DeviceInfoItem) -> Self {
        let w = World::new(EngineConfig::default());
        let store = Arc::new(MemoryStore::new());
        let clock = Arc::new(ManualClock::fixed());
        let edges = Edges { store, lock: w.lock.clone(), clock: clock.clone(), ..memory_edges() };
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
        app.set_phone(phone_id(), &item).unwrap();

        let phone_app = Arc::new(PhoneApp {
            router: Arc::downgrade(&w.pair.phone),
            received: Mutex::new(Vec::new()),
            answer: Mutex::new(Box::new(|_, _| Some(ValueSet::new().with("result", 0i32)))),
        });
        w.pair.phone.set_app_inbound(phone_app.clone());

        let peers = StaticPeers::new(vec![PeerDevice { client: phone_id(), info: item }]);
        let switches = PhoneSwitches::open(&StoreLocation::Memory).unwrap();
        let notifier = RecordingNotifier::new();
        let env = FeatureEnv {
            config: w.pair.config.clone(),
            edges,
            app: app.clone(),
            sessions: w.sessions.clone(),
            peers: peers.clone(),
            switches: switches.clone(),
            notifier: notifier.clone(),
            responses: NotificationRouter::new(),
            stores: StoreLocation::Memory,
            tap: MemoryTap::new(),
        };

        let pc = FixedPcStatus::new(PcState::default());
        StatusWorld { w, app, phone_app, peers, switches, notifier, clock, pc, env }
    }

    pub fn install(&self) -> Arc<DeviceStatus> {
        self.install_with(DeviceStatusConfig::default())
    }

    pub fn install_with(&self, config: DeviceStatusConfig) -> Arc<DeviceStatus> {
        DeviceStatus::install(
            &self.env,
            config,
            self.w.routes.msaep.clone(),
            self.w.routes.drm.clone(),
            self.pc.clone(),
        )
        .expect("device status installs")
    }

    /// The phone sends an App request with `contentType` and `keys` on `/legacy/phonecontent`;
    /// returns the PC's answer.
    pub async fn phone_sends(&self, content_type: &str, keys: ValueSet) -> ValueSet {
        let mut vs = ValueSet::new()
            .with("contentType", content_type)
            .with("contractVersion", 3.99f64)
            .with("correlationVector", "phone-cv-1");
        for (k, v) in keys.iter() {
            vs.insert(k, v.clone());
        }

        let message = TransportMessage::new(
            MessageType::App,
            HeaderMap::from_pairs([("_route", "/legacy/phonecontent")]).unwrap(),
            valueset::encode(&vs),
        );

        let response = self.w.pair.phone.request(from_phone(message)).await;
        let message = response.message.expect("the PC answers phone content");
        valueset::decode(&message.payload).unwrap()
    }

    /// The phone publishes `status` on MSAEP tag `tag` with `additional` = its encoding.
    pub async fn phone_publishes(&self, tag: i32, message_id: &str, additional: Vec<u8>) {
        let payload = pb::PubSubPayload { data: Vec::new(), additional }.encode_to_vec();
        let body = pb::MsaepMessage {
            message_tag: tag,
            dcg_client_id: "some-other-self-declared-id".into(),
            message_id: message_id.into(),
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

    /// The phone asks the PC on `/DeviceResourceManager`.
    pub async fn phone_drm_request(
        &self,
        resource_type: i32,
        request_type: i32,
        path: &str,
        payload: &[u8],
    ) -> PlatformResponse {
        let body = pb::DeviceResourceManagerMessage {
            resource_type,
            request_type,
            payload: payload.to_vec(),
            resource_path: path.into(),
        }
        .encode_to_vec();

        let mut request = from_phone(platform(
            &[("_route", route::DEVICE_RESOURCE_MANAGER), ("ms-content-type", "application/x-binary")],
            &body,
        ));
        request.timeout = Duration::from_secs(5);

        self.w.pair.phone.request(request).await
    }

    /// The phone answers every PC `/DeviceResourceManager` request with `answer` as the
    /// `DeviceResourceManagerResponseMessage` payload (`SUCCESS`).
    pub fn phone_answers_pulls(&self, answer: Vec<u8>) {
        self.w.phone_drm.set_reply(Box::new(move |_| Some(drm_answer(&answer, Some(1)))));
    }

    /// The PC's `/DeviceResourceManager` requests the phone received, decoded.
    pub fn pulls(&self) -> Vec<pb::DeviceResourceManagerMessage> {
        self.w
            .phone_drm
            .received()
            .iter()
            .map(|m| pb::DeviceResourceManagerMessage::decode(m.message.payload.as_slice()).unwrap())
            .collect()
    }

    pub fn events(rx: &mut tokio::sync::broadcast::Receiver<DeviceStatusEvent>) -> Vec<DeviceStatusEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }
}

/// A `DeviceStatusResponseMessage` with `status` and `info`.
pub fn status_message(status: i32, info: Option<PbDeviceStatus>) -> Vec<u8> {
    DeviceStatusResponseMessage { status, info }.encode_to_vec()
}

/// The page's illustrative `DeviceStatusResponseMessage` (24 bytes): battery 57, charging, BT 12,
/// DND 1, signal 4 of 5, data 2, ringer 2, Wi-Fi 3, on Wi-Fi.
pub const PAGE_STATUS_VECTOR: [u8; 24] = [
    0x08, 0x01, 0x12, 0x14, 0x08, 0x39, 0x10, 0x01, 0x20, 0x0C, 0x28, 0x01, 0x38, 0x04, 0x40, 0x05, 0x48, 0x02, 0x50,
    0x02, 0x58, 0x03, 0x60, 0x01,
];

/// The page's `GET /devicestatus` body (19 bytes).
pub const PAGE_PULL_VECTOR: [u8; 19] =
    [0x08, 0x04, 0x10, 0x01, 0x22, 0x0D, 0x2F, 0x64, 0x65, 0x76, 0x69, 0x63, 0x65, 0x73, 0x74, 0x61, 0x74, 0x75, 0x73];
