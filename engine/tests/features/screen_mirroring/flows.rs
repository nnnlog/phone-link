//! Screen mirroring end to end against an in-memory phone: the test plays the phone on every channel
//! of an in-memory connection.

use phonelink_engine::config::features::LinkTimers;
use phonelink_engine::config::features::ScreenMirroringConfig;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use phonelink_engine::features::screen_mirroring::edge::testing::{PresenterCall, RecordingPresenter};
use phonelink_engine::features::screen_mirroring::link::LinkEvent;
use phonelink_engine::features::screen_mirroring::link::testing::{PhoneSide, TestConnector};
use phonelink_engine::features::screen_mirroring::wire::{self, scope};
use phonelink_engine::features::screen_mirroring::{
    AudioButton, Details, FocusableState, PHONE_SCREEN_PACKAGE, PhoneState, ScreenMirroring, ScreenMirroringEvent,
    WindowState,
};
use phonelink_engine::nano::channel::blob::BlobChannel;
use phonelink_engine::nano::channel::message::{InboundMessage, MessageChannel};
use phonelink_engine::nano::channel::video::VideoFormat;
use phonelink_engine::nano::channel::{PT_CHANNEL, RawChannel, RawInbound, frame};
use phonelink_engine::nano::transport::SendMode;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

use crate::support::app_world::{self, Harness};
use crate::support::feature_world::{FeatureWorld, feature_world};
use phonelink_engine::codec::valueset::ValueSet;

const SAMSUNG_NO_NEARBY: i32 = 0x1 | 0x2 | 0x4 | 0x8 | 0x20 | 0x100 | 0x2000 | 0x4000;
const T: Duration = Duration::from_secs(5);

struct World {
    h: Harness,
    _fw: FeatureWorld,
    connector: Arc<TestConnector>,
    presenter: Arc<RecordingPresenter>,
    feature: Arc<ScreenMirroring>,
    events: broadcast::Receiver<ScreenMirroringEvent>,
}

fn world_with(remoting: i32, os: &str, config: ScreenMirroringConfig) -> World {
    let h = Harness::new();
    h.app.update_phone_record(|r| {
        r.remoting_capabilities = Some(remoting);
        r.client_model_name = Some(1);
        r.client_model_revision = Some(1);
        r.input_injector_available = Some(true);
        r.reported_contract_version = Some(3.99);
        r.attributes_os_version = Some(os.to_string());
        r.capability_bits |= 0x4;
    });

    let fw = feature_world(&h);
    let connector = Arc::new(TestConnector::default());
    let presenter = RecordingPresenter::new();
    let feature = ScreenMirroring::install(&fw.env, config, connector.clone(), presenter.clone()).unwrap();
    let events = feature.subscribe();
    World { h, _fw: fw, connector, presenter, feature, events }
}

fn world(remoting: i32) -> World {
    world_with(remoting, "14", ScreenMirroringConfig::default())
}

async fn recv(rx: &mut mpsc::UnboundedReceiver<InboundMessage>) -> InboundMessage {
    tokio::time::timeout(T, rx.recv()).await.expect("a message in time").expect("the channel is open")
}

fn json(m: &InboundMessage) -> Value {
    serde_json::from_slice(&m.body).expect("a JSON body")
}

/// The phone's end of a Message channel it created, listening on `scopes`.
struct PhoneMessage {
    channel: MessageChannel,
    inbound: HashMap<String, mpsc::UnboundedReceiver<InboundMessage>>,
}

impl PhoneMessage {
    async fn open(side: &PhoneSide, props: &str, scopes: &[&str]) -> Self {
        let (raw, _) = side.create(&format!("Microsoft::Basix::Dct::Channel::Class::Messaging,3,4,{props}"));
        let scopes: Vec<String> = scopes.iter().map(|s| s.to_string()).collect();
        let (channel, inbound) = MessageChannel::open_listening(raw, &scopes).await.unwrap();
        Self { channel, inbound }
    }

    async fn next(&mut self, target: &str) -> InboundMessage {
        recv(self.inbound.get_mut(target).expect("listening")).await
    }

    fn try_next(&mut self, target: &str) -> Option<InboundMessage> {
        self.inbound.get_mut(target).expect("listening").try_recv().ok()
    }

    fn send(&self, target: &str, body: &str) {
        self.channel.send(target, body.as_bytes()).unwrap();
    }
}

async fn next_workflow(side: &mut PhoneSide) -> (String, String, Vec<u8>) {
    let m = recv(&mut side.workflows).await;
    let v = json(&m);
    let (name, id) = (v["workflow"].as_str().unwrap().to_string(), v["workflowId"].as_str().unwrap().to_string());
    assert_eq!(m.body, wire::workflow_body(&name, &id), "the /workflow bytes");
    (name, id, m.body)
}

async fn wait_until(feature: &ScreenMirroring, what: &str, f: impl Fn(&ScreenMirroring) -> bool) {
    tokio::time::timeout(T, async {
        while !f(feature) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} in time"));
}

fn state(feature: &ScreenMirroring, id: &str) -> Option<WindowState> {
    feature.session(id).map(|s| s.state)
}

/// The phone's side once the set-up ran and the window's channels opened.
struct Up {
    side: PhoneSide,
    permission: PhoneMessage,
    orchestrator: PhoneMessage,
    audio: Option<PhoneMessage>,
    window: PhoneMessage,
    video: RawChannel,
    input: RawChannel,
    _blob: BlobChannel,
}

const ORCHESTRATOR_OUT: &[&str] = &[
    scope::CONFIG,
    scope::ALWAYS_EXPANDED,
    scope::WAKE_SCREEN,
    scope::LAUNCH_APP,
    scope::LAUNCH_PHONE_SCREEN_APP,
    scope::REQUEST_SESSION_RESULT,
    scope::BLACK_SCREEN,
    scope::APP_RESUMABLE,
    scope::LAUNCH_NOTIFICATION,
    scope::LAUNCH_CUSTOM_INTENT,
];

const WINDOW_OUT: &[&str] = &[
    scope::APP_FOCUSABLE_STATE,
    scope::DISCONNECTING_RESULT,
    scope::REQUEST_MOVE_TO_VD_RESULT,
    scope::REQUEST_SCREEN_LAYOUT,
    scope::DRAG_START_PC,
    scope::DRAG_DROP_PC,
    scope::DRAG_REQUEST_ITEM,
    scope::DRAG_REQUEST_OWNERSHIP,
    scope::DRAG_CANCEL,
];

/// Runs the set-up as the phone and opens session `session`'s channels.
async fn bring_up(w: &mut World, session: &str, package: &str, audio: bool) -> Up {
    bring_up_answering(w, session, package, audio.then_some(["accept", "accept"])).await
}

/// [`bring_up`], answering the audio checks (`oemAudio`, `audio`) with `audio` (no `deny`).
async fn bring_up_answering(w: &mut World, session: &str, package: &str, audio_checks: Option<[&str; 2]>) -> Up {
    let audio = audio_checks.is_some();
    let mut side = tokio::time::timeout(T, w.connector.next()).await.expect("a connection");
    assert_eq!(side.workflow, "permission", "the first workflow opens the connection");
    assert_eq!(side.audio, audio);

    let mut permission = grant_permission(w, &mut side, session, audio_checks).await;
    let mut orchestrator = open_orchestrator(&mut side).await;
    let audio_channel = if audio { Some(open_audio(&mut side).await) } else { None };

    let launch = orchestrator.next(scope::LAUNCH_APP).await;
    // The wake before a launch went out when nothing was open yet: no wake and no second consent
    // request after the set-up.
    assert!(orchestrator.try_next(scope::WAKE_SCREEN).is_none(), "no wakeScreen after the set-up");
    assert!(permission.try_next(scope::REQUEST_PERMISSION).is_none(), "one consent request");

    let flags = if package == PHONE_SCREEN_PACKAGE { 3 } else { 0 };
    assert_eq!(
        String::from_utf8(launch.body).unwrap(),
        format!(r#"{{"packageName":"{package}","sessionId":"{session}","targetScreenLayout":0,"flags":{flags}}}"#)
    );

    let (window, video, input, blob) = open_set(&side, session, package).await;
    Up { side, permission, orchestrator, audio: audio_channel, window, video, input, _blob: blob }
}

/// The permission workflow: the screen-scrape consent asked, shown as needed, then granted, and the
/// audio checks answered with `audio_checks`.
async fn grant_permission(
    w: &World,
    side: &mut PhoneSide,
    session: &str,
    audio_checks: Option<[&str; 2]>,
) -> PhoneMessage {
    let (name, _, _) = next_workflow(side).await;
    assert_eq!(name, "permission");

    let mut permission = PhoneMessage::open(
        side,
        "workflow=permission",
        &[scope::REQUEST_PERMISSION, scope::CHECK_PERMISSION, scope::SHOW_PERMISSION, scope::LAUNCH_SETTINGS],
    )
    .await;

    let m = permission.next(scope::REQUEST_PERMISSION).await;
    assert_eq!(m.body, br#"{"permission":"screenScrape"}"#);
    permission.send(scope::PERMISSION_STATUS, r#"{"status":"inProgress"}"#);
    wait_until(&w.feature, "permission needed", |f| {
        f.status().unwrap().phone_state == PhoneState::ScreenScrapePermissionNeeded
    })
    .await;
    assert_eq!(state(&w.feature, session), Some(WindowState::ScreenScrapePermissionNeeded));

    permission.send(scope::PERMISSION_STATUS, r#"{"status":"accept"}"#);
    for (p, status) in ["oemAudio", "audio"].into_iter().zip(audio_checks.into_iter().flatten()) {
        let m = permission.next(scope::CHECK_PERMISSION).await;
        let v = json(&m);
        assert_eq!(v["permission"], p);
        let tx = v["transactionId"].as_str().unwrap();
        permission.send(scope::CHECK_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"{status}"}}"#));
    }
    permission
}

/// The remote workflow's orchestrator channel, after the PC's display configuration and layout.
async fn open_orchestrator(side: &mut PhoneSide) -> PhoneMessage {
    let (name, _, _) = next_workflow(side).await;
    assert_eq!(name, "remote");
    let mut orchestrator = PhoneMessage::open(side, "workflow=remote&type=orchestrator", ORCHESTRATOR_OUT).await;

    let config = orchestrator.next(scope::CONFIG).await;
    assert_eq!(
        config.body,
        br#"{"maxMainDisplayFps":0,"appsFeatureType":2,"width":2880,"height":1700,"dpi":192,"hostDisplayWidth":2880,"hostDisplayHeight":1800}"#
    );
    assert_eq!(orchestrator.next(scope::ALWAYS_EXPANDED).await.body, br#"{"isAlwaysInExpandLayout":false}"#);
    orchestrator
}

/// The audio workflow's channel, after the PC allows streaming.
async fn open_audio(side: &mut PhoneSide) -> PhoneMessage {
    let (name, _, _) = next_workflow(side).await;
    assert_eq!(name, "audio");
    let mut a =
        PhoneMessage::open(side, "workflow=audio", &[scope::AUDIO_PC_STREAMING_ALLOWED, scope::AUDIO_SET_VOLUME]).await;
    assert_eq!(a.next(scope::AUDIO_PC_STREAMING_ALLOWED).await.body, br#"{"isAudioStreamingAllowed":true}"#);
    a
}

/// The phone presents the window's first frame and reports its launch: the window connects.
async fn connect(w: &World, up: &mut Up, id: &str) {
    w.feature.window_ready(id).unwrap();
    let first = video_handshake(&mut up.video).await;
    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"success"}}"#));
    wait_until(&w.feature, "launched", |f| f.session(id).unwrap().launched).await;
    up.video.sender().send(SendMode::Reliable, PT_CHANNEL, video_frame(first, b"\0\0\0\x01frame")).unwrap();
    wait_until(&w.feature, "connected", |f| state(f, id) == Some(WindowState::Connected)).await;
}

/// The phone creates and opens session `session`'s window channel set.
async fn open_set(
    side: &PhoneSide,
    session: &str,
    package: &str,
) -> (PhoneMessage, RawChannel, RawChannel, BlobChannel) {
    let props = format!("workflow=remote&type=container&packageName={package}&sessionId={session}&setId={session}");
    let class = "Microsoft::Basix::Dct::Channel::Class";

    let (mut video, _) = side.create(&format!("{class}::Video,10,1,{props}"));
    let (mut input, _) = side.create(&format!("{class}::Input,11,2,{props}"));
    let window = PhoneMessage::open(side, &props, WINDOW_OUT);
    let (blob, _) = side.create(&format!("{class}::Blob,12,5,{props}"));
    let (window, blob) = tokio::join!(window, BlobChannel::open(blob));

    video.open().await.unwrap();
    input.open().await.unwrap();
    (window, video, input, blob.unwrap())
}

fn video_packet(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut p = kind.to_le_bytes().to_vec();
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(&(body.len() as u32).to_le_bytes());
    p.extend_from_slice(body);
    p
}

/// The phone's video: the handshake; returns the PC's first frame id.
async fn video_handshake(video: &mut RawChannel) -> u32 {
    let mut b = Vec::new();
    for v in [6u32, 1080, 2400, 30] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&1_700_000_000_000u64.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    VideoFormat { fps: 30, width: 1080, height: 2400, codec: 0, rgb: None }.encode(&mut b);
    video.sender().send(SendMode::Reliable, PT_CHANNEL, video_packet(1, &b)).unwrap();

    let client = next_data(video).await;
    assert_eq!(&client[..4], &2u32.to_le_bytes(), "ClientHandshake");
    let initial = u32::from_le_bytes(client[12..16].try_into().unwrap());

    let start = next_data(video).await;
    assert_eq!(&start[12..16], &0x30u32.to_le_bytes(), "StartStream | RequestKeyframe");
    initial
}

fn video_frame(frame_id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&0x06u32.to_le_bytes());
    b.extend_from_slice(&frame_id.to_le_bytes());
    b.extend_from_slice(&5u64.to_le_bytes());
    for v in [0, bytes.len() as u32, 1, 0, 0, bytes.len() as u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(bytes);
    video_packet(4, &b)
}

async fn next_data(c: &mut RawChannel) -> Vec<u8> {
    loop {
        match tokio::time::timeout(T, c.recv()).await.expect("a packet in time") {
            Some(RawInbound::Data { payload, .. }) => return payload,
            Some(RawInbound::Opened) => {}
            other => panic!("channel ended: {other:?}"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn phone_screen_from_set_up_to_teardown() {
    let mut w = world(SAMSUNG_NO_NEARBY);
    // The phone registers its screen size as "<width>,<height>": the window starts with it.
    w.h.app.update_phone_record(|r| r.device_resolution = Some("1440,3120".to_string()));

    let id = w.feature.open_phone_screen().unwrap();
    assert!(matches!(
        w.events.recv().await.unwrap(),
        ScreenMirroringEvent::SessionOpened { session_id, package_name, phone_initiated: false }
            if session_id == id && package_name == PHONE_SCREEN_PACKAGE
    ));

    assert_eq!(w.feature.open_phone_screen().unwrap(), id, "a second open raises the window");

    let mut up = bring_up(&mut w, &id, PHONE_SCREEN_PACKAGE, true).await;
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::Connected);

    // Video starts once the window is ready; the first decoded frame and the launch result connect it.
    w.feature.window_ready(&id).unwrap();
    let first = video_handshake(&mut up.video).await;
    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"success"}}"#));
    wait_until(&w.feature, "launched", |f| f.session(&id).unwrap().launched).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::Loading), "launched but no video yet");
    up.video.sender().send(SendMode::Reliable, PT_CHANNEL, video_frame(first, b"\0\0\0\x01frame")).unwrap();
    wait_until(&w.feature, "connected", |f| state(f, &id) == Some(WindowState::Connected)).await;
    assert!(w.presenter.calls().contains(&PresenterCall::Frame(id.clone(), b"\0\0\0\x01frame".to_vec())));

    let ack = next_data(&mut up.video).await;
    assert_eq!(&ack[12..16], &0x81u32.to_le_bytes(), "frame presented");

    // Display state and input scaled to the device resolution; the start keeps the record's.
    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.initial_device_resolution, s.device_resolution), (Some((1440, 3120)), Some((1440, 3120))));

    up.window.send(scope::DEVICE_RESOLUTION_CHANGED, r#"{"width":1080,"height":2400}"#);
    up.window.send(scope::ORIENTATION_CHANGED, r#"{"orientationInfo":0}"#);
    up.window.send(scope::NAVIGATION_MODE, r#"{"NavigationMode":0,"ButtonOrder":""}"#);
    wait_until(&w.feature, "the display", |f| f.session(&id).unwrap().navigation_buttons).await;

    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.initial_device_resolution, s.device_resolution), (Some((1440, 3120)), Some((1080, 2400))));

    let mut b = Vec::new();
    for v in [4u32, 8, 1080, 2400, 10, 500] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    up.input.sender().send(SendMode::Reliable, PT_CHANNEL, frame(5, &b)).unwrap();
    let client = next_data(&mut up.input).await;
    assert_eq!(&client[..4], &6u32.to_le_bytes(), "ClientHandshakeV4");
    assert_eq!(&client[8..16], &[4, 0, 0, 0, 8, 0, 0, 0], "versions 4 to 8: version 8, as Windows");

    tokio::time::sleep(Duration::from_millis(20)).await;
    w.feature.focus_changed(&id, FocusableState::InFocus, false).unwrap();
    let focus = up.window.next(scope::APP_FOCUSABLE_STATE).await;
    let v = json(&focus);
    assert_eq!(v["focusableState"], "inFocus");
    assert!(
        String::from_utf8(focus.body)
            .unwrap()
            .starts_with(r#"{"Scope":"/message/windowFocusableState","CorrelationId":""#)
    );

    w.feature.mouse_move(&id, 0.5, 0.25).unwrap();
    let f = next_data(&mut up.input).await;
    assert_eq!(&f[..4], &7u32.to_le_bytes(), "FrameV4");
    assert_eq!(&f[8..12], &501u32.to_le_bytes(), "the frame after the phone's initial frame id");
    let changes = &f[20..];
    assert_eq!(&changes[..7], &[0, 1, 0, 0, 0, 0, 0], "the counts and indices first: one mouse");
    assert_eq!(changes.len(), 7 + 32, "a version-8 mouse record");
    assert_eq!(
        &changes[7 + 8..7 + 16],
        &[540i32.to_le_bytes(), 600i32.to_le_bytes()].concat(),
        "x and y after its buttons"
    );

    // Black screen on change; an answer to the phone's question.
    let mut s = w.feature.status().unwrap().settings;
    s.black_screen = false;
    w.feature.set_settings(s).unwrap();
    assert_eq!(up.orchestrator.next(scope::BLACK_SCREEN).await.body, br#"{"blackScreenToggleState":false}"#);

    up.orchestrator.send(scope::REQUEST_BLACK_SCREEN_STATE, "null");
    assert_eq!(up.orchestrator.next(scope::BLACK_SCREEN).await.body, br#"{"blackScreenToggleState":false}"#);

    // Phone lock states reach the window.
    up.orchestrator.send(scope::SET_LOCKED_STATE, r#"{"isLocked":true,"isFolded":false,"canDraw":true}"#);
    wait_until(&w.feature, "locked", |f| state(f, &id) == Some(WindowState::Locked)).await;
    up.orchestrator.send(scope::SET_LOCKED_STATE, r#"{"isLocked":false,"isFolded":false,"canDraw":true}"#);
    wait_until(&w.feature, "unlocked", |f| state(f, &id) == Some(WindowState::Connected)).await;

    // Disconnecting: acknowledged, then by reason.
    up.window.send(scope::DISCONNECTING, r#"{"reason":"heat","transactionId":"t-1"}"#);
    assert_eq!(up.window.next(scope::DISCONNECTING_RESULT).await.body, br#"{"transactionId":"t-1"}"#);
    wait_until(&w.feature, "overheated", |f| state(f, &id) == Some(WindowState::DeviceOverheat)).await;

    // Closing the last window stops every workflow; the connection closes when idle.
    w.feature.close_session(&id).unwrap();

    let mut stopped = Vec::new();
    for _ in 0..3 {
        stopped.push(json(&recv(&mut up.side.stops).await)["workflow"].as_str().unwrap().to_string());
    }

    stopped.sort();
    assert_eq!(stopped, ["audio", "permission", "remote"]);
    assert!(!up.side.closed_by_pc.load(Ordering::SeqCst), "kept for the idle release");
    let _ = (&up.permission, &up.audio);
}

#[tokio::test(start_paused = true)]
async fn idle_release_and_peer_stop() {
    let config = ScreenMirroringConfig {
        link: LinkTimers { idle_release: Duration::from_millis(50), ..LinkTimers::default() },
        ..ScreenMirroringConfig::default()
    };
    let mut w = world_with(SAMSUNG_NO_NEARBY & !0x2000, "14", config);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    // A stop naming another workflow id ends nothing; then the phone stops the remote workflow: the
    // window shows "peer ended".
    up.side.launcher.send(scope::STOP_WORKFLOW, br#"{"workflow":"remote","workflowId":"not-this-one"}"#).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::Connected);

    up.side.launcher.send(scope::STOP_WORKFLOW, br#"{"workflow":"remote"}"#).unwrap();
    wait_until(&w.feature, "peer ended", |f| {
        f.session(&id).is_some_and(|s| s.state == WindowState::Error && s.details == Details::Error { code: 85 })
    })
    .await;

    // The PC stopped the others; with nothing live the connection closes after the idle time.
    let stop = json(&recv(&mut up.side.stops).await);
    assert_eq!(stop["workflow"], "permission");
    wait_until(&w.feature, "idle release", |_| up.side.closed_by_pc.load(Ordering::SeqCst)).await;
    let _ = &up.window;
}

#[tokio::test(start_paused = true)]
async fn connection_loss_and_failed_set_up() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let up = bring_up(&mut w, &id, "com.example.app", false).await;

    up.side.event(LinkEvent::Unstable);
    wait_until(&w.feature, "unstable", |f| f.status().unwrap().phone_state == PhoneState::ConnectionUnstable).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::ConnectionUnstable));
    up.side.event(LinkEvent::Recovered);
    wait_until(&w.feature, "recovered", |f| f.status().unwrap().phone_state == PhoneState::Connected).await;
    up.side.event(LinkEvent::Closed(phonelink_engine::nano::connection::CloseReason::KeepAliveTimeout));
    wait_until(&w.feature, "lost", |f| f.session(&id).unwrap().details == Details::Error { code: 81 }).await;

    // A window opened after the error starts again rather than showing it.
    *w.connector.fail.lock().unwrap() = Some(808);
    let other = w.feature.launch_app("com.other", None).unwrap();
    assert_ne!(state(&w.feature, &other), Some(WindowState::Error));

    // A refused handshake: 808 (metered) with ICE support.
    *w.connector.fail.lock().unwrap() = Some(808);
    w.feature.retry().unwrap();
    wait_until(&w.feature, "metered", |f| f.session(&id).unwrap().details == Details::Error { code: 808 }).await;
    assert_eq!(w.feature.status().unwrap().phone_error, 808);

    // Without ICE support it shows as 96.
    let w2 = world(0x1 | 0x100);
    *w2.connector.fail.lock().unwrap() = Some(808);
    let id2 = w2.feature.launch_app("com.example.app", None).unwrap();
    wait_until(&w2.feature, "metered", |f| f.session(&id2).unwrap().details == Details::Error { code: 96 }).await;

    // "Allow apps" off: error 35 without connecting.
    let w3 = world(SAMSUNG_NO_NEARBY);
    let mut s = w3.feature.status().unwrap().settings;
    s.allow_apps = false;
    w3.feature.set_settings(s).unwrap();

    let id3 = w3.feature.launch_app("com.example.app", None).unwrap();
    wait_until(&w3.feature, "pc permission", |f| f.session(&id3).unwrap().details == Details::Error { code: 35 }).await;
    assert_eq!(w3.connector.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_window_that_never_loads_times_out() {
    let w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();

    // The phone never creates the permission channel.
    tokio::time::sleep(Duration::from_secs(99)).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::Loading));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.state, s.details), (WindowState::Error, Details::Error { code: 37 }));
}

#[tokio::test(start_paused = true)]
async fn window_teardown_by_reason_and_resume() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"appNotExist"}}"#));
    wait_until(&w.feature, "app error", |f| f.session(&id).unwrap().details == Details::Error { code: 34 }).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::Error), "an app window's error is not dismissible");

    // A launch that did not succeed retires the window's channel set: the PC closes it.
    wait_until(&w.feature, "the set closed", |_| up.window.channel.send(scope::APP_READY, b"{}").is_err()).await;

    // Reopen: the app launches again and the phone creates a new set.
    w.feature.reopen(&id).unwrap();
    let launch = json(&up.orchestrator.next(scope::LAUNCH_APP).await);
    assert_eq!(launch["flags"].as_i64(), Some(0));
    let (mut window, _video, _input, _blob) = open_set(&up.side, &id, "com.example.app").await;
    window.send(scope::DISCONNECTING, r#"{"reason":"MaxApps","transactionId":"t"}"#);
    wait_until(&w.feature, "suspended", |f| state(f, &id) == Some(WindowState::Suspended)).await;
    let _ = window.next(scope::DISCONNECTING_RESULT).await;

    // Resume asks the phone, then launches the app again (no bypass flag).
    let feature = w.feature.clone();
    let rid = id.clone();
    let resume = tokio::spawn(async move { feature.resume(&rid).await });
    let ask = up.orchestrator.next(scope::APP_RESUMABLE).await;
    let tx = json(&ask)["transactionId"].as_str().unwrap().to_string();
    up.orchestrator.send(scope::APP_RESUMABLE_RESULT, &format!(r#"{{"isAppResumable":true,"transactionId":"{tx}"}}"#));
    assert!(resume.await.unwrap().unwrap());

    let launch = json(&up.orchestrator.next(scope::LAUNCH_APP).await);
    assert_eq!((launch["flags"].as_i64(), launch["sessionId"].as_str()), (Some(0), Some(id.as_str())));
    assert_eq!(state(&w.feature, &id), Some(WindowState::Loading));

    // An unknown reason closes the app (Reopen); a missing transactionId is acknowledged with "".
    window.send(scope::DISCONNECTING, r#"{"reason":"SomethingNew"}"#);
    assert_eq!(window.next(scope::DISCONNECTING_RESULT).await.body, br#"{"transactionId":""}"#);
    wait_until(&w.feature, "closed", |f| state(f, &id) == Some(WindowState::Closed)).await;
    window.send(scope::DISCONNECTING, r#"{"reason":"CloseWindow","transactionId":"v"}"#);
    wait_until(&w.feature, "window closed", |f| f.session(&id).is_none()).await;

    // A message with no handler or a malformed body changes nothing and closes nothing.
    up.orchestrator.send("/mirror/phone/somethingNew", "{}");
    up.orchestrator.send(scope::SET_LOCKED_STATE, "not json");
}

#[tokio::test(start_paused = true)]
async fn phone_initiated_sessions_and_notification_launch() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    // A notification launch with app windows goes on the earlyLaunch workflow (no set-up runs).
    let feature = w.feature.clone();
    let launch = tokio::spawn(async move { feature.launch_notification("0|com.a|1|tag|10", -1).await });
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    assert_eq!(side.workflow, "earlyLaunch");
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "earlyLaunch");
    let mut early =
        PhoneMessage::open(&side, "workflow=earlyLaunch", &[scope::LAUNCH_NOTIFICATION, scope::REQUEST_SESSION_RESULT])
            .await;
    let m = json(&early.next(scope::LAUNCH_NOTIFICATION).await);
    assert_eq!((m["key"].as_str(), m["actionIndex"].as_i64()), (Some("0|com.a|1|tag|10"), Some(-1)));
    let tx = m["transactionId"].as_str().unwrap().to_string();
    early.send(scope::LAUNCH_NOTIFICATION_RESULT, &format!(r#"{{"launchResult":0,"transactionId":"{tx}"}}"#));
    launch.await.unwrap().unwrap();

    // The phone asks for a window: one session per transaction.
    early.send(scope::REQUEST_SESSION, r#"{"packageName":"com.a","transactionId":"T1"}"#);
    early.send(scope::REQUEST_SESSION, r#"{"packageName":"com.a","transactionId":"T1"}"#);
    let reply = json(&early.next(scope::REQUEST_SESSION_RESULT).await);
    let sid = reply["sessionId"].as_str().unwrap().to_string();
    assert_eq!(reply["transactionId"], "T1");
    assert_eq!(reply["sessionAlreadyExists"], false);
    assert_eq!(reply["targetScreenLayout"], 0);

    tokio::time::sleep(Duration::from_millis(50)).await;
    let sessions = w.feature.sessions();
    assert_eq!(sessions.len(), 1, "a repeated transaction is ignored");
    assert!(sessions[0].phone_initiated && sessions[0].id == sid);

    let mut opened = 0;
    while let Ok(e) = w.events.try_recv() {
        if matches!(e, ScreenMirroringEvent::SessionOpened { .. }) {
            opened += 1;
        }
    }
    assert_eq!(opened, 1);

    // The phone-initiated window runs the set-up on the same connection, then sends no launchApp.
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "permission");
}

#[tokio::test(start_paused = true)]
async fn notification_v1_and_keyboard_settings() {
    let w = world(SAMSUNG_NO_NEARBY & !0x2000 & !0x100);
    w.presenter.input_method.store(true, Ordering::SeqCst);

    let feature = w.feature.clone();
    let launch = tokio::spawn(async move { feature.launch_notification("k", 2).await });
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let _ = next_workflow(&mut side).await;
    let mut early =
        PhoneMessage::open(&side, "workflow=earlyLaunch", &[scope::LAUNCH_NOTIFICATION, scope::LAUNCH_CUSTOM_INTENT])
            .await;

    // Without the fallback result the launch is sent once and assumed done.
    launch.await.unwrap().unwrap();
    assert_eq!(early.next(scope::LAUNCH_NOTIFICATION).await.body, br#"{"key":"k","actionIndex":2}"#);

    w.feature.request_keyboard_settings().await.unwrap();
    assert_eq!(early.next(scope::LAUNCH_CUSTOM_INTENT).await.body, br#"{"intentKey":"hardware_keyboard_settings"}"#);
}

#[tokio::test(start_paused = true)]
async fn phone_screen_only_opens_apps_and_notifications_inside_it() {
    // Android 10: no app windows, the phone screen only.
    let mut w = world_with(SAMSUNG_NO_NEARBY & !0x2000, "10", ScreenMirroringConfig::default());
    assert!(
        w.feature.request_keyboard_settings().await.is_err(),
        "the phone-screen form needs the orchestrator; earlyLaunch would drop it"
    );

    let feature = w.feature.clone();
    let launch = tokio::spawn(async move { feature.launch_notification("dropped", -1).await });
    launch.await.unwrap().unwrap();
    let id = w.feature.sessions()[0].id.clone();
    assert!(w.feature.sessions()[0].phone_screen);

    let mut up = {
        let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
        let _ = next_workflow(&mut side).await;
        let mut permission = PhoneMessage::open(&side, "workflow=permission", &[scope::REQUEST_PERMISSION]).await;
        let _ = permission.next(scope::REQUEST_PERMISSION).await;
        permission.send(scope::PERMISSION_STATUS, r#"{"status":"accept"}"#);

        let _ = next_workflow(&mut side).await;
        let mut orchestrator = PhoneMessage::open(&side, "workflow=remote&type=orchestrator", ORCHESTRATOR_OUT).await;
        let config = json(&orchestrator.next(scope::CONFIG).await);
        assert_eq!((config["appsFeatureType"].as_i64(), config["maxMainDisplayFps"].as_i64()), (Some(1), Some(30)));
        let _ = orchestrator.next(scope::LAUNCH_APP).await;
        (side, permission, orchestrator)
    };

    // App launches go into the phone-screen window.
    assert_eq!(w.feature.launch_app("com.b", None).unwrap(), id);
    let m = up.2.next(scope::LAUNCH_PHONE_SCREEN_APP).await;
    assert_eq!(
        String::from_utf8(m.body).unwrap(),
        format!(r#"{{"packageName":"com.b","sessionId":"{id}","launchType":1}}"#)
    );

    // A recent task opens in a window of its own even without app windows.
    let task = w.feature.launch_app("com.task", Some(7)).unwrap();
    assert_ne!(task, id);
    let launch = json(&up.2.next(scope::LAUNCH_APP).await);
    assert_eq!((launch["sessionId"].as_str(), launch["flags"].as_i64()), (Some(task.as_str()), Some(4)));

    w.feature.request_keyboard_settings().await.unwrap();
    let k = json(&up.2.next(scope::LAUNCH_PHONE_SCREEN_APP).await);
    assert_eq!((k["packageName"].as_str(), k["launchType"].as_i64()), (Some(PHONE_SCREEN_PACKAGE), Some(2)));

    // The app launch into the phone screen replaced the first notification; a new one waits for the
    // window to become active.
    w.feature.launch_notification("nk", -1).await.unwrap();

    // Once the phone screen is connected, the notification opens with /notifications operation 5.
    let props = format!("workflow=remote&packageName={PHONE_SCREEN_PACKAGE}&sessionId={id}");
    let class = "Microsoft::Basix::Dct::Channel::Class";
    let (mut video, _) = up.0.create(&format!("{class}::Video,10,1,{props}"));
    let (_input, _) = up.0.create(&format!("{class}::Input,11,2,{props}"));
    let window = PhoneMessage::open(&up.0, &props, WINDOW_OUT);
    let (blob, _) = up.0.create(&format!("{class}::Blob,12,5,{props}"));
    let (_window, _blob) = tokio::join!(window, BlobChannel::open(blob));
    video.open().await.unwrap();
    w.feature.window_ready(&id).unwrap();
    let first = video_handshake(&mut video).await;
    up.2.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"success"}}"#));
    wait_until(&w.feature, "launched", |f| f.session(&id).unwrap().launched).await;
    video.sender().send(SendMode::Reliable, PT_CHANNEL, video_frame(first, b"x")).unwrap();
    wait_until(&w.feature, "connected", |f| state(f, &id) == Some(WindowState::Connected)).await;

    w.h.messenger.wait_requests(1).await;
    wait_until(&w.feature, "op 5", |_| !w.h.messenger.requests_on("/notifications").is_empty()).await;

    let r = &w.h.messenger.requests_on("/notifications")[0];
    let values = phonelink_engine::codec::valueset::decode(&r.message.payload).unwrap();
    assert_eq!(values.i32("operation"), Some(5));
    assert_eq!(values.str("key"), Some("nk"));
    assert!(!values.contains_key("actionIndex"), "the body: no action index");

    // An active window: a new notification waits for its next activation — the focus coming back
    // while connected.
    w.feature.focus_changed(&id, FocusableState::InFocus, false).unwrap();
    w.feature.launch_notification("nk2", 1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(w.h.messenger.requests_on("/notifications").len(), 1, "not while it stays active");

    w.feature.focus_changed(&id, FocusableState::NotInFocus, false).unwrap();
    w.feature.focus_changed(&id, FocusableState::InFocus, false).unwrap();
    wait_until(&w.feature, "op 5 again", |_| w.h.messenger.requests_on("/notifications").len() == 2).await;
    let r = &w.h.messenger.requests_on("/notifications")[1];
    let values = phonelink_engine::codec::valueset::decode(&r.message.payload).unwrap();
    assert_eq!(values.str("key"), Some("nk2"));
    let _ = (&mut w.events, &up.1);
}

#[tokio::test(start_paused = true)]
async fn nearby_check_runs_first_on_its_own_connection() {
    let mut w = world(SAMSUNG_NO_NEARBY | 0x8000);
    let id = w.feature.open_phone_screen().unwrap();

    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    assert_eq!(side.workflow, "nearby");
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "nearby");
    let mut nearby = PhoneMessage::open(
        &side,
        "workflow=nearby",
        &[scope::NEARBY_REQUEST_SETTING, scope::NEARBY_SKIP_UNLOCK, scope::NEARBY_REQUEST_PIN_CODE],
    )
    .await;

    let m = json(&nearby.next(scope::NEARBY_REQUEST_SETTING).await);
    assert_eq!(m["nearbyState"], true, "the direct IP connection: the phone is near");
    let tx = m["transactionId"].as_str().unwrap();
    nearby.send(
        scope::NEARBY_REQUEST_SETTING_RESULT,
        &format!(
            r#"{{"isLocked":false,"isSecureLocked":false,"skipUnlock":false,"skipVerification":false,"transactionId":"{tx}"}}"#
        ),
    );

    wait_until(&w.feature, "the nearby prompt", |f| f.status().unwrap().nearby.is_some_and(|n| n.skip_unlock)).await;
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::NearbyConnectionNeeded);
    assert_eq!(state(&w.feature, &id), Some(WindowState::NearbyConnectionNeeded));

    w.feature.nearby_request_pin_code().unwrap();
    assert_eq!(nearby.next(scope::NEARBY_REQUEST_PIN_CODE).await.body, b"{}");
    w.feature.nearby_skip_unlock().unwrap();
    assert_eq!(nearby.next(scope::NEARBY_SKIP_UNLOCK).await.body, b"{}");
    nearby.send(scope::NEARBY_SKIP_UNLOCK_RESULT, "{}");

    // The check is done: the permission workflow follows on the same connection.
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "permission");
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::Connecting);
    let _ = &mut w.events;
}

#[tokio::test(start_paused = true)]
async fn a_device_lock_after_a_check_done_without_waiting_changes_nothing() {
    let mut w = world(SAMSUNG_NO_NEARBY | 0x8000);
    let _id = w.feature.open_phone_screen().unwrap();

    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "nearby");
    let mut nearby = PhoneMessage::open(&side, "workflow=nearby", &[scope::NEARBY_REQUEST_SETTING]).await;
    let m = json(&nearby.next(scope::NEARBY_REQUEST_SETTING).await);
    let tx = m["transactionId"].as_str().unwrap();
    nearby.send(
        scope::NEARBY_REQUEST_SETTING_RESULT,
        &format!(
            r#"{{"isLocked":true,"isSecureLocked":true,"skipUnlock":true,"skipVerification":true,"transactionId":"{tx}"}}"#
        ),
    );

    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "permission");

    // The check waited for nothing: the phone unlocking (or locking) on its channel is not a prompt,
    // which would hold the phone state where the orchestrator's unlock cannot reach it.
    nearby.send(scope::NEARBY_DEVICE_LOCK, r#"{"isLocked":false,"isSecureLocked":false}"#);
    nearby.send(scope::NEARBY_DEVICE_LOCK, r#"{"isLocked":true,"isSecureLocked":true}"#);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let status = w.feature.status().unwrap();
    assert_eq!(status.phone_state, PhoneState::Disconnected, "the set-up has not asked for the connection yet");
    assert!(status.nearby.is_none());
    let _ = &mut w.events;
}

#[tokio::test(start_paused = true)]
async fn app_audio_volume_and_frames() {
    let mut w = world(SAMSUNG_NO_NEARBY);
    // Without an audio workflow a volume change is neither stored nor sent.
    w.feature.set_volume("com.example.app", 70, false).unwrap();

    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", true).await;
    let audio = up.audio.as_mut().unwrap();

    // A stream becomes ready: the stored volume (the default) goes to the phone.
    audio.send(scope::AUDIO_STREAM_READY, r#"{"packageName":"com.example.app"}"#);
    assert_eq!(audio.next(scope::AUDIO_SET_VOLUME).await.body, br#"{"packageName":"com.example.app","volume":50}"#);

    // Muting keeps the stored level.
    w.feature.set_volume("com.example.app", 40, true).unwrap();
    assert_eq!(audio.next(scope::AUDIO_SET_VOLUME).await.body, br#"{"packageName":"com.example.app","volume":0}"#);

    // Silence of a muted app still shows the volume button.
    audio.send(scope::AUDIO_SILENCE_STATE_CHANGED, r#"{"packageName":"com.example.app","isSilent":true}"#);
    wait_until(&w.feature, "the button", |f| f.session(&id).unwrap().audio_button == AudioButton::Volume).await;
    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.audio.volume, s.audio.muted, s.audio_button), (50, true, AudioButton::Volume));

    // The phone's volume 0 mutes and stores level 0; a later level unmutes. A target below the scope
    // is taken as the scope's.
    audio.send(scope::AUDIO_VOLUME_CHANGED, r#"{"packageName":"com.example.app","volume":30}"#);
    wait_until(&w.feature, "unmuted", |f| !f.session(&id).unwrap().audio.muted).await;
    audio.send(&format!("{}/more", scope::AUDIO_VOLUME_CHANGED), r#"{"packageName":"com.example.app","volume":0}"#);
    wait_until(&w.feature, "muted at 0", |f| f.session(&id).unwrap().audio.muted).await;
    assert_eq!(w.feature.session(&id).unwrap().audio.volume, 0);

    // Not silent and not muted: the volume button; silent and not muted: none.
    w.feature.set_volume("com.example.app", 20, false).unwrap();
    let _ = audio.next(scope::AUDIO_SET_VOLUME).await;
    audio.send(scope::AUDIO_SILENCE_STATE_CHANGED, r#"{"packageName":"com.example.app","isSilent":true}"#);
    wait_until(&w.feature, "no button", |f| f.session(&id).unwrap().audio_button == AudioButton::None).await;

    // The app refuses audio capture, then allows it: the open window follows.
    assert!(w.feature.session(&id).unwrap().audio_capture_allowed, "allowed until the phone says");
    audio.send(scope::AUDIO_CAPTURE_ALLOWED, r#"{"packageName":"com.example.app","isAudioCaptureAllowed":false}"#);
    wait_until(&w.feature, "refused", |f| !f.session(&id).unwrap().audio_capture_allowed).await;
    audio.send(scope::AUDIO_CAPTURE_ALLOWED, r#"{"packageName":"com.example.app","isAudioCaptureAllowed":true}"#);
    wait_until(&w.feature, "allowed", |f| f.session(&id).unwrap().audio_capture_allowed).await;

    // The phone is told again only when the Mac's last output goes or the first comes back.
    w.feature.audio_output_changed();
    w.presenter.audio_output.store(false, Ordering::SeqCst);
    w.feature.audio_output_changed();
    w.feature.audio_output_changed();
    assert_eq!(audio.next(scope::AUDIO_PC_STREAMING_ALLOWED).await.body, br#"{"isAudioStreamingAllowed":false}"#);
    w.presenter.audio_output.store(true, Ordering::SeqCst);
    w.feature.audio_output_changed();
    assert_eq!(audio.next(scope::AUDIO_PC_STREAMING_ALLOWED).await.body, br#"{"isAudioStreamingAllowed":true}"#);
    assert!(audio.try_next(scope::AUDIO_PC_STREAMING_ALLOWED).is_none(), "one message per change");

    // An audio channel waits for the phone's frame length, then plays.
    let class = "Microsoft::Basix::Dct::Channel::Class::Audio";
    let (mut channel, _) =
        up.side.create(&format!("{class},20,7,workflow=audio&packageName=com.example.app&syncId=com.example.app"));

    // A configuration without a frame length keeps it waiting; the channel opens with the next one's.
    audio.send(scope::AUDIO_PHONE_CONFIG, r#"{"frameLengthMs":0}"#);
    audio.send(scope::AUDIO_PHONE_CONFIG, r#"{"frameLengthMs":20}"#);
    channel.open().await.unwrap();

    let mut b = 7u32.to_le_bytes().to_vec();
    b.extend_from_slice(&0u64.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    for v in [2u32, 48_000, 0] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    channel.sender().send(SendMode::Reliable, PT_CHANNEL + 1, b).unwrap();

    let mut data = 0u32.to_le_bytes().to_vec();
    data.extend_from_slice(&9u32.to_le_bytes());
    data.extend_from_slice(&77u64.to_le_bytes());
    data.extend_from_slice(&3u32.to_le_bytes());
    data.extend_from_slice(b"ops");
    channel.sender().send(SendMode::Reliable, PT_CHANNEL + 4, data).unwrap();
    wait_until(&w.feature, "audio frames", |_| {
        w.presenter.calls().contains(&PresenterCall::AudioFrame("com.example.app".into(), b"ops".to_vec()))
    })
    .await;

    let started = w.presenter.calls().into_iter().find_map(|c| match c {
        PresenterCall::AudioStarted(p, f) => Some((p, f)),
        _ => None,
    });
    let (p, f) = started.expect("audio started");
    assert_eq!(
        (p.as_str(), f.channels, f.sample_rate, f.codec, f.frame_length_ms),
        ("com.example.app", 2, 48_000, 0, 20)
    );

    // The phone stops the audio workflow: its channel closes and its playback ends; the window and
    // the other workflows stay.
    up.side.launcher.send(scope::STOP_WORKFLOW, br#"{"workflow":"audio"}"#).unwrap();
    wait_until(&w.feature, "audio ended", |_| {
        w.presenter.calls().contains(&PresenterCall::AudioEnded("com.example.app".into()))
    })
    .await;

    let closed =
        tokio::time::timeout(T, async { while !matches!(channel.recv().await, Some(RawInbound::Closed) | None) {} })
            .await;
    assert!(closed.is_ok(), "the PC closed the audio channel");
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::Connected);
    assert!(up.side.stops.try_recv().is_err(), "the PC stops nothing");

    // Without the audio workflow a volume change is dropped.
    w.feature.set_volume("com.example.app", 90, false).unwrap();
    assert_eq!(w.feature.session(&id).unwrap().audio.volume, 20);
}

#[tokio::test(start_paused = true)]
async fn move_to_the_mac_and_always_unlocked_app_windows() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    up.window.send(scope::REQUEST_MOVE_TO_VD, r#"{"transactionId":"m-1","remoteTaskTransferSupported":true}"#);
    wait_until(&w.feature, "asked", |f| f.session(&id).unwrap().move_to_virtual_display == Some(true)).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::Restart));

    w.feature.answer_move_to_virtual_display(&id, true, true).unwrap();
    assert_eq!(
        up.window.next(scope::REQUEST_MOVE_TO_VD_RESULT).await.body,
        br#"{"result":true,"transactionId":"m-1"}"#
    );
    assert!(w.feature.status().unwrap().settings.do_not_ask_task_transfer);

    // Asked again: answered without asking.
    up.window.send(scope::REQUEST_MOVE_TO_VD, r#"{"transactionId":"m-2","remoteTaskTransferSupported":true}"#);
    assert_eq!(
        up.window.next(scope::REQUEST_MOVE_TO_VD_RESULT).await.body,
        br#"{"result":true,"transactionId":"m-2"}"#
    );

    // An app window ignores lock states while the phone keeps its display unlocked.
    let before = state(&w.feature, &id);
    up.orchestrator.send(
        scope::SET_LOCKED_STATE,
        r#"{"isLocked":true,"isFolded":false,"canDraw":true,"isAlwaysUnlockedDisplay":true}"#,
    );
    wait_until(&w.feature, "phone locked", |f| f.status().unwrap().phone_state == PhoneState::Locked).await;
    assert_eq!(state(&w.feature, &id), before);
}

#[tokio::test(start_paused = true)]
async fn a_window_opened_after_the_last_one_closed_replaces_the_idle_connection() {
    let mut w = world(SAMSUNG_NO_NEARBY | 0x8000);
    let first = w.feature.open_phone_screen().unwrap();
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "nearby");

    let mut nearby = PhoneMessage::open(&side, "workflow=nearby", &[scope::NEARBY_REQUEST_SETTING]).await;
    let m = json(&nearby.next(scope::NEARBY_REQUEST_SETTING).await);
    let tx = m["transactionId"].as_str().unwrap();
    nearby.send(
        scope::NEARBY_REQUEST_SETTING_RESULT,
        &format!(
            r#"{{"isLocked":false,"isSecureLocked":false,"skipUnlock":true,"skipVerification":true,"transactionId":"{tx}"}}"#
        ),
    );

    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "permission");

    // The user closes the only window: its workflows stop and the connection stays, idle.
    w.feature.close_session(&first).unwrap();
    wait_until(&w.feature, "no window", |f| f.sessions().is_empty()).await;

    // A new window: the nearby check closes the idle connection and opens its own.
    let second = w.feature.open_phone_screen().unwrap();
    let mut next = tokio::time::timeout(T, w.connector.next()).await.expect("a new connection");
    assert!(side.closed_by_pc.load(Ordering::SeqCst), "the idle connection was closed");
    assert_eq!(next.workflow, "nearby");
    let (name, _, _) = next_workflow(&mut next).await;
    assert_eq!(name, "nearby");
    let s = w.feature.session(&second).expect("the new window");
    assert_ne!(s.state, WindowState::Error, "replacing the idle connection is not a lost connection: {s:?}");

    // The old connection's nearby channel ends while the new check waits: that is not the new
    // check's channel, so the check goes on.
    let mut again = PhoneMessage::open(&next, "workflow=nearby", &[scope::NEARBY_REQUEST_SETTING]).await;
    let m = json(&again.next(scope::NEARBY_REQUEST_SETTING).await);
    nearby.channel.close();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let tx = m["transactionId"].as_str().unwrap();
    again.send(
        scope::NEARBY_REQUEST_SETTING_RESULT,
        &format!(
            r#"{{"isLocked":false,"isSecureLocked":false,"skipUnlock":true,"skipVerification":true,"transactionId":"{tx}"}}"#
        ),
    );

    let (name, _, _) = next_workflow(&mut next).await;
    assert_eq!(name, "permission");
    let s = w.feature.session(&second).expect("the new window");
    assert_ne!(s.state, WindowState::Error, "{s:?}");
    let _ = &mut w.events;
}

#[tokio::test(start_paused = true)]
async fn closing_the_last_window_outside_the_runtime_arms_the_idle_release() {
    let mut w = world(SAMSUNG_NO_NEARBY | 0x8000);
    let first = w.feature.open_phone_screen().unwrap();
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "nearby");

    let mut nearby = PhoneMessage::open(&side, "workflow=nearby", &[scope::NEARBY_REQUEST_SETTING]).await;
    let m = json(&nearby.next(scope::NEARBY_REQUEST_SETTING).await);
    let tx = m["transactionId"].as_str().unwrap();
    nearby.send(
        scope::NEARBY_REQUEST_SETTING_RESULT,
        &format!(
            r#"{{"isLocked":false,"isSecureLocked":false,"skipUnlock":true,"skipVerification":true,"transactionId":"{tx}"}}"#
        ),
    );

    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "permission");

    // The app closes a window from its main thread, where no runtime is current: stopping the last
    // workflow there starts the idle timer without a panic.
    std::thread::scope(|t| t.spawn(|| w.feature.close_session(&first)).join())
        .expect("closing the window outside the runtime panicked")
        .unwrap();
    wait_until(&w.feature, "no window", |f| f.sessions().is_empty()).await;

    // The feature still works: a new window opens its own nearby check.
    let _second = w.feature.open_phone_screen().unwrap();
    let mut next = tokio::time::timeout(T, w.connector.next()).await.expect("a new connection");
    let (name, _, _) = next_workflow(&mut next).await;
    assert_eq!(name, "nearby");
    let _ = &mut w.events;
}

#[tokio::test(start_paused = true)]
async fn a_launch_with_no_live_window_wakes_the_phone_first() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    // The first window is still loading: a second launch wakes the phone (the consent is given, so
    // no consent request), then the new window's set-up joins the open one and launches.
    let second = w.feature.launch_app("com.second", Some(42)).unwrap();
    assert!(up.orchestrator.next(scope::WAKE_SCREEN).await.body.is_empty());
    let launch = up.orchestrator.next(scope::LAUNCH_APP).await;
    assert_eq!(
        String::from_utf8(launch.body).unwrap(),
        format!(
            r#"{{"packageName":"com.second","sessionId":"{second}","targetScreenLayout":0,"transferTaskId":42,"flags":4}}"#
        ),
        "a task transfer carries flag 4 and its task id"
    );
    assert!(up.permission.try_next(scope::REQUEST_PERMISSION).is_none());

    // Once a window is connected, a launch does not wake the phone.
    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"SUCCESS"}}"#));
    w.feature.window_ready(&id).unwrap();
    let first = video_handshake(&mut up.video).await;
    up.video.sender().send(SendMode::Reliable, PT_CHANNEL, video_frame(first, b"f")).unwrap();
    wait_until(&w.feature, "connected (success in any case)", |f| state(f, &id) == Some(WindowState::Connected)).await;

    let third = w.feature.launch_app("com.third", None).unwrap();
    let launch = json(&up.orchestrator.next(scope::LAUNCH_APP).await);
    assert_eq!(launch["sessionId"].as_str(), Some(third.as_str()));
    assert!(up.orchestrator.try_next(scope::WAKE_SCREEN).is_none());
}

#[tokio::test(start_paused = true)]
async fn the_touch_warning_is_bypassed_once() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    assert!(w.feature.continue_true_touch(&id).is_err(), "nothing to bypass yet");
    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"trueTouchWarning"}}"#));
    wait_until(&w.feature, "warning", |f| state(f, &id) == Some(WindowState::TrueTouchWarning)).await;
    wait_until(&w.feature, "the set retired", |_| up.window.channel.send(scope::APP_READY, b"{}").is_err()).await;

    w.feature.continue_true_touch(&id).unwrap();
    let launch = json(&up.orchestrator.next(scope::LAUNCH_APP).await);
    assert_eq!(launch["flags"].as_i64(), Some(1), "past the phone's launch checks");
    assert!(w.feature.continue_true_touch(&id).is_err(), "the bypass is taken once");

    // A second warning while that bypass is pending is ignored.
    up.orchestrator.send(scope::APP_LAUNCH_RESULT, &format!(r#"{{"sessionId":"{id}","result":"trueTouchWarning"}}"#));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(w.feature.continue_true_touch(&id).is_err());
}

#[tokio::test(start_paused = true)]
async fn session_requests_on_the_orchestrator_follow_the_layout_settings() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.open_phone_screen().unwrap();
    let mut up = bring_up(&mut w, &id, PHONE_SCREEN_PACKAGE, false).await;

    let mut s = w.feature.status().unwrap().settings;
    s.always_expanded = true;
    w.feature.set_settings(s).unwrap();
    assert_eq!(up.orchestrator.next(scope::ALWAYS_EXPANDED).await.body, br#"{"isAlwaysInExpandLayout":true}"#);

    // The open phone screen: raised, the phone woken, the existing session answered.
    up.orchestrator
        .send(scope::REQUEST_PHONE_SCREEN_SESSION, r#"{"packageName":"yourphone.phonescreen","transactionId":"P"}"#);
    assert!(up.orchestrator.next(scope::WAKE_SCREEN).await.body.is_empty());
    let r = json(&up.orchestrator.next(scope::REQUEST_SESSION_RESULT).await);
    assert_eq!((r["sessionId"].as_str(), r["sessionAlreadyExists"].as_bool()), (Some(id.as_str()), Some(true)));
    assert_eq!(r["targetScreenLayout"], 2, "always expanded applies on the orchestrator");

    up.orchestrator.send(scope::REQUEST_SESSION, r#"{"packageName":"com.b","transactionId":"T"}"#);
    let r = json(&up.orchestrator.next(scope::REQUEST_SESSION_RESULT).await);
    assert_eq!((r["sessionAlreadyExists"].as_bool(), r["targetScreenLayout"].as_i64()), (Some(false), Some(2)));
}

#[tokio::test(start_paused = true)]
async fn the_permission_workflow_ending_releases_the_windows() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    up.side.launcher.send(scope::STOP_WORKFLOW, br#"{"workflow":"permission"}"#).unwrap();
    wait_until(&w.feature, "peer ended", |f| f.session(&id).unwrap().details == Details::Error { code: 85 }).await;

    let stop = json(&recv(&mut up.side.stops).await);
    assert_eq!(stop["workflow"], "remote", "the PC stops the other workflows");
}

#[tokio::test(start_paused = true)]
async fn a_refused_audio_permission_prompts_and_waits() {
    let w = world(SAMSUNG_NO_NEARBY);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let _ = next_workflow(&mut side).await;
    let mut permission = PhoneMessage::open(
        &side,
        "workflow=permission",
        &[scope::REQUEST_PERMISSION, scope::CHECK_PERMISSION, scope::SHOW_PERMISSION],
    )
    .await;

    let _ = permission.next(scope::REQUEST_PERMISSION).await;
    permission.send(scope::PERMISSION_STATUS, r#"{"status":"accept"}"#);

    for (p, status) in [("oemAudio", "disabled"), ("audio", "deny")] {
        let v = json(&permission.next(scope::CHECK_PERMISSION).await);
        assert_eq!(v["permission"], p);
        let tx = v["transactionId"].as_str().unwrap();
        permission.send(scope::CHECK_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"{status}"}}"#));
    }

    // A refusal shows the request and prompts on the phone; "disabled" at the check is no refusal.
    let v = json(&permission.next(scope::SHOW_PERMISSION).await);
    assert_eq!(v["permission"], "audio");
    wait_until(&w.feature, "audio needed", |f| state(f, &id) == Some(WindowState::AudioStreamingPermissionNeeded))
        .await;

    let tx = v["transactionId"].as_str().unwrap();
    permission.send(scope::SHOW_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"deny"}}"#));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(side.workflows.try_recv().is_err(), "the set-up waits on the refused prompt");

    // Continuing without audio lets the set-up go on to the remote workflow.
    w.feature.continue_without_audio(&id).unwrap();
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "remote");
}

#[tokio::test(start_paused = true)]
async fn an_already_opened_notification_is_no_success() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let feature = w.feature.clone();
    let launch = tokio::spawn(async move { feature.launch_notification("0|com.a|1|tag|10", -1).await });
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let _ = next_workflow(&mut side).await;
    let mut early = PhoneMessage::open(&side, "workflow=earlyLaunch", &[scope::LAUNCH_NOTIFICATION]).await;
    let tx = json(&early.next(scope::LAUNCH_NOTIFICATION).await)["transactionId"].as_str().unwrap().to_string();
    early.send(scope::LAUNCH_NOTIFICATION_RESULT, &format!(r#"{{"launchResult":4,"transactionId":"{tx}"}}"#));
    launch.await.unwrap().unwrap();

    let mut opened = None;
    while let Ok(e) = w.events.try_recv() {
        if let ScreenMirroringEvent::NotificationLaunched { opened: o, .. } = e {
            opened = Some(o);
        }
    }
    assert_eq!(opened, Some(false));
}

#[tokio::test(start_paused = true)]
async fn enable_audio_asks_again_for_the_window_permission() {
    let mut w = world(SAMSUNG_NO_NEARBY);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up_answering(&mut w, &id, "com.example.app", Some(["accept", "disabled"])).await;
    connect(&w, &mut up, &id).await;

    // The app window's own permission ended "disabled": the title bar offers to enable audio.
    assert_eq!(w.feature.session(&id).unwrap().audio_button, AudioButton::EnableAudio);

    // Still "disabled" at the fresh check: the always-denied window, without the button.
    let (feature, sid) = (w.feature.clone(), id.clone());
    let click = tokio::spawn(async move { feature.enable_audio(&sid).await });
    let v = json(&up.permission.next(scope::CHECK_PERMISSION).await);
    assert_eq!(v["permission"], "audio");
    let tx = v["transactionId"].as_str().unwrap();
    up.permission.send(scope::CHECK_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"disabled"}}"#));
    click.await.unwrap().unwrap();
    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.state, s.audio_button), (WindowState::AudioStreamingPermissionAlwaysDenied, AudioButton::None));

    // "deny" at the check prompts on the phone; a prompt ending "disabled" connects the window with
    // the button again.
    let (feature, sid) = (w.feature.clone(), id.clone());
    let click = tokio::spawn(async move { feature.enable_audio(&sid).await });
    let tx = json(&up.permission.next(scope::CHECK_PERMISSION).await)["transactionId"].as_str().unwrap().to_string();
    up.permission.send(scope::CHECK_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"deny"}}"#));
    let v = json(&up.permission.next(scope::SHOW_PERMISSION).await);
    assert_eq!(state(&w.feature, &id), Some(WindowState::AudioStreamingPermissionNeeded));
    let tx = v["transactionId"].as_str().unwrap();
    up.permission.send(scope::SHOW_PERMISSION_RESULT, &format!(r#"{{"transactionId":"{tx}","status":"disabled"}}"#));
    click.await.unwrap().unwrap();
    let s = w.feature.session(&id).unwrap();
    assert_eq!((s.state, s.audio_button), (WindowState::Connected, AudioButton::EnableAudio));

    // The app's sound replaces it with the volume button.
    up.audio
        .as_ref()
        .unwrap()
        .send(scope::AUDIO_SILENCE_STATE_CHANGED, r#"{"packageName":"com.example.app","isSilent":false}"#);
    wait_until(&w.feature, "volume", |f| f.session(&id).unwrap().audio_button == AudioButton::Volume).await;

    // A permission status below its scope is not one.
    up.permission.send(&format!("{}/x", scope::PERMISSION_STATUS), r#"{"status":"deny"}"#);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(w.feature.status().unwrap().permission_accepted);
}

#[tokio::test(start_paused = true)]
async fn a_lost_device_connection_releases_with_81() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let up = bring_up(&mut w, &id, "com.example.app", false).await;

    w.feature.device_connection_lost();
    wait_until(&w.feature, "lost", |f| f.session(&id).unwrap().details == Details::Error { code: 81 }).await;
    assert_eq!(state(&w.feature, &id), Some(WindowState::Error));
    assert!(up.side.closed_by_pc.load(Ordering::SeqCst), "the connection is closed");
}

#[tokio::test(start_paused = true)]
async fn a_failed_early_launch_cancels_the_waiting_launches() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut side = tokio::time::timeout(T, w.connector.next()).await.unwrap();
    let _ = next_workflow(&mut side).await;
    let permission = PhoneMessage::open(&side, "workflow=permission", &[scope::REQUEST_PERMISSION]).await;

    // While the set-up waits for the consent, an early launch fails: its channel never opens.
    let feature = w.feature.clone();
    let launch = tokio::spawn(async move { feature.launch_notification("k", -1).await });
    let (name, _, _) = next_workflow(&mut side).await;
    assert_eq!(name, "earlyLaunch");
    let (raw, _) = side.create("Microsoft::Basix::Dct::Channel::Class::Messaging,3,4,workflow=earlyLaunch");
    raw.close();

    assert!(launch.await.unwrap().is_err());
    assert_eq!(w.feature.status().unwrap().phone_state, PhoneState::Error);

    // The set-up goes on to the remote workflow, but the window sends no launchApp.
    permission.send(scope::PERMISSION_STATUS, r#"{"status":"accept"}"#);
    let (name, _, _) = loop {
        let wf = next_workflow(&mut side).await;
        if wf.0 == "remote" {
            break wf;
        }
    };

    assert_eq!(name, "remote");
    let mut orchestrator = PhoneMessage::open(&side, "workflow=remote&type=orchestrator", ORCHESTRATOR_OUT).await;
    let _ = orchestrator.next(scope::CONFIG).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(orchestrator.try_next(scope::LAUNCH_APP).is_none(), "no launch after the failed early launch");
    let _ = (&id, &mut w.events);
}

#[tokio::test(start_paused = true)]
async fn a_resume_ends_when_the_orchestrator_closes() {
    let mut w = world(SAMSUNG_NO_NEARBY & !0x2000);
    let id = w.feature.launch_app("com.example.app", None).unwrap();
    let mut up = bring_up(&mut w, &id, "com.example.app", false).await;

    up.window.send(scope::DISCONNECTING, r#"{"reason":"MaxApps","transactionId":"t-1"}"#);
    wait_until(&w.feature, "suspended", |f| state(f, &id) == Some(WindowState::Suspended)).await;

    let (feature, sid) = (w.feature.clone(), id.clone());
    let resume = tokio::spawn(async move { feature.resume(&sid).await });
    let _ = up.orchestrator.next(scope::APP_RESUMABLE).await;
    up.orchestrator.channel.close();

    let resumable = tokio::time::timeout(T, resume).await.expect("the wait ends").unwrap().unwrap();
    assert!(!resumable);
}

/// A connected phone-screen window whose phone reports its resolution, and a directory for the files
/// the tests drag.
async fn drag_world() -> (World, String, Up, tempfile::TempDir) {
    let mut w = world(SAMSUNG_NO_NEARBY);
    w.h.app.update_phone_record(|r| r.device_resolution = Some("1080,2400".to_string()));
    let id = w.feature.open_phone_screen().unwrap();
    let mut up = bring_up(&mut w, &id, PHONE_SCREEN_PACKAGE, true).await;
    connect(&w, &mut up, &id).await;
    (w, id, up, tempfile::tempdir().unwrap())
}

/// Moves the pointer out of the video until the PC asks the phone for the drag.
async fn pointer_leaves_the_video(w: &World, id: &str, up: &mut Up) {
    tokio::time::timeout(T, async {
        loop {
            w.feature.mouse_move(id, 1.5, 0.5).unwrap();
            if up.window.try_next(scope::DRAG_REQUEST_OWNERSHIP).is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("requestOwnership once the pointer leaves the video");
}

/// Drag and drop both ways on a connected window: a Mac file pulled by the phone as a blob, and a phone
/// file handed over when the pointer leaves the video, received as a blob and written where the Mac
/// destination asks. The phone → Mac half continues from the pointer state the Mac → phone half leaves.
#[tokio::test(start_paused = true)]
#[expect(clippy::too_many_lines, reason = "one drag scenario whose second half depends on the first")]
async fn drag_and_drop_both_ways() {
    use phonelink_engine::features::screen_mirroring::drag::{EnterOutcome, MacDragFile};
    use phonelink_engine::nano::channel::blob::BlobEvent;

    let (mut w, id, mut up, dir) = drag_world().await;

    // Mac → phone: folders are skipped; the file goes as one clip item with a fresh id.
    let file = dir.path().join("note.txt");
    std::fs::write(&file, b"from the mac").unwrap();
    let files = vec![
        MacDragFile { path: file.clone(), mime_type: Some("text/plain".into()) },
        MacDragFile { path: dir.path().to_path_buf(), mime_type: None },
    ];

    assert_eq!(w.feature.drag_enter(&id, files.clone()).unwrap(), EnterOutcome::Accepted);
    let start = json(&up.window.next(scope::DRAG_START_PC).await);
    assert_eq!(start["Scope"], "/drag/startPc");
    let items = start["clipItems"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!((items[0]["filename"].as_str(), items[0]["fileSize"].as_i64()), (Some("note.txt"), Some(12)));
    assert_eq!(items[0]["mimeType"], "text/plain");
    let data_id = items[0]["dataId"].as_str().unwrap().to_string();

    assert!(w.feature.drag_over(&id, 0.5, 0.5).unwrap(), "copy over the window");
    w.feature.drag_drop(&id).unwrap();

    let drop = json(&up.window.next(scope::DRAG_DROP_PC).await);
    assert_eq!(drop["correlationId"], start["correlationId"]);

    let mut pulled = up._blob.register(data_id.clone().into_bytes());
    up.window.send(scope::DRAG_REQUEST_ITEM, &format!(r#"{{"itemId":"{data_id}"}}"#));

    let mut got = Vec::new();
    loop {
        match tokio::time::timeout(T, pulled.recv()).await.unwrap().unwrap() {
            BlobEvent::Start { size } => assert_eq!(size, 12),
            BlobEvent::Chunk(c) => got.extend(c),
            BlobEvent::Completed(_) => break,
        }
    }

    assert_eq!(got, b"from the mac");

    // Over the limits: refused, nothing sent; a leave of an accepted drag sends cancel 5.
    let many: Vec<MacDragFile> = (0..101).map(|_| files[0].clone()).collect();
    assert_eq!(w.feature.drag_enter(&id, many).unwrap(), EnterOutcome::OverLimit);
    assert_eq!(w.feature.drag_enter(&id, files[..1].to_vec()).unwrap(), EnterOutcome::Accepted);

    let again = json(&up.window.next(scope::DRAG_START_PC).await);
    w.feature.drag_leave(&id).unwrap();
    let cancel = json(&up.window.next(scope::DRAG_CANCEL).await);
    assert_eq!((cancel["correlationId"].clone(), cancel["reason"].as_i64()), (again["correlationId"].clone(), Some(5)));

    // clipData with no drag handed over is an invalid pair: the PC cancels with reason 5.
    let clip = r#"{"correlationId":"c","clipDataItems":{"dataIds":["d1"],"mimeTypes":["text/plain"],"fileSizes":[5],"filenames":["x/y.txt"]}}"#;
    up.window.send(scope::DRAG_CLIP_DATA, clip);
    assert_eq!(json(&up.window.next(scope::DRAG_CANCEL).await)["reason"], 5); // Phone → Mac: leaving the video asks for the drag; the next move starts it and requests the file.
    up.window.send(scope::DRAG_START_PHONE, r#"{"correlationId":"c","mimeTypes":["text/plain"],"sessionId":""}"#);
    pointer_leaves_the_video(&w, &id, &mut up).await;
    up.window.send(scope::DRAG_CLIP_DATA, clip);
    let offered = tokio::time::timeout(T, async {
        loop {
            if let Some(items) = w.feature.take_phone_drag(&id, true).unwrap() {
                return items;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    assert_eq!(offered.len(), 1);
    assert_eq!((offered[0].name.as_str(), offered[0].size), ("x_y.txt", 5));
    assert_eq!(json(&up.window.next(scope::DRAG_REQUEST_ITEM).await)["itemId"], "d1");

    up._blob.send_stream(b"d1", 5, &b"hello"[..]).await.unwrap();
    w.feature.phone_drag_ended(&id, true).unwrap();
    let out = dir.path().join("out.txt");
    w.feature.write_phone_drag_item(&id, "d1", &out).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), b"hello");

    let ended = tokio::time::timeout(T, async {
        loop {
            if let ScreenMirroringEvent::DragEnded { session_id } = w.events.recv().await.unwrap() {
                return session_id;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ended, id, "every file written completes the drag");

    // The drop shows a progress notification with Cancel; the outcome replaces it.
    let posted = w._fw.notifier.posted();
    let drag_id = format!("screen_mirroring/drag/{id}");
    let progress = posted.iter().find(|n| n.id == drag_id && !n.actions.is_empty()).expect("a progress notification");
    assert_eq!((progress.title.as_str(), progress.actions[0].id.as_str()), ("Transferring 1 file", "cancel"));
    assert_eq!(
        posted.last().map(|n| (n.id.as_str(), n.title.as_str())),
        Some((drag_id.as_str(), "Successfully transferred 1 file"))
    );

    // The phone's cancel 11 during a drag is answered with 11.
    up.window.send(scope::DRAG_START_PHONE, r#"{"correlationId":"c2","mimeTypes":["text/plain"]}"#);
    up.window.send(scope::DRAG_CANCEL, r#"{"correlationId":"c2","reason":11}"#);
    assert_eq!(json(&up.window.next(scope::DRAG_CANCEL).await)["reason"], 11);

    // So is the progress notification's Cancel, during a dropped drag.
    up.window.send(scope::DRAG_START_PHONE, r#"{"correlationId":"c3","mimeTypes":["text/plain"]}"#);
    pointer_leaves_the_video(&w, &id, &mut up).await;
    let clip3 = r#"{"correlationId":"c3","clipDataItems":{"dataIds":["d3"],"mimeTypes":["text/plain"],"fileSizes":[5],"filenames":["z.txt"]}}"#;
    up.window.send(scope::DRAG_CLIP_DATA, clip3);
    tokio::time::timeout(T, async {
        while w.feature.take_phone_drag(&id, true).unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();

    w.feature.phone_drag_ended(&id, true).unwrap();
    w._fw.responses.route(
        &drag_id,
        &HashMap::new(),
        phonelink_engine::features::shared::notifier::NotificationResponse::Action { action_id: "cancel".into() },
    );

    assert_eq!(json(&up.window.next(scope::DRAG_CANCEL).await)["reason"], 11);
}

/// The phone's Mirror to PC: `phoneCommand` `LaunchUri` with its remoting URI.
async fn mirror_to_pc(w: &World, uri: &str) {
    let command = ValueSet::new().with("phoneCommandType", 1i32).with("launchURI", uri);
    w.h.app
        .handle_inbound(app_world::phone(), app_world::trace(), app_world::phone_content("phoneCommand", command))
        .await;
}

async fn next_launch_event(events: &mut broadcast::Receiver<ScreenMirroringEvent>) -> ScreenMirroringEvent {
    loop {
        match tokio::time::timeout(T, events.recv()).await.expect("an event in time").expect("the feature is alive") {
            ScreenMirroringEvent::Changed | ScreenMirroringEvent::SessionChanged { .. } => {}
            e => return e,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn mirror_to_pc_on_the_phone_opens_the_phone_screen_as_a_pc_launch() {
    let mut w = world(SAMSUNG_NO_NEARBY);
    mirror_to_pc(&w, "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen&cid=6f1c0d5e").await;

    let ScreenMirroringEvent::SessionOpened { session_id, package_name, phone_initiated } =
        next_launch_event(&mut w.events).await
    else {
        panic!("no window opened")
    };
    assert_eq!((package_name.as_str(), phone_initiated), (PHONE_SCREEN_PACKAGE, false));
    assert_ne!(session_id, "6f1c0d5e", "the session id is the PC's own");

    bring_up(&mut w, &session_id, PHONE_SCREEN_PACKAGE, true).await;

    // A second press raises the open window.
    mirror_to_pc(&w, "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen&cid=7a").await;
    assert_eq!(next_launch_event(&mut w.events).await, ScreenMirroringEvent::BringToFront { session_id });
}

#[tokio::test(start_paused = true)]
async fn mirror_to_pc_outside_the_gates_shows_the_main_window_or_the_apps_view() {
    let mut w = world(SAMSUNG_NO_NEARBY);
    w.h.app.update_phone_record(|r| r.reported_contract_version = Some(3.9));
    mirror_to_pc(&w, "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen").await;
    assert_eq!(next_launch_event(&mut w.events).await, ScreenMirroringEvent::ShowApps, "no phone-screen apps");

    w.h.app.update_phone_record(|r| r.capability_bits &= !0x4);
    mirror_to_pc(&w, "ms-phone:remoting?launchsource=protocol&packagename=yourphone.phonescreen").await;
    assert_eq!(next_launch_event(&mut w.events).await, ScreenMirroringEvent::ShowMainWindow, "remoting not supported");
    assert!(w.feature.sessions().is_empty());
}
