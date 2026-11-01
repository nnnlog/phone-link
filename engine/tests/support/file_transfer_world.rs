//! File transfer over the platform `World` (`support/platform_world`): an App layer on the PC router
//! (the feature environment needs one), the World's phone in the account's device list, a recording
//! notifier, a scripted phone for `/DeviceResourceManager`, and channels that need no connection: each
//! channel the feature opens is an in-memory `NamedStreams` pair whose phone end the test drives.
//! `tests/main.rs` declares it as `crate::support::file_transfer_world`, next to the `crate::support::platform_world` it uses.
#![allow(dead_code)]

use phonelink_engine::config::features::FileTransferConfig;
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
use phonelink_engine::features::file_transfer::{FileTransfer, FileTransferEvent};
use phonelink_engine::features::shared::nano_channels::{
    ChannelOpenError, OpenedChannel, ShareChannels, TransferChannel,
};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PhoneSwitches, StoreLocation};
use phonelink_engine::nano::channel::blob::BlobChannel;
use phonelink_engine::nano::channel::message::MessageChannel;
use phonelink_engine::nano::channel::testing::pair;
use phonelink_engine::nano::device::{PlatformChannel, PlatformChannelType};
use phonelink_engine::nano::named_streams::NamedStreams;
use phonelink_engine::platform::{ContextHandle, route};
use phonelink_engine::proto::permissions::v1 as perm;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::{PartnerSessionControl, PlatformMessenger, PlatformResponse};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, DeviceInfoItem, TraceContext, TransportMessage};

use crate::support::platform_world::{DrmAnswer, World, drm_answer, from_phone, phone_id, platform, settle};

/// The World phone's device-list entry: Android, a current Link to Windows, file transfer, "Galaxy".
pub fn phone_item() -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone_id().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        os_version: Some("14".into()),
        consent_version: Some(0),
        client_version: Some("1.24072.123.0".into()),
        capabilities: Some(vec!["DEVICE_STATUS".into(), "FILE_TRANSFER".into()]),
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

/// A channel the feature opened: its scenario and the phone's end.
pub struct Opened {
    pub scenario: String,
    pub phone: Arc<NamedStreams>,
    /// The control messages the phone receives, listened to from the channel's creation (as the phone
    /// does).
    pub control: mpsc::UnboundedReceiver<phonelink_engine::nano::channel::message::InboundMessage>,
}

/// [`ShareChannels`] over in-memory channel pairs.
pub struct FakeChannels {
    opened: mpsc::UnboundedSender<Opened>,
    /// The next opens fail with these, in order.
    pub refusals: Mutex<Vec<ChannelOpenError>>,
    pub handles: Mutex<Vec<ContextHandle>>,
}

async fn named_streams_pair(scenario: &str) -> (Arc<NamedStreams>, Arc<NamedStreams>) {
    let ((m_pc, _), (m_ph, _)) = pair(&format!("x,1,4,ScenarioId={scenario}"), 1200);
    let ((b_pc, _), (b_ph, _)) = pair(&format!("x,2,5,ScenarioId={scenario}"), 1200);
    let (m_pc, m_ph, b_pc, b_ph) = tokio::join!(
        MessageChannel::open(m_pc),
        MessageChannel::open(m_ph),
        BlobChannel::open(b_pc),
        BlobChannel::open(b_ph)
    );

    let channel = |message, blob| PlatformChannel {
        kind: PlatformChannelType::NamedStreams,
        scenario_id: scenario.to_owned(),
        channel_id: "c".into(),
        message,
        blob: Some(blob),
        video: None,
        input: None,
        audio: None,
    };

    let pc = NamedStreams::new(channel(m_pc.unwrap(), b_pc.unwrap()), Duration::from_secs(30)).unwrap();
    let phone = NamedStreams::new(channel(m_ph.unwrap(), b_ph.unwrap()), Duration::from_secs(30)).unwrap();
    (pc, phone)
}

#[async_trait]
impl ShareChannels for FakeChannels {
    async fn open(
        &self,
        handle: &ContextHandle,
        scenario: &str,
        _trace: &TraceContext,
    ) -> Result<OpenedChannel, ChannelOpenError> {
        self.handles.lock().unwrap().push(handle.clone());

        let refusal = {
            let mut r = self.refusals.lock().unwrap();
            if r.is_empty() { None } else { Some(r.remove(0)) }
        };
        if let Some(e) = refusal {
            return Err(e);
        }

        let (pc, phone) = named_streams_pair(scenario).await;
        let pc_control = pc.messages("/fileTransfer/platform");
        let control = phone.messages("/fileTransfer/platform");
        let _ = self.opened.send(Opened { scenario: scenario.to_owned(), phone, control });
        Ok(OpenedChannel { channel: TransferChannel::new(pc, None), control: pc_control })
    }
}

pub struct ShareWorld {
    pub w: World,
    pub app: Arc<AppLayer>,
    pub peers: Arc<StaticPeers>,
    pub switches: Arc<PhoneSwitches>,
    pub notifier: Arc<RecordingNotifier>,
    pub responses: Arc<NotificationRouter>,
    pub env: FeatureEnv,
    pub channels: Arc<FakeChannels>,
    pub opened: tokio::sync::Mutex<mpsc::UnboundedReceiver<Opened>>,
    pub folder: tempfile::TempDir,
    answer: Arc<Mutex<DrmAnswer>>,
}

impl ShareWorld {
    /// The World with an App layer and the phone (`item`) in the device list, connected; the feature
    /// is not installed. The phone grants the files permission and answers the probe `OK`.
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
        let notifier = RecordingNotifier::new();
        let responses = NotificationRouter::new();
        let env = FeatureEnv {
            config: w.pair.config.clone(),
            edges,
            app: app.clone(),
            sessions: w.sessions.clone(),
            peers: peers.clone(),
            switches: switches.clone(),
            notifier: notifier.clone(),
            responses: responses.clone(),
            stores: StoreLocation::Memory,
            tap: MemoryTap::new(),
        };

        let answer: Arc<Mutex<DrmAnswer>> = Arc::new(Mutex::new(Box::new(grant_all)));
        let a = answer.clone();
        w.phone_drm.set_reply(Box::new(move |inbound| {
            let request = pb::DeviceResourceManagerMessage::decode(inbound.message.payload.as_slice()).ok()?;
            (a.lock().unwrap())(&request)
        }));

        let (tx, rx) = mpsc::unbounded_channel();
        let channels = Arc::new(FakeChannels { opened: tx, refusals: Mutex::default(), handles: Mutex::default() });
        w.connect_phone().await;

        ShareWorld {
            w,
            app,
            peers,
            switches,
            notifier,
            responses,
            env,
            channels,
            opened: tokio::sync::Mutex::new(rx),
            folder: tempfile::tempdir().unwrap(),
            answer,
        }
    }

    pub fn install(&self) -> Arc<FileTransfer> {
        self.install_with(FileTransferConfig::default())
    }

    /// Installs the feature with `config`, its default folder the world's temporary folder.
    pub fn install_with(&self, config: FileTransferConfig) -> Arc<FileTransfer> {
        let config = FileTransferConfig { default_folder: Some(self.folder.path().join("Mobile Devices")), ..config };
        FileTransfer::install(&self.env, config, &self.w.routes, self.channels.clone()).expect("file transfer installs")
    }

    /// How the phone answers the PC's `/DeviceResourceManager` requests from now on.
    pub fn answer(
        &self,
        f: impl Fn(&pb::DeviceResourceManagerMessage) -> Option<TransportMessage> + Send + Sync + 'static,
    ) {
        *self.answer.lock().unwrap() = Box::new(f);
    }

    /// The PC's `/DeviceResourceManager` requests the phone received, decoded, with their trace state.
    pub fn requests(&self) -> Vec<(pb::DeviceResourceManagerMessage, Vec<(String, String)>)> {
        self.w
            .phone_drm
            .received()
            .iter()
            .map(|m| {
                (
                    pb::DeviceResourceManagerMessage::decode(m.message.payload.as_slice()).unwrap(),
                    m.trace.trace_state.clone(),
                )
            })
            .collect()
    }

    /// The `PERMISSIONS` requests: `(request_type, ypTriggerId)`.
    pub fn permission_requests(&self) -> Vec<(i32, String)> {
        self.requests()
            .into_iter()
            .filter(|(m, _)| m.resource_type == pb::ProtoDeviceResourceType::Permissions as i32)
            .map(|(m, t)| (m.request_type, t.iter().find(|(k, _)| k == "ypTriggerId").unwrap().1.clone()))
            .collect()
    }

    /// The phone publishes its files permission on tag 3 (which delivers its context handle).
    pub async fn publish_permission(&self) {
        let additional = perm::PermissionsResponseMessage {
            request_status: perm::PermissionsRequestStatus::Accepted as i32,
            feature_permissions: vec![perm::FeaturePermissions::Files as i32],
            ..Default::default()
        }
        .encode_to_vec();

        let payload = pb::PubSubPayload { data: Vec::new(), additional }.encode_to_vec();
        let body = pb::MsaepMessage {
            message_tag: 3,
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

    /// The phone's `/DeviceResourceManager` request to the PC; `timeout` bounds the wait for the answer.
    pub async fn phone_drm(
        &self,
        request_type: i32,
        path: &str,
        payload: &[u8],
        timeout: Duration,
    ) -> PlatformResponse {
        let body = pb::DeviceResourceManagerMessage {
            resource_type: pb::ProtoDeviceResourceType::Unknown as i32,
            request_type,
            payload: payload.to_vec(),
            resource_path: path.into(),
        }
        .encode_to_vec();

        let mut r = from_phone(platform(
            &[("_route", route::DEVICE_RESOURCE_MANAGER), ("ms-content-type", "application/x-binary")],
            &body,
        ));
        r.timeout = timeout;

        self.w.pair.phone.request(r).await
    }

    /// The next channel the feature opens.
    pub async fn next_channel(&self) -> Opened {
        tokio::time::timeout(Duration::from_secs(300), self.opened.lock().await.recv()).await.unwrap().unwrap()
    }

    /// Events received so far.
    pub fn events(rx: &mut tokio::sync::broadcast::Receiver<FileTransferEvent>) -> Vec<FileTransferEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }
}

/// The phone grants the files permission and answers the probe `OK`.
pub fn grant_all(m: &pb::DeviceResourceManagerMessage) -> Option<TransportMessage> {
    if m.resource_type == pb::ProtoDeviceResourceType::Permissions as i32 {
        Some(permission_answer(true))
    } else if m.resource_type == pb::ProtoDeviceResourceType::FileShare as i32 {
        let ok = phonelink_engine::proto::file_transfer::v1::FileTransferResponseMessage { status: 1 };
        Some(drm_answer(&ok.encode_to_vec(), Some(1)))
    } else {
        None
    }
}

/// A `SUCCESS` permission reply, with or without `FILES`.
pub fn permission_answer(files: bool) -> TransportMessage {
    let reply = perm::PermissionsResponseMessage {
        request_status: perm::PermissionsRequestStatus::Accepted as i32,
        feature_permissions: if files { vec![perm::FeaturePermissions::Files as i32] } else { vec![] },
        ..Default::default()
    };
    drm_answer(&reply.encode_to_vec(), Some(1))
}
