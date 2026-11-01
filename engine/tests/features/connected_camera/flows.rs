//! The connected camera: the sync and the context handle from tag 2, default enable and publishing the
//! camera, opening a stream (permissions, the auto-prompt, the handle, the channel), the phone's
//! control messages, frames and the state, the user's controls, closing, the channel closing under the
//! stream, unpublishing, the screen lock, tag 3, and the network quality.

use crate::support::camera_world as world;
use crate::support::common::eventually;
use phonelink_engine::config::features::ConnectedCameraConfig;

use std::time::Duration;

use serde_json::json;

use phonelink_engine::features::connected_camera::edge::testing::CameraCall;
use phonelink_engine::features::connected_camera::{
    CameraError, CameraFace, CameraPlaceholder, CameraState, ConnectedCameraEvent, Effect, StreamOpenError,
};
use phonelink_engine::features::shared::nano_channels::ChannelOpenError;
use phonelink_engine::nano::device::{AcceptanceReason, PlatformChannelError};
use phonelink_engine::proto::platform_sdk::v1 as pb;

use crate::support::platform_world::{phone_id, settle};
use world::{CameraWorld, phone_item};

const DEVICE: fn() -> String = || phone_id().id.to_ascii_lowercase();

fn config(face: i32, default_extensions: i32) -> serde_json::Value {
    json!({"cameraFace":face,"oemExtensionEnabled":false,"supportedDefaultExtensions":default_extensions,
        "selectedDefaultExtension":0,"selectedOemExtension":null,"supportedOemExtensions":null})
}

/// A world whose phone published tag 2 (capable, both permissions): the camera is published.
async fn registered() -> (CameraWorld, std::sync::Arc<phonelink_engine::features::connected_camera::ConnectedCamera>) {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install();
    w.publish_camera(true, true, true).await;
    assert!(eventually(|| feature.status(&phone_id()).unwrap().registered).await, "the camera is published");
    (w, feature)
}

/// A registered camera whose client started: the stream channel is open.
async fn streaming()
-> (CameraWorld, std::sync::Arc<phonelink_engine::features::connected_camera::ConnectedCamera>, world::Stream) {
    let (w, feature) = registered().await;
    feature.client_changed(&phone_id(), true);
    let stream = w.next_stream().await;
    (w, feature, stream)
}

#[tokio::test(start_paused = true)]
async fn listening_sends_the_sync_and_tag_2_publishes_the_camera() {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install();
    assert!(eventually(|| w.syncs() == 1).await, "the sync on listening");

    let (sync, trigger) = w.requests().into_iter().next().unwrap();
    assert_eq!(sync.resource_type, pb::ProtoDeviceResourceType::CameraMetadata as i32);
    assert!(sync.payload.is_empty());
    assert_eq!(sync.resource_path, "/camerametadata");
    assert_eq!(trigger, "CDEH.CameraStream.SendDeviceRequest");

    let status = feature.status(&phone_id()).unwrap();
    assert!(status.capable && status.supported && !status.camera_enabled && !status.registered);

    w.publish_camera(true, true, true).await;
    assert!(eventually(|| feature.status(&phone_id()).unwrap().registered).await);
    let status = feature.status(&phone_id()).unwrap();
    assert!(status.camera_enabled, "enabled by default");
    assert!(!status.permission_required);
    assert_eq!(status.state, CameraState::Registered);
    assert_eq!(
        w.camera.calls(),
        vec![
            CameraCall::Register(DEVICE(), "Pixel".into()),
            CameraCall::Placeholder(DEVICE(), CameraPlaceholder::Black)
        ]
    );

    // The switch is turned on by default once only.
    feature.set_camera_enabled(&phone_id(), false).unwrap();
    assert!(eventually(|| !feature.status(&phone_id()).unwrap().registered).await);
    w.publish_camera(true, true, true).await;
    settle().await;
    assert!(!feature.status(&phone_id()).unwrap().camera_enabled);
}

#[tokio::test(start_paused = true)]
async fn an_unparsable_publication_is_dropped() {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install();
    w.publish(2, vec![0xff, 0xff, 0xff], Vec::new()).await;
    settle().await;

    let status = feature.status(&phone_id()).unwrap();
    assert!(!status.camera_enabled && !status.registered, "dropped: no default enable");

    w.publish_camera(true, true, true).await;
    assert!(eventually(|| feature.status(&phone_id()).unwrap().registered).await, "the next one is read");
}

#[tokio::test(start_paused = true)]
async fn a_stream_opens_runs_and_closes() {
    let (w, feature, stream) = streaming().await;
    let phone = phone_id();
    let mut events = feature.subscribe();

    assert_eq!(feature.status(&phone).unwrap().state, CameraState::Connecting);
    assert!(feature.status(&phone).unwrap().control_window);
    assert!(w.camera.calls().contains(&CameraCall::Placeholder(DEVICE(), CameraPlaceholder::Connecting)));
    assert_eq!(w.channels.handles.lock().unwrap().len(), 1, "created against the tag-2 handle");

    stream.action(7, None, Some(config(0, 0b10010)));
    stream.action(1, None, Some(config(0, 0b10010)));
    assert!(eventually(|| stream.fake.video_started.load(std::sync::atomic::Ordering::SeqCst) == 1).await);

    let status = feature.status(&phone).unwrap();
    assert_eq!(status.face, Some(CameraFace::Front));
    assert_eq!(status.effects, vec![Effect::None, Effect::BuiltIn(2), Effect::BuiltIn(16)]);
    assert_eq!(status.selected_effect, Some(0));

    stream.frame(10, b"sps");
    stream.frame(11, b"");
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Streaming).await);
    assert_eq!(*stream.fake.acks.lock().unwrap(), vec![10], "only decoded frames are acknowledged");

    feature.pause(&phone).unwrap();
    assert_eq!(feature.status(&phone).unwrap().state, CameraState::Paused);
    stream.frame(12, b"p");
    assert!(
        eventually(|| w.camera.calls().contains(&CameraCall::Frame(b"p".to_vec(), false))).await,
        "decoded, not shown"
    );

    feature.resume(&phone).unwrap();
    feature.switch_camera(&phone, CameraFace::Back).unwrap();
    feature.select_effect(&phone, 2).unwrap();

    let actions = stream.fake.actions();
    assert_eq!(actions.iter().map(|(a, _)| *a).collect::<Vec<_>>(), vec![2, 3, 5, 5]);
    let sent = stream.fake.sent();
    assert_eq!(sent[2].1["cameraConfigPayload"]["cameraFace"], 1);
    assert_eq!(sent[3].1["cameraConfigPayload"]["selectedDefaultExtension"], 16);
    assert_eq!(feature.status(&phone).unwrap().selected_effect, Some(2));

    stream.message("/streaming/indicators/battery", json!({"isCharging":true,"batteryLevel":77}));
    assert!(eventually(|| feature.status(&phone).unwrap().battery_level == 77).await);

    feature.client_changed(&phone, false);
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Registered).await);
    assert!(stream.fake.closed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(stream.fake.actions().len(), 4, "closed without a message");
    assert!(CameraWorld::events(&mut events).contains(&ConnectedCameraEvent::ControlWindow {
        phone: phonelink_engine::types::DcgClient::new(DEVICE(), phone.env),
        shown: false
    }));
}

#[tokio::test(start_paused = true)]
async fn phone_messages_set_the_state_and_bad_ones_are_dropped() {
    let (_w, feature, stream) = streaming().await;
    let phone = phone_id();

    stream.raw("/streaming/action", b"");
    stream.raw("/streaming/action", b"{not json");
    stream.raw("/streaming/unknown", b"{}");
    stream.action(1, None, None);
    stream.message("/streaming/status", json!({"currentStatus":4}));
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::StartStreamNotification).await);

    stream.action(6, Some(8), None);
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::TerminalError).await);
    assert_eq!(feature.status(&phone).unwrap().error, CameraError::ScreenLocked);

    stream.action(6, Some(1), None);
    assert!(eventually(|| feature.status(&phone).unwrap().error == CameraError::CameraDeviceFatal).await);
    feature.close_fatal_error(&phone).unwrap();
    assert_eq!(stream.fake.actions().last().unwrap(), &(6, json!(1)));
    assert_eq!(feature.status(&phone).unwrap().state, CameraState::Error);

    stream.action(6, None, None);
    assert!(eventually(|| feature.status(&phone).unwrap().error == CameraError::Unknown).await);

    stream.action(2, Some(12), None);
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Paused).await);
    assert_eq!(feature.status(&phone).unwrap().error, CameraError::CallInProgress);

    stream.action(4, None, None);
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Registered).await);
    assert!(!feature.status(&phone).unwrap().control_window);
}

#[tokio::test(start_paused = true)]
async fn the_channel_closing_invalidates_the_handle() {
    let (w, feature, stream) = streaming().await;
    let phone = phone_id();
    let syncs = w.syncs();

    stream.close();
    assert!(eventually(|| feature.status(&phone).unwrap().error == CameraError::ChannelDisconnected).await);
    assert!(w.camera.calls().contains(&CameraCall::Reset(DEVICE())));

    feature.refresh(&phone).unwrap();
    assert!(eventually(|| w.syncs() == syncs + 1).await, "a new sync for the next stream");

    w.publish_camera(true, true, true).await;
    let again = w.next_stream().await;
    assert_eq!(feature.status(&phone).unwrap().state, CameraState::Connecting);
    drop(again);
}

#[tokio::test(start_paused = true)]
async fn unpublishing_an_active_camera_sends_stop() {
    let (w, feature, stream) = streaming().await;
    let phone = phone_id();

    feature.set_camera_enabled(&phone, false).unwrap();
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Unregistered).await);
    assert_eq!(stream.fake.actions(), vec![(4, serde_json::Value::Null)]);
    assert!(stream.fake.closed.load(std::sync::atomic::Ordering::SeqCst));
    assert!(w.camera.calls().contains(&CameraCall::Unregister(DEVICE())));
}

#[tokio::test(start_paused = true)]
async fn the_screen_lock_removes_and_restores_the_camera() {
    let (w, feature) = registered().await;
    let phone = phone_id();

    feature.screen_lock_changed(true);
    assert!(!feature.status(&phone).unwrap().registered);
    feature.screen_lock_changed(false);
    assert!(feature.status(&phone).unwrap().registered);

    let registers = w.camera.calls().iter().filter(|c| matches!(c, CameraCall::Register(..))).count();
    assert_eq!(registers, 2);
}

#[tokio::test(start_paused = true)]
async fn a_missing_permission_auto_prompts() {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install();
    let phone = phone_id();

    w.publish_camera(true, false, true).await;
    assert!(eventually(|| feature.status(&phone).unwrap().registered).await);
    assert!(feature.status(&phone).unwrap().permission_required);

    feature.client_changed(&phone, true);
    assert!(eventually(|| feature.status(&phone).unwrap().state == CameraState::Error).await);
    assert_eq!(feature.status(&phone).unwrap().error, CameraError::CameraPermissionNeededAutoprompt);

    let (prompt, trigger) = w
        .requests()
        .into_iter()
        .find(|(m, _)| m.request_type == pb::ProtoDeviceResourceRequestType::Update as i32)
        .expect("the auto-prompt");
    assert_eq!(prompt.payload, b"CameraStreamingPermissionsRequested");
    assert_eq!(trigger, "CDEH.CameraStream.StartCameraStream");
    assert!(w.camera.calls().contains(&CameraCall::Placeholder(DEVICE(), CameraPlaceholder::StartStreamNotification)));
    assert!(w.no_stream());

    // The permanently-denied reply.
    w.answer(|m| {
        (m.request_type == pb::ProtoDeviceResourceRequestType::Update as i32)
            .then(|| phonelink_engine::features::connected_camera::wire::auto_prompt_permanently_denied(&[0x20, 1]))
            .map(|_| crate::support::platform_world::drm_answer(&[0x20, 0x01], Some(1)))
    });
    feature.refresh(&phone).unwrap();
    assert!(
        eventually(|| feature.status(&phone).unwrap().error == CameraError::CameraPermissionPermanentlyDenied).await
    );

    // The retry asks for the permission; the phone grants it and the stream opens.
    w.answer(world::phone_answers);
    feature.request_permissions(&phone).unwrap();
    assert!(eventually(|| feature.status(&phone).unwrap().camera_permission).await);
    let _stream = w.next_stream().await;
}

#[tokio::test(start_paused = true)]
async fn without_auto_prompt_a_missing_permission_is_an_error() {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install_with(ConnectedCameraConfig {
        auto_prompt: false,
        client_signal_pause: Duration::from_millis(10),
        ..ConnectedCameraConfig::default()
    });
    let phone = phone_id();

    w.publish_camera(true, true, false).await;
    assert!(eventually(|| feature.status(&phone).unwrap().registered).await);

    feature.client_changed(&phone, true);
    assert!(eventually(|| feature.status(&phone).unwrap().error == CameraError::CameraPermissionNeeded).await);
    assert!(w.camera.calls().contains(&CameraCall::Placeholder(DEVICE(), CameraPlaceholder::PhoneLocked)));
}

#[tokio::test(start_paused = true)]
async fn a_refused_channel_maps_to_errors_and_an_unopened_one_stays_connecting() {
    let (w, feature) = registered().await;
    let phone = phone_id();
    w.channels.refusals.lock().unwrap().extend([
        StreamOpenError::Creation(ChannelOpenError::Channel(PlatformChannelError::RemoteScenarioRejectedChannel(
            AcceptanceReason::MobileDataDisabled,
        ))),
        StreamOpenError::Open(ChannelOpenError::Channel(PlatformChannelError::Timeout)),
    ]);

    feature.client_changed(&phone, true);
    assert!(eventually(|| feature.status(&phone).unwrap().error == CameraError::MeteredConnection).await);

    feature.refresh(&phone).unwrap();
    assert!(eventually(|| w.channels.handles.lock().unwrap().len() == 2).await);

    settle().await;
    assert_eq!(feature.status(&phone).unwrap().state, CameraState::Connecting);
}

#[tokio::test]
async fn the_network_quality_is_shown_then_sent_on_change() {
    tokio::time::pause();
    let (_w, feature, stream) = streaming().await;
    let phone = phone_id();

    *stream.fake.metrics.lock().unwrap() =
        Some(r#"{"rttavg":"0.1","bandwidthavg":"1","packetlossrate":"0.6","packetsreceivedintimewindow":"1","metricsVersion":"1"}"#.into());
    stream.action(1, None, Some(config(1, 0)));
    assert!(eventually(|| feature.status(&phone).unwrap().low_network).await, "shown");
    assert!(stream.fake.sent().iter().all(|(s, _)| s != "/streaming/network"), "not sent");

    *stream.fake.metrics.lock().unwrap() =
        Some(r#"{"rttavg":"0.1","bandwidthavg":"1","packetlossrate":"0","packetsreceivedintimewindow":"1","metricsVersion":"1"}"#.into());
    tokio::time::advance(Duration::from_secs(11)).await;
    assert!(eventually(|| !feature.status(&phone).unwrap().low_network).await);
    let network: Vec<_> = stream.fake.sent().into_iter().filter(|(s, _)| s == "/streaming/network").collect();
    assert_eq!(network.len(), 1);
    assert_eq!(network[0].1["isLowQualityNetwork"], false);
}

#[tokio::test(start_paused = true)]
async fn a_camera_grant_on_tag_3_turns_the_camera_on() {
    let w = CameraWorld::new(phone_item()).await;
    let feature = w.install();
    let phone = phone_id();

    // Capable but turned off: the default enable already ran.
    w.publish_camera(true, false, false).await;
    feature.set_camera_enabled(&phone, false).unwrap();
    assert!(eventually(|| !feature.status(&phone).unwrap().registered).await);

    w.publish_permissions(true, true).await;
    assert!(eventually(|| feature.status(&phone).unwrap().registered).await);
    let status = feature.status(&phone).unwrap();
    assert!(status.camera_enabled);
    assert!(!status.camera_permission, "no permission request was sent since the camera was published");
}
