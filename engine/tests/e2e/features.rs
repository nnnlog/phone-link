//! End to end, offline, with the features installed: the real engine (every layer and every feature,
//! wired by `EngineCore`) against fake MSA and DCG REST servers, a fake relay hub and a scripted phone
//! at the `ms-dcg` level. The connect message carries each feature's provider keys and its response
//! reaches the features; a notification batch the phone pushes is answered and presented through the
//! system-notification edge; a CDEH device-status publication reaches the device-status model; and a
//! response to a posted notification is routed back to the owning feature.

use crate::support::auth_world;
use crate::support::fake_hub;
use crate::support::scripted_phone;

use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use auth_world::harness::{Harness, eventually};
use fake_hub::{FakeHub, on_connected, on_partner_connected};
use phonelink_engine::codec::valueset::{self, ValueSet};
use phonelink_engine::edge::Clock;
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::shared::notifier::testing::RecordingNotifier;
use phonelink_engine::features::{FeatureEdges, StoreLocation};
use phonelink_engine::proto::device_info::v1::{DeviceStatus, DeviceStatusResponseMessage, DeviceStatusResponseStatus};
use phonelink_engine::proto::platform_sdk::v1 as psdk;
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{DcgClient, Environment, HeaderMap, MessageType, TransportMessage, header};
use prost::Message;
use rmpv::Value;
use scripted_phone::{FromPc, ScriptedPhone, app_request, app_response, platform_response};

/// What the scripted phone received from the PC.
#[derive(Default)]
struct PhoneLog {
    messages: Vec<TransportMessage>,
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

/// The phone's answers: `/SessionValidation`, and a connect response that grants notification access
/// and reports the notification capabilities.
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
                .with("yppCapabilities", 0x7Fi32)
                .with("permissions", ValueSet::new().with("notifications", true))
                .with("capabilities", ValueSet::new().with("notifications", 0i32))
                .with("batteryPercent", 81i32);
            Some(app_response(msg, &vs))
        }
        _ => None,
    }
}

/// A `/Context/Publish` request carrying `payload` under MSAEP `tag`.
fn publication(request_id: &str, phone_id: &str, tag: i32, payload: psdk::PubSubPayload) -> TransportMessage {
    let msaep = psdk::MsaepMessage {
        message_tag: tag,
        dcg_client_id: phone_id.to_string(),
        message_id: uuid::Uuid::new_v4().hyphenated().to_string(),
        payload: payload.encode_to_vec(),
        platform_major_version: 1,
        platform_minor_version: 1,
    };

    let mut headers = HeaderMap::new();
    headers.insert(header::REQUEST_ID, request_id);
    headers.insert(header::ROUTE, "/Context/Publish");
    headers.insert(header::REJECTION_VERSION, "1");
    headers.insert(header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY);

    TransportMessage::new(MessageType::Platform, headers, msaep.encode_to_vec())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(clippy::too_many_lines, reason = "one end-to-end scenario across the features")]
async fn the_features_ride_the_phone_session_end_to_end() {
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
    let log = Arc::new(Mutex::new(PhoneLog::default()));
    {
        let scripted = scripted.clone();
        let log = log.clone();
        let pid = phone_id.clone();
        hub.on_invocation(move |session, inv| {
            if inv.target_id() != Some(pid.as_str()) {
                return;
            }

            let Some(packet) = inv.packet() else { return };
            let mut phone = scripted.lock().unwrap();
            if let FromPc::Fragment { ack, message } = phone.on_pc_packet(&packet.to_value()) {
                session.invoke("OnReceiveMessage", phone.ack_arguments(&ack));
                if let Some(msg) = message {
                    if let Some(reply) = answer(&msg) {
                        let (args, _) = phone.message_arguments(&reply);
                        session.invoke("OnReceiveMessage", args);
                    }
                    log.lock().unwrap().messages.push(msg);
                }
            }
        });
    }

    let notifier = RecordingNotifier::new();
    let features = FeatureEdges::stand_ins(StoreLocation::Memory, notifier.clone());
    let engine =
        EngineCore::build(config, h.edges(), features, MemoryTap::new(), h.core_events()).expect("engine builds");
    engine.start(None).await.expect("start sequence");
    let installed = engine.features().expect("the features are installed by the start sequence");
    let phone = DcgClient::new(phone_id.clone(), Environment::Prod);

    let session = hub.session(0).await;
    session.send(on_connected(Some("westus"), &[]));
    assert!(eventually(Duration::from_secs(20), || h.dcg.count("Wake") > 0).await, "the PC woke the phone");
    session.send(on_partner_connected(&phone_id, Value::from("westus")));
    assert!(eventually(Duration::from_secs(20), || engine.phone_connected()).await, "phone connected");

    // The connect message carries every feature provider's keys.
    assert!(
        eventually(Duration::from_secs(20), || !log.lock().unwrap().requests_on("/connect/v1").is_empty()).await,
        "the connect message reached the phone"
    );
    {
        let connect = log.lock().unwrap().requests_on("/connect/v1")[0].clone();
        let vs = valueset::decode(&connect.payload).unwrap();
        let configuration = vs.object("configuration").expect("configuration");

        for key in [
            "BatteryIndicator",     // device status
            "MediaInfoSync",        // photos
            "MessagingSync",        // messaging
            "AudioInfoSyncEnabled", // notifications
            "ContactsV2Sync",       // contacts
        ] {
            assert!(configuration.contains_key(key), "connect configuration lacks {key}");
        }

        assert_eq!(configuration.bool("Wallpaper"), Some(true), "wallpaper");
        let sequence_numbers = vs.object("sequenceNumbers").expect("sequenceNumbers");
        for key in ["media_info", "sms", "contact", "wallpaper"] {
            assert_eq!(sequence_numbers.i64(key), Some(-1), "{key}: nothing synced yet");
        }
    }

    // Its response reached the features: notification access granted, the battery recorded.
    assert!(
        eventually(Duration::from_secs(10), || engine
            .app
            .phone_record()
            .is_some_and(|r| r.permission_or(0x40, false) && r.status_keys.battery_percent == Some(81)))
        .await,
        "the connect response was applied by the notifications and device-status providers"
    );

    let send = |msg: TransportMessage| {
        let (args, _) = scripted.lock().unwrap().message_arguments(&msg);
        session.invoke("OnReceiveMessage", args);
    };

    // A notification batch: answered `result 0` and presented through the system-notification edge.
    let now_ms = h.clock.now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64;
    let key = "0|com.example.chat|42|tag|10123";
    let item = serde_json::json!({
        "key": key, "packageName": "com.example.chat", "appName": "Chat",
        "title": "Ada", "text": "Lunch?", "postTime": now_ms, "importance": 4,
        "notificationActions": [{"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0}],
    })
    .to_string();
    send(app_request(
        "/legacy/phonecontent",
        "nt-1",
        &ValueSet::new()
            .with("contentType", "notifications")
            .with("correlationVector", "0123456789abcdef")
            .with("contractVersion", 3.99f64)
            .with("notificationKeys", vec![key])
            .with("operations", vec![1i32])
            .with("notifications", vec![item.as_str()]),
    ));
    assert!(
        eventually(Duration::from_secs(20), || log.lock().unwrap().response_to("nt-1").is_some()).await,
        "the batch was answered"
    );

    let answer = log.lock().unwrap().response_to("nt-1").unwrap();
    assert_eq!(valueset::decode(&answer.payload).unwrap().i32("result"), Some(0));
    assert!(eventually(Duration::from_secs(10), || !notifier.posted().is_empty()).await, "the notification was posted");

    let posted = notifier.posted()[0].clone();
    assert!(posted.id.starts_with("notifications/"), "{}", posted.id);
    assert_eq!(
        (posted.title.as_str(), posted.subtitle.as_deref(), posted.body.as_str()),
        ("Chat", Some("Ada"), "Lunch?")
    );
    assert!(posted.alert);
    assert_eq!(installed.notifications.list().expect("the list").len(), 1);

    // A CDEH device-status publication (MSAEP tag 8) is acknowledged and reaches the CDEH model.
    let status = DeviceStatusResponseMessage {
        status: DeviceStatusResponseStatus::Ok as i32,
        info: Some(DeviceStatus { battery_percent: 57, wifi_state: true, ..Default::default() }),
    };
    send(publication(
        "pub-8",
        &phone_id,
        8,
        psdk::PubSubPayload { additional: status.encode_to_vec(), ..Default::default() },
    ));
    assert!(
        eventually(Duration::from_secs(20), || log.lock().unwrap().response_to("pub-8").is_some()).await,
        "the publication was acknowledged"
    );
    assert!(
        eventually(Duration::from_secs(10), || installed
            .device_status
            .cdeh_status(&phone)
            .is_some_and(|s| s.battery_percent == 57))
        .await,
        "the publication reached the CDEH model"
    );

    // Responses to posted notifications are routed to the owning feature: a reply becomes a queued
    // `/notifications` operation 5 with the typed text.
    let reply_action = posted.actions.iter().find(|a| a.text_input.is_some()).expect("a reply action");
    engine.responses.route(
        &posted.id,
        &posted.user_info,
        phonelink_engine::features::NotificationResponse::Reply {
            action_id: reply_action.id.clone(),
            text: "On my way".into(),
        },
    );

    // (The connect response that turned notification access on also started a full sync — trigger
    // (b), operation 3 — so the reply is found by its operation.)
    let operation = |m: &TransportMessage| valueset::decode(&m.payload).ok().and_then(|vs| vs.i32("operation"));
    let replies = || -> Vec<TransportMessage> {
        log.lock().unwrap().requests_on("/notifications").into_iter().filter(|m| operation(m) == Some(5)).collect()
    };

    assert!(eventually(Duration::from_secs(20), || !replies().is_empty()).await, "the reply reached the phone");
    assert!(
        log.lock().unwrap().requests_on("/notifications").iter().any(|m| operation(m) == Some(3)),
        "the access grant started a full sync"
    );

    let vs = valueset::decode(&replies()[0].payload).unwrap();
    assert_eq!(vs.str("key"), Some(key));
    assert_eq!(vs.i32("actionIndex"), Some(0));
    assert_eq!(vs.str("inlineReplyMessage"), Some("On my way"));

    assert!(engine.phone_connected());
    assert!(!session.is_closed());
}
