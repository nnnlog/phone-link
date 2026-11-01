//! End to end, offline: the real engine (every layer, wired by `EngineCore`) against fake MSA and
//! DCG REST servers, a fake relay hub, and a scripted phone at the `ms-dcg` level. It checks the
//! full start sequence without the network: sign-in → enrollment → trust sync → hub
//! connection → wake → partner on the hub → `/SessionValidation` → `/connect/v1`, then the phone's
//! own traffic: a `heartbeat`, an unknown platform route, an unknown `contentType`, a malformed
//! payload — each answered as the protocol requires, with the session kept up throughout.

use crate::support::auth_world;
use crate::support::fake_hub;
use crate::support::scripted_phone;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use auth_world::harness::{Harness, eventually};
use fake_hub::{FakeHub, on_connected, on_partner_connected};
use phonelink_engine::auth::FetchMode;
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::{FeatureEdges, StoreLocation};
use phonelink_engine::proto::platform_sdk::v1 as psdk;
use phonelink_engine::tap::{MemoryTap, TapLayer};
use phonelink_engine::types::{MessageType, TransportMessage, header};
use prost::Message;
use rmpv::Value;
use scripted_phone::{FromPc, ScriptedPhone, app_request, app_response, platform_request, platform_response};

/// The features' in-memory stand-ins (`FeatureEdges::stand_ins`): in-memory stores, a recording
/// notifier.
fn feature_edges() -> FeatureEdges {
    FeatureEdges::stand_ins(StoreLocation::Memory, RecordingNotifier::new())
}

/// What the scripted phone received from the PC.
#[derive(Default)]
struct PhoneLog {
    messages: Vec<TransportMessage>,
    acks: Vec<i32>,
    other: Vec<String>,
}

impl PhoneLog {
    fn requests_on(&self, route: &str) -> Vec<TransportMessage> {
        self.messages.iter().filter(|m| m.route() == Some(route)).cloned().collect()
    }
    fn response_to(&self, request_id: &str) -> Option<TransportMessage> {
        self.messages
            .iter()
            .find(|m| {
                m.route() == Some(header::RESPONSE_ROUTE)
                    && m.headers.get(header::ORIGINAL_REQUEST_ID) == Some(request_id)
            })
            .cloned()
    }
}

/// The phone's answers to the PC's requests.
fn answer(msg: &TransportMessage) -> Option<TransportMessage> {
    match (msg.message_type, msg.route()) {
        (MessageType::Platform, Some("/SessionValidation")) => {
            let response = psdk::ProtoPlatformSessionValidationResponse {
                platform_capabilities: vec![psdk::ProtoPlatformCapabilities::SessionValidation as i32],
                persistent_messaging_channel_version: 0,
                nano_transport_preference_version: 0,
            };
            Some(platform_response(msg, header::CONTENT_TYPE_BINARY, response.encode_to_vec()))
        }
        (MessageType::App, Some("/connect/v1")) => {
            let vs = ValueSet::new()
                .with("result", 0i32)
                .with("contractVersion", 3.99f64)
                .with("yppVersion", 8i32)
                .with("yppCapabilities", 0x7Fi32);
            Some(app_response(msg, &vs))
        }
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(clippy::too_many_lines, reason = "one end-to-end scenario from the start to the phone's answers")]
async fn pc_brings_up_the_phone_session_and_answers_the_phone() {
    let h = Harness::new().await;
    h.seed_session();
    let device = h.phone("Pixel");
    let phone_id = device.id.clone();
    h.dcg.add_peer(device.item.clone());
    h.select_phone(&device.id);

    let hub = FakeHub::start().await;
    let mut config = h.config.clone();
    config.endpoints.hub_base = Some(hub.base_url());

    // The scripted phone answers every packet the PC sends it.
    let scripted = Arc::new(Mutex::new(ScriptedPhone::new(&phone_id)));
    let log = Arc::new(Mutex::new(PhoneLog::default()));
    {
        let scripted = scripted.clone();
        let log = log.clone();
        let pid = phone_id.clone();
        hub.on_invocation(move |session, inv| {
            if inv.target_id() != Some(pid.as_str()) {
                return;
            }

            let Some(packet) = inv.packet() else { return }; // SendConnectedAsync carries no packet
            let mut phone = scripted.lock().unwrap();
            match phone.on_pc_packet(&packet.to_value()) {
                FromPc::Fragment { ack, message } => {
                    session.invoke("OnReceiveMessage", phone.ack_arguments(&ack));
                    if let Some(msg) = message {
                        if let Some(reply) = answer(&msg) {
                            let (args, _) = phone.message_arguments(&reply);
                            session.invoke("OnReceiveMessage", args);
                        }
                        log.lock().unwrap().messages.push(msg);
                    }
                }
                FromPc::Ack(a) => log.lock().unwrap().acks.push(a.sequence_number),
                FromPc::Other(e) => log.lock().unwrap().other.push(e),
            }
        });
    }

    let tap = MemoryTap::new();
    let engine =
        EngineCore::build(config, h.edges(), feature_edges(), tap.clone(), h.core_events()).expect("engine builds");
    let started = engine.start(None).await.expect("start sequence");
    assert_eq!(started.phone.expect("the selected phone").client.id, phone_id, "the selected LTW device is the phone");

    // Hub connection up → OnConnected; the wake waits for it, then posts /Dispatcher/Wake.
    let session = hub.session(0).await;
    session.send(on_connected(Some("westus"), &[]));
    assert!(eventually(Duration::from_secs(20), || h.dcg.count("Wake") > 0).await, "the PC woke the phone");

    let wake = &h.dcg.of("Wake")[0];
    assert_eq!(wake.json()["deviceId"], phone_id.as_str());
    assert_eq!(wake.json()["data"]["DCG-HubRegion"], "westus");

    // The phone arrives on the hub.
    session.send(on_partner_connected(&phone_id, Value::from("westus")));
    assert!(
        eventually(Duration::from_secs(20), || engine.phone_connected()).await,
        "the per-partner connection reports the phone connected"
    );

    // The PC validates the session and sends the connect message; the phone answers both.
    assert!(
        eventually(Duration::from_secs(20), || {
            let l = log.lock().unwrap();
            !l.requests_on("/SessionValidation").is_empty() && !l.requests_on("/connect/v1").is_empty()
        })
        .await,
        "/SessionValidation and /connect/v1 reached the phone"
    );
    {
        let l = log.lock().unwrap();
        let sv = &l.requests_on("/SessionValidation")[0];
        assert_eq!(sv.message_type, MessageType::Platform);
        assert_eq!(sv.headers.get(header::MS_CONTENT_TYPE), Some(header::CONTENT_TYPE_BINARY));
        assert_eq!(sv.headers.get(header::REJECTION_VERSION), Some("1"));
        assert_eq!(sv.payload, vec![0x0A, 0x01, 0x01], "[SESSION_VALIDATION]");

        let connect = &l.requests_on("/connect/v1")[0];
        assert_eq!(connect.message_type, MessageType::App);
        let keys: Vec<_> = connect.headers.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["_requestId", "_route", "_rejectionVersion"]);

        let vs = valueset::decode(&connect.payload).unwrap();
        assert_eq!(vs.str("contentType"), Some("connect"));
        assert_eq!(vs.f64("contractVersion"), Some(3.99));
    }
    assert!(
        eventually(Duration::from_secs(10), || engine
            .app
            .phone_record()
            .is_some_and(|r| r.reported_contract_version == Some(3.99)))
        .await,
        "the connect response was applied to the phone record"
    );

    // The phone's own traffic.
    let send = |msg: TransportMessage| {
        let (args, _) = scripted.lock().unwrap().message_arguments(&msg);
        session.invoke("OnReceiveMessage", args);
    };

    let cv = "0123456789abcdef";
    send(app_request(
        "/legacy/phonecontent",
        "hb-1",
        &ValueSet::new()
            .with("contentType", "heartbeat")
            .with("correlationVector", cv)
            .with("contractVersion", 3.99f64),
    ));

    send(platform_request("/Totally/Unknown", "unk-1", "{}"));

    send(app_request(
        "/legacy/phonecontent",
        "type-1",
        &ValueSet::new()
            .with("contentType", "noSuchType")
            .with("metadata", true)
            .with("correlationVector", cv)
            .with("contractVersion", 3.99f64),
    ));

    let mut garbage = app_request("/legacy/phonecontent", "bad-1", &ValueSet::new());
    garbage.payload = vec![0x0A, 0x05, 0x0A]; // truncated protobuf
    send(garbage);

    assert!(
        eventually(Duration::from_secs(20), || {
            let l = log.lock().unwrap();
            ["hb-1", "unk-1", "type-1", "bad-1"].iter().all(|id| l.response_to(id).is_some())
        })
        .await,
        "every phone request was answered"
    );
    {
        let l = log.lock().unwrap();
        let hb = l.response_to("hb-1").unwrap();
        assert_eq!(hb.message_type, MessageType::App);
        assert_eq!(hb.headers.get(header::RESULT), Some("0"));
        assert_eq!(valueset::decode(&hb.payload).unwrap().i32("result"), Some(0));

        let unk = l.response_to("unk-1").unwrap();
        assert_eq!(unk.message_type, MessageType::Platform);
        assert_eq!(unk.headers.get(header::REJECTED_REASON), Some("NotSubscribedToRoute"));
        assert_eq!(unk.headers.get(header::MS_CONTENT_TYPE), Some(header::CONTENT_TYPE_JSON));
        assert_eq!(unk.payload, b"{}");

        let ty = l.response_to("type-1").unwrap();
        assert_eq!(valueset::decode(&ty.payload).unwrap().i32("result"), Some(2), "unknown type, metadata phase");

        let bad = l.response_to("bad-1").unwrap();
        assert_eq!(valueset::decode(&bad.payload).unwrap().i32("result"), Some(1), "unparseable payload");

        // Every phone fragment was acked by the PC (2 answers + 4 requests), nothing was undecodable,
        // and the session is still up.
        assert!(l.acks.len() >= 6, "PC acked the phone's fragments: {:?}", l.acks);
        assert!(l.other.is_empty(), "unexpected packets: {:?}", l.other);
    }
    assert!(engine.phone_connected());
    assert!(!session.is_closed());

    // A later device-list refresh that changes the phone's entry rewrites the phone record.
    h.dcg.peer_mut(&phone_id, |item| item.client_version = Some("1.25999.1.0".into()));
    engine.auth.sync_trust(FetchMode::ForceCloud).await.expect("cloud refresh");
    assert!(
        eventually(Duration::from_secs(10), || engine
            .app
            .phone_record()
            .is_some_and(|r| r.client_version.as_deref() == Some("1.25999.1.0")))
        .await,
        "the refreshed device-list entry reached the phone record"
    );

    // The decode seam saw every layer.
    let layers: std::collections::HashSet<_> = tap.snapshot().into_iter().map(|e| e.layer).collect();
    for layer in [
        TapLayer::Rest,
        TapLayer::HubMessage,
        TapLayer::DcgPacket,
        TapLayer::Envelope,
        TapLayer::Platform,
        TapLayer::App,
    ] {
        assert!(layers.contains(&layer), "tap missing {layer:?}");
    }
}

/// Prints the tap of one run (diagnostics only; `cargo test --test engine e2e::dump_tap_of_one_run -- --ignored
/// --nocapture`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn dump_tap_of_one_run() {
    let h = Harness::new().await;
    h.seed_session();
    let device = h.phone("Pixel");
    let phone_id = device.id.clone();
    h.dcg.add_peer(device.item.clone());
    h.select_phone(&device.id);

    let hub = FakeHub::start().await;
    let mut config = h.config.clone();
    config.endpoints.hub_base = Some(hub.base_url());

    let scripted = Arc::new(Mutex::new(ScriptedPhone::new(&phone_id)));
    {
        let scripted = scripted.clone();
        let pid = phone_id.clone();
        hub.on_invocation(move |session, inv| {
            if inv.target_id() != Some(pid.as_str()) {
                return;
            }
            let Some(packet) = inv.packet() else { return };
            let mut phone = scripted.lock().unwrap();
            if let FromPc::Fragment { ack, message } = phone.on_pc_packet(&packet.to_value()) {
                session.invoke("OnReceiveMessage", phone.ack_arguments(&ack));
                if let Some(reply) = message.as_ref().and_then(answer) {
                    let (args, _) = phone.message_arguments(&reply);
                    session.invoke("OnReceiveMessage", args);
                }
            }
        });
    }

    let tap = MemoryTap::new();
    let engine = EngineCore::build(config, h.edges(), feature_edges(), tap.clone(), h.core_events()).unwrap();
    engine.start(None).await.unwrap();

    let session = hub.session(0).await;
    session.send(on_connected(Some("westus"), &[]));
    assert!(eventually(Duration::from_secs(20), || h.dcg.count("Wake") > 0).await);
    session.send(on_partner_connected(&phone_id, Value::from("westus")));
    assert!(
        eventually(Duration::from_secs(20), || engine
            .app
            .phone_record()
            .is_some_and(|r| r.reported_contract_version.is_some()))
        .await
    );

    for e in tap.snapshot() {
        let s = e.summary.to_string();
        println!("{:?} {:?} {} {}", e.layer, e.direction, e.partner.unwrap_or_default(), &s[..s.len().min(160)]);
    }
}
