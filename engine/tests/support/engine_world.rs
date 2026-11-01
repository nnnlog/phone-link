//! An engine-level world: a fake DCG server (`support/auth_world`), a fake relay hub
//! (`support/fake_hub`) and a scripted phone (`support/scripted_phone`) that answers
//! `/SessionValidation` and `/connect/v1` (with [`World::connect_reply`]) and records every message it
//! receives together with the hub trace context it arrived with. `tests/main.rs` declares it as
//! `crate::support::engine_world`, next to the `crate::support::auth_world`, `crate::support::fake_hub` and `crate::support::scripted_phone` it
//! uses.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use phonelink_engine::codec::hub::TraceContextPacket;
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::config::{CdehIdentity, EngineConfig};
use phonelink_engine::constants::{PL_CAPABILITIES, PROFILE_PL, PROFILE_WEA};
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::{FeatureEdges, StoreLocation};
use phonelink_engine::proto::platform_sdk::v1 as psdk;
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, Environment, MessageType, TransportMessage, header};
use prost::Message;

use crate::support::auth_world::harness::{Harness, eventually};
use crate::support::fake_hub::{FakeHub, HubSession, on_connected, on_partner_connected};
use crate::support::scripted_phone::{FromPc, ScriptedPhone, app_response, platform_response};

/// One message the phone received, with the trace context of the hub invocation that completed it.
#[derive(Debug, Clone)]
pub struct Received {
    pub message: TransportMessage,
    pub trace: Option<TraceContextPacket>,
}

impl Received {
    /// A trace-state value (`ypTriggerId`, `isuserinitiated`, `isForeground`, …).
    pub fn trace_state(&self, key: &str) -> Option<String> {
        self.trace.as_ref()?.trace_state.as_ref()?.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }
}

pub struct World {
    pub h: Harness,
    pub hub: FakeHub,
    pub phone_id: String,
    /// What the phone answers to `/connect/v1`.
    pub connect_reply: Arc<Mutex<ValueSet>>,
    pub received: Arc<Mutex<Vec<Received>>>,
    scripted: Arc<Mutex<ScriptedPhone>>,
    session: Mutex<Option<Arc<HubSession>>>,
}

/// The default connect reply: success, the PC's contract version, every YPP capability.
pub fn default_connect_reply() -> ValueSet {
    ValueSet::new().with("result", 0i32).with("contractVersion", 3.99f64).with("yppCapabilities", 0x7Fi32)
}

impl World {
    /// A linked Android phone (`custom_data` replaces its `customData`; the harness default carries
    /// every YPP capability) in the account's device list, a hub, and the scripted phone.
    pub async fn new(custom_data: Option<&str>) -> Self {
        let h = Harness::new().await;
        h.seed_session();

        let mut device = h.phone("Pixel");
        if let Some(custom) = custom_data {
            device.item.custom_data = Some(custom.to_string());
        }
        let phone_id = device.id.clone();
        h.dcg.add_peer(device.item.clone());
        h.select_phone(&device.id);

        let hub = FakeHub::start().await;
        let scripted = Arc::new(Mutex::new(ScriptedPhone::new(&phone_id)));

        let world = Self {
            h,
            hub,
            phone_id,
            connect_reply: Arc::new(Mutex::new(default_connect_reply())),
            received: Arc::new(Mutex::new(Vec::new())),
            scripted,
            session: Mutex::new(None),
        };

        world.script_phone();
        world
    }

    fn script_phone(&self) {
        let scripted = self.scripted.clone();
        let received = self.received.clone();
        let connect_reply = self.connect_reply.clone();
        let pid = self.phone_id.clone();

        self.hub.on_invocation(move |session, inv| {
            if inv.target_id() != Some(pid.as_str()) {
                return;
            }

            let Some(packet) = inv.packet() else { return };
            let mut phone = scripted.lock().unwrap();
            if let FromPc::Fragment { ack, message } = phone.on_pc_packet(&packet.to_value()) {
                session.invoke("OnReceiveMessage", phone.ack_arguments(&ack));
                let Some(msg) = message else { return };

                let reply = match (msg.message_type, msg.route()) {
                    (MessageType::Platform, Some("/SessionValidation")) => {
                        let response = psdk::ProtoPlatformSessionValidationResponse {
                            platform_capabilities: vec![psdk::ProtoPlatformCapabilities::SessionValidation as i32],
                            persistent_messaging_channel_version: 0,
                            nano_transport_preference_version: 0,
                        };
                        Some(platform_response(&msg, header::CONTENT_TYPE_BINARY, response.encode_to_vec()))
                    }
                    (MessageType::App, Some("/connect/v1")) => {
                        Some(app_response(&msg, &connect_reply.lock().unwrap().clone()))
                    }
                    _ => None,
                };

                if let Some(reply) = reply {
                    let (args, _) = phone.message_arguments(&reply);
                    session.invoke("OnReceiveMessage", args);
                }

                received.lock().unwrap().push(Received { message: msg, trace: inv.trace() });
            }
        });
    }

    /// The test configuration with this world's hub. The scripted phone does not answer
    /// `/PlatformDisconnectRequest`: a stop waits 1 s for it, not 30 s.
    pub fn config(&self) -> EngineConfig {
        let mut config = self.h.config.clone();
        config.endpoints.hub_base = Some(self.hub.base_url());
        config.platform.send_disconnect_request_timeout = Duration::from_secs(1);
        config
    }

    /// [`World::config`] with `EngineConfig::production`'s identities: Phone Link's `PL` and, beside
    /// it, the Cross Device Experience Host's `WEA`.
    pub fn two_identities(&self) -> EngineConfig {
        let mut config = self.config();
        config.identity.profile = PROFILE_PL;
        config.auth.capabilities = PL_CAPABILITIES.iter().map(|s| s.to_string()).collect();
        config.identity.cdeh =
            Some(CdehIdentity { profile: PROFILE_WEA, capabilities: EngineConfig::default().auth.capabilities });
        config
    }

    /// In-memory feature stores and the features' stand-in edges.
    pub fn feature_edges() -> FeatureEdges {
        FeatureEdges::stand_ins(StoreLocation::Memory, RecordingNotifier::new())
    }

    /// An `EngineCore` over this world, built and started.
    pub async fn engine(&self) -> Arc<EngineCore> {
        let engine = EngineCore::build(
            self.config(),
            self.h.edges(),
            Self::feature_edges(),
            MemoryTap::new(),
            self.h.core_events(),
        )
        .expect("engine builds");
        engine.start(None).await.expect("start sequence");
        engine
    }

    pub fn phone(&self) -> DcgClient {
        DcgClient::new(self.phone_id.clone(), Environment::Prod)
    }

    /// Brings the phone onto the hub: `OnConnected`, the wake, `OnPartnerConnected`; returns when
    /// `connected()` holds.
    pub async fn connect_phone(&self, connected: impl FnMut() -> bool) {
        let session = self.hub.session(0).await;
        session.send(on_connected(Some("westus"), &[]));
        assert!(eventually(Duration::from_secs(20), || self.h.dcg.count("Wake") > 0).await, "the PC woke the phone");
        session.send(on_partner_connected(&self.phone_id, rmpv::Value::from("westus")));
        assert!(eventually(Duration::from_secs(20), connected).await, "the phone connected");
        *self.session.lock().unwrap() = Some(session);
    }

    /// The phone sends `message` to the PC (after [`World::connect_phone`]).
    pub fn send(&self, message: &TransportMessage) {
        let session = self.session.lock().unwrap().clone().expect("the phone is connected");
        let (args, _) = self.scripted.lock().unwrap().message_arguments(message);
        session.invoke("OnReceiveMessage", args);
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    pub fn requests_on(&self, route: &str) -> Vec<Received> {
        self.received().into_iter().filter(|r| r.message.route() == Some(route)).collect()
    }

    /// The PC's answer to the phone's request `request_id`.
    pub fn response_to(&self, request_id: &str) -> Option<TransportMessage> {
        self.received().into_iter().map(|r| r.message).find(|m| {
            m.route() == Some(header::RESPONSE_ROUTE) && m.headers.get(header::ORIGINAL_REQUEST_ID) == Some(request_id)
        })
    }

    /// The `/DeviceResourceManager` requests the phone received for `path` (e.g. `/clipboard`).
    pub fn drm_requests_for(&self, path: &str) -> Vec<Received> {
        self.requests_on("/DeviceResourceManager")
            .into_iter()
            .filter(|r| {
                psdk::DeviceResourceManagerMessage::decode(r.message.payload.as_slice())
                    .is_ok_and(|m| m.resource_path.split('?').next() == Some(path))
            })
            .collect()
    }

    /// The `ValueSet` of an App message.
    pub fn values(message: &TransportMessage) -> ValueSet {
        valueset::decode(&message.payload).expect("an App ValueSet")
    }
}
