//! New-photo notifications over the platform `World` (`support/platform_world`): an App layer on
//! the PC router (the feature environment needs one), the World's phone in the account's device list,
//! a recording notifier, and a scripted phone for `/DeviceResourceManager`. `tests/main.rs`
//! declares it as `crate::support::remote_snipping_world`, next to the `crate::support::platform_world` it uses.
#![allow(dead_code)]

use phonelink_engine::config::features::RemoteSnippingConfig;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use prost::Message;

use phonelink_engine::app::{AccountGate, AppLayer};
use phonelink_engine::auth::PeerDevice;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::Edges;
use phonelink_engine::edge::testing::{ManualClock, MemoryStore, memory_edges};
use phonelink_engine::features::remote_snipping::{RemoteSnipping, RemoteSnippingEvent};
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::shared::peers::testing::StaticPeers;
use phonelink_engine::features::{FeatureEnv, NotificationRouter, PhoneSwitches, StoreLocation};
use phonelink_engine::platform::route;
use phonelink_engine::proto::permissions::v1 as perm;
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::proto::remotesnipping::v1 as rs;
use phonelink_engine::seams::{PartnerSessionControl, PlatformMessenger};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, DeviceInfoItem, TransportMessage};

use crate::support::platform_world::{DrmAnswer, World, drm_answer, from_phone, phone_id, platform, settle};

/// The World phone's device-list entry: Android, a current Link to Windows, named "Galaxy".
pub fn phone_item() -> DeviceInfoItem {
    DeviceInfoItem {
        id: phone_id().id,
        is_linked: Some(true),
        os_name: Some("Android".into()),
        os_version: Some("14".into()),
        consent_version: Some(0),
        client_version: Some("1.24072.123.0".into()),
        capabilities: Some(vec!["DEVICE_STATUS".into()]),
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

pub struct SnipWorld {
    pub w: World,
    pub app: Arc<AppLayer>,
    pub peers: Arc<StaticPeers>,
    pub switches: Arc<PhoneSwitches>,
    pub notifier: Arc<RecordingNotifier>,
    pub clock: Arc<ManualClock>,
    pub responses: Arc<NotificationRouter>,
    pub env: FeatureEnv,
    answer: Arc<Mutex<DrmAnswer>>,
}

impl SnipWorld {
    /// The World with an App layer and the phone (`item`) in the device list; the phone is not
    /// connected yet and the feature is not installed. The phone answers every `MEDIA` request with
    /// an empty reply until a test says otherwise.
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

        let answer: Arc<Mutex<DrmAnswer>> =
            Arc::new(Mutex::new(Box::new(|_| Some(media_answer(rs::MediaResponseMessage::default())))));
        let a = answer.clone();
        w.phone_drm.set_reply(Box::new(move |inbound| {
            let request = pb::DeviceResourceManagerMessage::decode(inbound.message.payload.as_slice()).ok()?;
            (a.lock().unwrap())(&request)
        }));

        SnipWorld { w, app, peers, switches, notifier, clock, responses, env, answer }
    }

    pub fn install(&self) -> Arc<RemoteSnipping> {
        self.install_with(RemoteSnippingConfig::default())
    }

    pub fn install_with(&self, config: RemoteSnippingConfig) -> Arc<RemoteSnipping> {
        RemoteSnipping::install(&self.env, config, self.w.routes.msaep.clone(), self.w.routes.drm.clone())
            .expect("remote snipping installs")
    }

    /// How the phone answers the PC's `/DeviceResourceManager` requests from now on.
    pub fn answer(
        &self,
        f: impl Fn(&pb::DeviceResourceManagerMessage) -> Option<TransportMessage> + Send + Sync + 'static,
    ) {
        *self.answer.lock().unwrap() = Box::new(f);
    }

    /// The phone answers capability requests with `capable` / `has_permissions`, thumbnails with
    /// `thumbnail` and photos with `photo` (absent when `None`).
    pub fn phone_serves(
        &self,
        capable: bool,
        has_permissions: bool,
        thumbnail: Option<Vec<u8>>,
        photo: Option<Vec<u8>>,
    ) {
        self.answer(move |m| {
            if m.resource_type != pb::ProtoDeviceResourceType::Media as i32 {
                return None;
            }

            let request = rs::MediaRequestMessage::decode(m.payload.as_slice()).ok()?;
            let mut reply = rs::MediaResponseMessage {
                has_permissions,
                status: rs::MediaResponseStatus::Ok as i32,
                ..Default::default()
            };

            match rs::MediaRequestType::try_from(request.request_type).ok()? {
                rs::MediaRequestType::Capabilities => {
                    if capable {
                        reply.capabilities = vec![rs::MediaCapability::NewMediaNotifications as i32];
                    }
                }
                rs::MediaRequestType::Thumbnail => {
                    reply.item = Some(rs::MediaMetadata {
                        media_id: request.media_id?,
                        thumbnail: thumbnail.clone(),
                        ..Default::default()
                    })
                }
                rs::MediaRequestType::Photo => {
                    reply.item = Some(rs::MediaMetadata {
                        media_id: request.media_id?,
                        photo: photo.clone(),
                        ..Default::default()
                    })
                }
                rs::MediaRequestType::Unspecified => return None,
            }

            Some(media_answer(reply))
        });
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

    /// The `MEDIA` requests' `MediaRequestMessage`s, in order.
    pub fn media_requests(&self) -> Vec<rs::MediaRequestMessage> {
        self.requests()
            .into_iter()
            .filter(|(m, _)| m.resource_type == pb::ProtoDeviceResourceType::Media as i32)
            .map(|(m, _)| rs::MediaRequestMessage::decode(m.payload.as_slice()).unwrap())
            .collect()
    }

    /// The phone publishes `additional` on MSAEP `tag`.
    pub async fn publish(&self, tag: i32, message_id: &str, additional: Vec<u8>) {
        let payload = pb::PubSubPayload { data: Vec::new(), additional }.encode_to_vec();
        let body = pb::MsaepMessage {
            message_tag: tag,
            dcg_client_id: "self-declared".into(),
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

    /// A `NEW_PHOTO` publication of `media_id`, last modified `age` before the world's clock.
    pub async fn new_photo(&self, message_id: &str, media_id: i64, age: Duration) {
        self.publish(6, message_id, new_photo(media_id, Some(self.clock_now() - age))).await;
    }

    pub fn clock_now(&self) -> SystemTime {
        use phonelink_engine::edge::Clock;
        self.clock.now()
    }

    /// Events received so far.
    pub fn events(rx: &mut tokio::sync::broadcast::Receiver<RemoteSnippingEvent>) -> Vec<RemoteSnippingEvent> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }
}

/// `MediaChangeMessage { NEW_PHOTO, [ { media_id, date_modified } ] }`.
pub fn new_photo(media_id: i64, modified: Option<SystemTime>) -> Vec<u8> {
    rs::MediaChangeMessage {
        change_type: rs::MediaChangeType::NewPhoto as i32,
        changed_media: vec![rs::MediaMetadata {
            media_id,
            date_modified: modified.map(timestamp),
            date_taken: modified.map(timestamp),
            ..Default::default()
        }],
    }
    .encode_to_vec()
}

pub fn timestamp(t: SystemTime) -> prost_types::Timestamp {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).unwrap();
    prost_types::Timestamp { seconds: d.as_secs() as i64, nanos: d.subsec_nanos() as i32 }
}

/// A tag-3 `PermissionsResponseMessage` listing `photos`.
pub fn permission_publication(photos: bool) -> Vec<u8> {
    perm::PermissionsResponseMessage {
        request_status: perm::PermissionsRequestStatus::Accepted as i32,
        feature_permissions: if photos { vec![perm::FeaturePermissions::Photos as i32] } else { vec![] },
        ..Default::default()
    }
    .encode_to_vec()
}

/// A `SUCCESS` `/DeviceResourceManager` answer carrying `reply`.
pub fn media_answer(reply: rs::MediaResponseMessage) -> TransportMessage {
    drm_answer(&reply.encode_to_vec(), Some(1))
}
