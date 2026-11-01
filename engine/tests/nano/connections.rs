//! The Nano path end to end against a scripted phone on 127.0.0.1: `IpSrtp` and `IceSrtp` connections
//! through SRTP, the MTU probe, keepalive and rate-control handshakes; channels created by the phone;
//! the device connection's message channel; a platform channel's creation and its messages; blob
//! streams; and the close reasons.

use std::collections::HashMap;
use std::time::Duration;

use phonelink_engine::config::NanoConfig;
use phonelink_engine::nano::channel::blob::{BlobChannel, BlobEvent, status};
use phonelink_engine::nano::channel::message::MessageChannel;
use phonelink_engine::nano::connection::{CloseReason, ConnectObject, ConnectOptions, ConnectionType, NanoConnection};
use phonelink_engine::nano::device::{
    CREATE_REQUEST_TARGET, CREATION_RESULT_TARGET, DeviceConnection, PlatformChannelType,
};
use phonelink_engine::nano::netif::HostAddress;
use phonelink_engine::proto::platform_sdk::v1::{
    ProtoCreatePlatformChannelRequest, ProtoCreatePlatformChannelStatus, ProtoPeerPlatformChannelCreationResult,
};
use phonelink_engine::types::{DcgClient, Environment};
use prost::Message as _;

use crate::support::nano_phone::{FakePhone, Mode};

const DEVICE_CONNECTION: &str = "Microsoft::Basix::Dct::Channel::Class::Messaging,1,4,ScenarioId=DeviceConnection";

async fn open_ip(phone: &FakePhone) -> NanoConnection {
    let (object, offer) = ConnectObject::initialize(ConnectOptions::new(ConnectionType::IpSrtp)).await.unwrap();
    assert!(offer.is_empty(), "the IpSrtp offer has no contract keys");
    tokio::time::timeout(Duration::from_secs(10), object.open(phone.ip_answer())).await.unwrap().unwrap()
}

fn partner() -> DcgClient {
    DcgClient::new("phone-dcg-id", Environment::Prod)
}

#[tokio::test]
async fn ip_srtp_connection_carries_a_message_channel() {
    let phone = FakePhone::start(Mode::IpSrtp).await;
    let connection = open_ip(&phone).await;
    let mut channels = connection.channels();

    let phone_end = phone.create_channel(DEVICE_CONNECTION);
    let pc_end = tokio::time::timeout(Duration::from_secs(5), channels.recv()).await.unwrap().unwrap();
    assert_eq!(pc_end.announcement.scenario_id(), Some("DeviceConnection"));

    let (pc, ph) = tokio::join!(MessageChannel::open(pc_end), MessageChannel::open(phone_end));
    let (pc, ph) = (pc.unwrap(), ph.unwrap());

    let mut at_phone = ph.register("/x/y");
    let big = vec![7u8; 5000];
    pc.send("/x/y", &big).unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), at_phone.recv()).await.unwrap().unwrap();
    assert_eq!(m.body, big, "fragmented across packets and reassembled");

    let mut at_pc = pc.register("/back");
    ph.send("/back", b"hello").unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), at_pc.recv()).await.unwrap().unwrap().body, b"hello");

    let rt: serde_json::Value = serde_json::from_str(&connection.realtime_metrics()).unwrap();
    assert_eq!(rt["metricsVersion"], "1");
    assert_eq!(rt["packetsreceivedintimewindow"], "1");
}

#[tokio::test]
async fn ice_srtp_connection_with_host_candidates() {
    let phone = FakePhone::start(Mode::IceSrtp).await;
    let options = ConnectOptions {
        host_addresses: Some(vec![HostAddress { address: "127.0.0.1:0".parse().unwrap(), interface_index: 1 }]),
        ..ConnectOptions::new(ConnectionType::IceSrtp)
    };

    let (object, offer) = ConnectObject::initialize(options).await.unwrap();
    assert_eq!(offer["security"]["key"].as_str().unwrap().len(), 32);
    assert_eq!(offer["security"]["salt"].as_str().unwrap().len(), 28);
    assert_eq!(offer["ice"]["stun"]["url"], "");

    let answer = phone.ice_answer(&offer);
    let connection = tokio::time::timeout(Duration::from_secs(10), object.open(answer)).await.unwrap().unwrap();

    let mut channels = connection.channels();
    let phone_end = phone.create_channel(DEVICE_CONNECTION);
    let pc_end = tokio::time::timeout(Duration::from_secs(5), channels.recv()).await.unwrap().unwrap();
    let (pc, ph) = tokio::join!(MessageChannel::open(pc_end), MessageChannel::open(phone_end));
    let (pc, ph) = (pc.unwrap(), ph.unwrap());

    let mut rx = ph.register("/t");
    pc.send("/t", b"over ice").unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap().body, b"over ice");
}

#[tokio::test]
async fn device_connection_creates_a_platform_channel() {
    let phone = FakePhone::start(Mode::IpSrtp).await;
    let connection = open_ip(&phone).await;
    let phone_dc = phone.create_channel(DEVICE_CONNECTION);
    let (device, ph_dc) = tokio::join!(
        DeviceConnection::complete(partner(), connection, NanoConfig::default()),
        MessageChannel::open(phone_dc),
    );
    let (device, ph_dc) = (device.unwrap(), ph_dc.unwrap());

    let mut requests = ph_dc.register(CREATE_REQUEST_TARGET);
    let scenario = "fileShare_FDF73A34-0C30-4CC0-8165-87FC30DB1E35";
    let operation = phonelink_engine::types::TraceContext::new_root(vec![("ypScenarioId".into(), "X".into())]);
    let create = device.create_platform_channel_listening(
        PlatformChannelType::NamedStreams,
        scenario,
        HashMap::new(),
        &["/fileTransfer/platform"],
        &operation,
    );
    let phone_side = async {
        let m = tokio::time::timeout(Duration::from_secs(5), requests.recv()).await.unwrap().unwrap();
        let request = ProtoCreatePlatformChannelRequest::decode(m.body.as_slice()).unwrap();
        assert_eq!(request.scenario_id, scenario);
        assert_eq!(request.r#type, 1, "NamedStreams");

        // A child of the operation's context, without its trace state.
        let trace = request.trace_context.clone().unwrap();
        assert_eq!(trace.trace_id, operation.trace_id);
        assert_eq!(trace.parent_id, operation.correlation_id);
        assert_ne!(trace.correlation_id, operation.correlation_id);
        assert!(trace.trace_state.is_empty());

        // As the phone does: the Message child, the result, then the Blob child once the Message child
        // is open.
        let consumer = &request.channel_id;
        let msg = phone.create_channel(&format!(
            "Microsoft::Basix::Dct::Channel::Class::Messaging,2,4,ScenarioId={scenario}&ConsumerId={consumer}"
        ));

        let result = ProtoPeerPlatformChannelCreationResult {
            r#type: 1,
            scenario_id: scenario.into(),
            result_code: ProtoCreatePlatformChannelStatus::Success as i32,
            trace_context: request.trace_context.clone(),
            result_reason: 1,
            channel_id: request.channel_id.clone(),
        };
        ph_dc.send(CREATION_RESULT_TARGET, &result.encode_to_vec()).unwrap();

        let m = MessageChannel::open(msg).await.unwrap();
        // Sent the moment the phone's side is open.
        m.send(&format!("/{scenario}/NamedStreams/fileTransfer/platform"), b"early").unwrap();

        let blob = phone.create_channel(&format!(
            "Microsoft::Basix::Dct::Channel::Class::Blob,3,5,ScenarioId={scenario}&ConsumerId={consumer}"
        ));
        (m, BlobChannel::open(blob).await.unwrap())
    };

    let (platform, (ph_msg, ph_blob)) = tokio::join!(create, phone_side);
    let (platform, mut listeners) = platform.unwrap();
    let mut early = listeners.remove("/fileTransfer/platform").unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), early.recv()).await.unwrap().unwrap().body,
        b"early",
        "listened to before the Message child opened"
    );
    assert!(platform.blob.is_some() && platform.video.is_none());

    // Feature messages under the NamedStreams scope.
    let mut feature = ph_msg.register(format!("/{scenario}/NamedStreams/fileTransfer/platform"));
    platform.send("/fileTransfer/platform", b"meta").unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), feature.recv()).await.unwrap().unwrap().body, b"meta");

    // A blob stream from the PC to the phone.
    let mut incoming = ph_blob.register(b"entry-1".to_vec());
    let data: Vec<u8> = (0..150_000u32).map(|i| (i % 251) as u8).collect();
    let sent = platform.blob.as_ref().unwrap().send_stream(b"entry-1", data.len() as u64, &data[..]);

    let receive = async {
        let mut got = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_secs(10), incoming.recv()).await.unwrap().unwrap() {
                BlobEvent::Start { size } => assert_eq!(size, 150_000),
                BlobEvent::Chunk(c) => got.extend(c),
                BlobEvent::Completed(s) => {
                    assert_eq!(s, status::COMPLETED);
                    return got;
                }
            }
        }
    };

    let (sent, got) = tokio::join!(sent, receive);
    sent.unwrap();
    assert_eq!(got, data);
}

#[tokio::test]
async fn refused_platform_channel_maps_the_result() {
    let phone = FakePhone::start(Mode::IpSrtp).await;
    let connection = open_ip(&phone).await;
    let phone_dc = phone.create_channel(DEVICE_CONNECTION);
    let (device, ph_dc) = tokio::join!(
        DeviceConnection::complete(partner(), connection, NanoConfig::default()),
        MessageChannel::open(phone_dc),
    );
    let (device, ph_dc) = (device.unwrap(), ph_dc.unwrap());

    let mut requests = ph_dc.register(CREATE_REQUEST_TARGET);
    let operation = phonelink_engine::types::TraceContext::new_root(Vec::new());
    let create = device.create_platform_channel(PlatformChannelType::Messaging, "s", HashMap::new(), &operation);

    let refuse = async {
        let m = requests.recv().await.unwrap();
        let request = ProtoCreatePlatformChannelRequest::decode(m.body.as_slice()).unwrap();
        let result = ProtoPeerPlatformChannelCreationResult {
            result_code: ProtoCreatePlatformChannelStatus::RemoteScenarioRejectedChannel as i32,
            result_reason: 2,
            channel_id: request.channel_id,
            ..Default::default()
        };
        ph_dc.send(CREATION_RESULT_TARGET, &result.encode_to_vec()).unwrap();
    };

    let (r, ()) = tokio::join!(create, refuse);
    assert_eq!(r.unwrap_err().to_string(), "ErrorRemoteScenarioRejectedChannel (PermissionRevoked)",);
}

#[tokio::test]
async fn close_reasons() {
    let phone = FakePhone::start(Mode::IpSrtp).await;
    let connection = open_ip(&phone).await;
    let mut closed = connection.closed();
    phone.close_connection();
    let reason =
        tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some)).await.unwrap().unwrap().clone();
    assert_eq!(reason, Some(CloseReason::PeerEnded));

    let phone = FakePhone::start(Mode::IpSrtp).await;
    let connection = open_ip(&phone).await;
    let mut closed = connection.closed();
    phone.go_silent();
    let reason =
        tokio::time::timeout(Duration::from_secs(10), closed.wait_for(Option::is_some)).await.unwrap().unwrap().clone();
    assert_eq!(reason, Some(CloseReason::KeepAliveTimeout), "the phone's 3000 ms timeout");

    // Missed packets not recovered in time close the connection as lost, before the keepalive timeout.
    let phone = FakePhone::start(Mode::IpSrtp).await;
    let (object, _) = ConnectObject::initialize(ConnectOptions::new(ConnectionType::IpSrtp)).await.unwrap();
    let connection = object.open(phone.ip_answer_with_warning(200)).await.unwrap();
    connection.close_when_unrecovered(Duration::from_millis(300));
    let mut closed = connection.closed();
    let started = std::time::Instant::now();
    phone.go_silent();

    let reason =
        tokio::time::timeout(Duration::from_secs(5), closed.wait_for(Option::is_some)).await.unwrap().unwrap().clone();
    assert_eq!(reason, Some(CloseReason::KeepAliveTimeout));
    assert!(started.elapsed() < Duration::from_millis(2500), "{:?}", started.elapsed());

    // An answer for an address nobody serves never opens.
    let phone = FakePhone::start(Mode::IpSrtp).await;
    phone.go_silent();
    let (object, _) = ConnectObject::initialize(ConnectOptions {
        open_timeout: Some(Duration::from_millis(800)),
        ..ConnectOptions::new(ConnectionType::IpSrtp)
    })
    .await
    .unwrap();
    assert_eq!(object.open(phone.ip_answer()).await.unwrap_err(), CloseReason::TimeoutExceeded);
}
