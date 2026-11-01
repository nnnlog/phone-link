//! Remote control through `EngineCore` over the fakes (`support/engine_world`): both receivers are
//! installed, a `/remotecontrol` request from a phone whose device-list entry lacks `REMOTE_CONTROL`
//! makes the engine fetch the device list from the cloud before it answers, and `remoteControl`
//! content acts. The Mac's actions only reach a recording edge.

use crate::support::auth_world;
use crate::support::engine_world as world;
use crate::support::scripted_phone;

use std::time::Duration;

use auth_world::harness::eventually;
use phonelink_engine::codec::valueset::ValueSet;
use phonelink_engine::engine::EngineCore;
use phonelink_engine::features::remote_control::PowerAction;
use phonelink_engine::features::remote_control::edge::standins::RecordingPowerControl;
use phonelink_engine::proto::platform_sdk::v1 as psdk;
use phonelink_engine::proto::remote_control::v1::{
    RemoteControlRequestMessage, RemoteControlResponseMessage, RemoteControlResult, RemoteControlType,
};
use phonelink_engine::tap::MemoryTap;
use phonelink_engine::types::{HeaderMap, MessageType, TransportMessage, header};
use prost::Message;
use world::World;

fn drm_request(request_id: &str, control_type: RemoteControlType) -> TransportMessage {
    let body = psdk::DeviceResourceManagerMessage {
        resource_type: psdk::ProtoDeviceResourceType::Unknown as i32,
        request_type: psdk::ProtoDeviceResourceRequestType::Update as i32,
        payload: RemoteControlRequestMessage { control_type: control_type as i32 }.encode_to_vec(),
        resource_path: "/remotecontrol".into(),
    }
    .encode_to_vec();

    let mut headers = HeaderMap::new();
    headers.insert(header::ROUTE, "/DeviceResourceManager");
    headers.insert(header::REQUEST_ID, request_id);
    headers.insert(header::REJECTION_VERSION, "1");
    headers.insert(header::MS_CONTENT_TYPE, header::CONTENT_TYPE_BINARY);
    TransportMessage::new(MessageType::Platform, headers, body)
}

fn drm_answer(message: &TransportMessage) -> RemoteControlResponseMessage {
    let body = psdk::DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()).unwrap();
    assert_eq!(body.response_type, Some(1));
    RemoteControlResponseMessage::decode(body.payload.as_slice()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_contracts_act_through_the_engine() {
    let w = World::new(None).await;
    let power = RecordingPowerControl::new();
    let mut edges = World::feature_edges();
    edges.pc_power = power.clone();

    let engine =
        EngineCore::build(w.config(), w.h.edges(), edges, MemoryTap::new(), w.h.core_events()).expect("engine builds");
    engine.start(None).await.expect("start sequence");
    w.connect_phone(|| engine.phone_connected()).await;

    // The harness phone advertises no REMOTE_CONTROL: the engine refreshes the device list from the
    // cloud, still finds none, and answers FAILED.
    let lists_before = w.h.dcg.count("GetDeviceInfoList");
    w.send(&drm_request("rc-1", RemoteControlType::Lock));
    assert!(eventually(Duration::from_secs(20), || w.response_to("rc-1").is_some()).await, "answered");
    assert_eq!(drm_answer(&w.response_to("rc-1").unwrap()).control_result, RemoteControlResult::Failed as i32);
    assert!(w.h.dcg.count("GetDeviceInfoList") > lists_before, "the device list was fetched again");
    assert!(power.actions().is_empty());

    // The phone now advertises both: the refresh before the next check finds them.
    w.h.dcg.peer_mut(&w.phone_id, |p| {
        p.capabilities = Some(vec!["REMOTE_CONTROL".into(), "REMOTE_CONTROL_V2".into()]);
    });
    w.send(&drm_request("rc-2", RemoteControlType::Sleep));
    assert!(eventually(Duration::from_secs(20), || w.response_to("rc-2").is_some()).await, "answered");
    assert_eq!(drm_answer(&w.response_to("rc-2").unwrap()).control_result, RemoteControlResult::Success as i32);
    assert_eq!(power.actions(), [PowerAction::Sleep]);

    // Phone Link's `remoteControl` content.
    let content = ValueSet::new()
        .with("contentType", "remoteControl")
        .with("contractVersion", 3.99f64)
        .with("syncID", "rc-content")
        .with("remoteControlType", 0i32);
    w.send(&scripted_phone::app_request("/legacy/phonecontent", "rc-3", &content));
    assert!(eventually(Duration::from_secs(20), || w.response_to("rc-3").is_some()).await, "answered");
    assert_eq!(World::values(&w.response_to("rc-3").unwrap()), ValueSet::new().with("result", 0i32));
    assert_eq!(power.actions(), [PowerAction::Sleep, PowerAction::LockScreen]);
}
