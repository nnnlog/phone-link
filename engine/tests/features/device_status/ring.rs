//! Ring my phone — the ring capability from the connect reply, when the control is offered, the
//! `RING_MY_PHONE` request through the App layer and how its outcome is mapped, the 20 s countdown,
//! the two-step stop and the ringing state a failed stop brings back.

use crate::support::app_world;
use crate::support::device_status_world::fixture;
use phonelink_engine::config::features::DeviceStatusConfig;

use std::time::Duration;

use app_world::*;
use fixture::*;
use phonelink_engine::app::{AppOutcome, PhoneRecord, ResourceFailure};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::edge::LockState;
use phonelink_engine::features::device_status::{ControlOutcome, Prompt, RingControl, RingRequest, RingShown};
use phonelink_engine::proto::platform_sdk::v1::{
    DeviceResourceManagerMessage, DeviceResourceManagerResponseMessage, DeviceResourceManagerResponseMessageType,
};
use phonelink_engine::proto::ring_my_phone::v1::{RingMyPhoneRequestMessage, RingMyPhoneResponseMessage};
use phonelink_engine::seams::{
    ConnectionFailure, PlatformRequest, PlatformRequestStatus, PlatformResponse, SessionEvent,
};
use phonelink_engine::types::{HeaderMap, MessageType, TransportMessage};
use prost::Message;

const DRM: &str = "/DeviceResourceManager";

/// A reply of the phone's `/DeviceResourceManager` receiver carrying `status`.
fn drm_reply(status: i32, response_type: DeviceResourceManagerResponseMessageType) -> PlatformResponse {
    let body = DeviceResourceManagerResponseMessage {
        payload: RingMyPhoneResponseMessage { status }.encode_to_vec(),
        response_type: Some(response_type as i32),
    }
    .encode_to_vec();

    let mut headers = HeaderMap::new();
    headers.insert("_originalRequestId", "1");
    headers.insert("_route", "/internal/response");

    PlatformResponse {
        status: PlatformRequestStatus::Success,
        message: Some(TransportMessage::new(MessageType::Platform, headers, body)),
    }
}

fn ok_reply() -> PlatformResponse {
    drm_reply(1, DeviceResourceManagerResponseMessageType::Success)
}

/// A fixture whose phone announced it can ring and whose DRM requests are answered `OK`.
async fn ringable_with(config: DeviceStatusConfig) -> Fixture {
    let f = Fixture::with_config(config);
    f.h.messenger.answer_all(ok_reply());
    f.h.app.update_phone_record(|r| {
        r.apply_device_status_connect_reply(
            &ValueSet::new().with("capabilities", ValueSet::new().with("ringMyPhoneCapabilities", 1i32)),
        )
    });
    settle().await;
    f
}

async fn ringable() -> Fixture {
    ringable_with(DeviceStatusConfig::default()).await
}

fn ring(f: &Fixture) -> Option<RingControl> {
    f.status().ring_my_phone
}

fn shown(f: &Fixture) -> RingShown {
    ring(f).expect("the ring control exists").shown
}

fn ringing(remaining_seconds: u32, confirm_stop: bool) -> RingShown {
    RingShown::Ringing { remaining_seconds, confirm_stop }
}

/// The ring requests sent so far, each checked for its envelope, and decoded.
fn ring_requests(f: &Fixture) -> Vec<i32> {
    f.h.messenger
        .requests_on(DRM)
        .iter()
        .map(|req| {
            check_envelope(req);
            let m = DeviceResourceManagerMessage::decode(req.message.payload.as_slice()).unwrap();
            assert_eq!((m.resource_type, m.request_type, m.resource_path.as_str()), (7, 2, "/ringmyphone"));
            RingMyPhoneRequestMessage::decode(m.payload.as_slice()).unwrap().request_type
        })
        .collect()
}

fn check_envelope(req: &PlatformRequest) {
    assert_eq!(req.message.message_type, MessageType::Platform);
    let headers: Vec<_> = req.message.headers.iter().collect();
    assert_eq!(headers, vec![("_route", DRM), ("ms-content-type", "application/x-binary")]);
    assert_eq!(req.priority, 100, "protobuf-bodied platform messages");
    assert!(req.options.ignore_device_disabled);

    let state: Vec<(&str, &str)> = req.trace.trace_state.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        &state[..3],
        [
            ("ypTriggerId", "ToggleRingMyPhoneIndicator"),
            ("ypScenarioId", "SendRingMyPhoneRequest"),
            ("isuserinitiated", "True")
        ]
    );
    assert_eq!(state[3].0, "isForeground");
}

// ------------------------------------------------------------------------------------------------
// The capability and when the control is offered
// ------------------------------------------------------------------------------------------------

#[test]
fn the_connect_reply_says_whether_the_phone_can_ring() {
    let caps = |v: Value| ValueSet::new().with("capabilities", ValueSet::new().with("ringMyPhoneCapabilities", v));
    let apply = |capable: bool, reply: &ValueSet| {
        let mut r = PhoneRecord::new(&phone());
        r.ring_my_phone_capable = capable;
        r.apply_device_status_connect_reply(reply);
        r.ring_my_phone_capable
    };

    assert!(apply(false, &caps(Value::Int32(1))));
    assert!(!apply(true, &caps(Value::Int32(0))));
    for other in [Value::Int32(2), Value::Int32(-1), Value::Int64(1), Value::Bool(true), Value::from("1")] {
        assert!(!apply(true, &caps(other.clone())), "{other:?}");
    }

    // `capabilities` without the key: cannot ring.
    assert!(!apply(true, &ValueSet::new().with("capabilities", ValueSet::new().with("statusCapabilities", 4i32))));
    // No `capabilities` object: unchanged.
    assert!(apply(true, &ValueSet::new()));
    assert!(apply(true, &ValueSet::new().with("capabilities", 1i32)));
    assert!(!PhoneRecord::new(&phone()).ring_my_phone_capable, "a new record cannot ring");
}

#[tokio::test(start_paused = true)]
async fn the_control_exists_for_a_ringable_android_phone_of_the_account() {
    let f = Fixture::new();
    f.h.messenger.answer_all(ok_reply());
    assert_eq!(ring(&f), None, "the phone has not said it can ring");
    assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::NotAvailable);

    let f = ringable().await;
    assert_eq!(ring(&f), Some(RingControl { shown: RingShown::Off, enabled: true, can_stop: false }));

    // No device action strip.
    f.h.app.update_phone_record(|r| r.status_capabilities &= !0x4);
    assert_eq!(ring(&f), None);
    f.h.app.update_phone_record(|r| r.status_capabilities |= 0x4);

    // Not Android.
    f.h.app.update_phone_record(|r| r.platform = 2);
    assert_eq!(ring(&f), None);
    f.h.app.update_phone_record(|r| r.platform = 1);
    assert!(ring(&f).is_some());

    // Not in the account's device list.
    f.fw.peers.remove(&phone());
    assert_eq!(ring(&f), None);

    // The option off.
    let f = ringable_with(DeviceStatusConfig { ring_my_phone_enabled: false, ..DeviceStatusConfig::default() }).await;
    assert_eq!(ring(&f), None);
}

#[tokio::test(start_paused = true)]
async fn the_control_is_unknown_while_the_phone_is_away_and_needs_no_permission() {
    let f = ringable().await;
    // Neither the post-notification nor the notifications-sync permission is granted.
    assert!(ring(&f).unwrap().enabled);

    f.h.sessions.set_connected(false);
    f.h.sessions
        .send_event(SessionEvent::Disconnected { partner: phone(), failure: ConnectionFailure::ConnectionDropped });
    settle().await;

    assert_eq!(ring(&f), Some(RingControl { shown: RingShown::Unknown, enabled: false, can_stop: false }));
    assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::NotAvailable);

    f.h.sessions.set_connected(true);
    f.h.sessions.send_event(SessionEvent::Connected(phone()));
    settle().await;

    assert_eq!(shown(&f), RingShown::Off);

    f.h.app.update_phone_record(|r| r.all_features_disabled = true);
    settle().await;

    assert_eq!(ring(&f).unwrap().shown, RingShown::Unknown);
    assert!(ring_requests(&f).is_empty());
}

// ------------------------------------------------------------------------------------------------
// Ring and countdown
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn ring_sends_on_and_counts_down_20_s() {
    let mut f = ringable().await;
    f.take_events();

    assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::RingSent(RingRequest::On));
    assert_eq!(shown(&f), ringing(20, false));
    assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::NotAvailable, "not while ringing");
    settle().await;
    assert_eq!(ring_requests(&f), vec![1]);
    assert!(ring(&f).unwrap().can_stop);

    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(shown(&f), ringing(19, false));
    tokio::time::sleep(Duration::from_millis(18_000)).await;
    assert_eq!(shown(&f), ringing(1, false));
    let ticks = f.take_events().len();
    assert!(ticks >= 19, "one change per second ({ticks})");

    tokio::time::sleep(Duration::from_millis(1_000)).await;
    assert_eq!(shown(&f), RingShown::Off);
    assert_eq!(ring_requests(&f), vec![1], "nothing is sent when the countdown ends");

    // The next ring counts the full 20 s again.
    assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::RingSent(RingRequest::On));
    assert_eq!(shown(&f), ringing(20, false));
}

#[tokio::test(start_paused = true)]
async fn the_ring_outcome_and_reply_status_do_not_change_the_control() {
    for reply in [
        drm_reply(2, DeviceResourceManagerResponseMessageType::Success), // FEATURE_NOT_SUPPORT
        drm_reply(1, DeviceResourceManagerResponseMessageType::ResourceHandlerNotRegistered),
        status_response(PlatformRequestStatus::RemoteNotSubscribedToRoute),
        status_response(PlatformRequestStatus::ResponseTimedOut),
        status_response(PlatformRequestStatus::Success), // no reply message
    ] {
        let f = ringable().await;
        f.h.messenger.answer_all(reply);

        assert_eq!(f.ds.ring_my_phone(&phone()), ControlOutcome::RingSent(RingRequest::On));
        settle().await;
        assert_eq!(ring_requests(&f), vec![1]);
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(shown(&f), ringing(15, false));
    }
}

#[tokio::test(start_paused = true)]
async fn a_disconnect_ends_the_ring() {
    let f = ringable().await;
    f.ds.ring_my_phone(&phone());
    tokio::time::sleep(Duration::from_secs(3)).await;

    f.h.sessions.set_connected(false);
    f.h.sessions
        .send_event(SessionEvent::Disconnected { partner: phone(), failure: ConnectionFailure::ConnectionDropped });
    settle().await;
    assert_eq!(shown(&f), RingShown::Unknown);

    f.h.sessions.set_connected(true);
    f.h.sessions.send_event(SessionEvent::Connected(phone()));
    settle().await;
    assert_eq!(shown(&f), RingShown::Off, "a ring is not resumed");
}

// ------------------------------------------------------------------------------------------------
// Stop
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn stop_asks_once_then_sends_off() {
    let f = ringable().await;
    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::NotAvailable, "not ringing");

    f.ds.ring_my_phone(&phone());
    settle().await;
    tokio::time::sleep(Duration::from_secs(4)).await;

    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::Prompt(Prompt::StopRinging));
    assert_eq!(shown(&f), ringing(16, true));
    settle().await;
    assert_eq!(ring_requests(&f), vec![1], "the confirmation sends nothing");

    // Accepting the confirmation is the second stop.
    f.ds.accept_prompt(&phone(), Prompt::StopRinging);
    assert_eq!(shown(&f), RingShown::Off);
    settle().await;
    assert_eq!(ring_requests(&f), vec![1, 2]);
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(shown(&f), RingShown::Off);

    // A declined confirmation keeps it: the next stop sends at once.
    f.ds.ring_my_phone(&phone());
    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::Prompt(Prompt::StopRinging));
    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::RingSent(RingRequest::Off));
    settle().await;
    assert_eq!(ring_requests(&f), vec![1, 2, 1, 2]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_stop_shows_the_phone_ringing_again() {
    let f = ringable().await;
    f.ds.ring_my_phone(&phone());
    settle().await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    f.h.messenger.answer_all(status_response(PlatformRequestStatus::ResponseTimedOut));
    f.ds.stop_ringing(&phone());
    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::RingSent(RingRequest::Off));
    assert_eq!(shown(&f), RingShown::Off);

    settle().await;
    assert_eq!(shown(&f), ringing(20, true), "restored, confirmed, with a new countdown");

    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(shown(&f), RingShown::Off);

    // A stop the phone answered — even FEATURE_NOT_SUPPORT — is a success.
    f.h.messenger.answer_all(drm_reply(2, DeviceResourceManagerResponseMessageType::Success));
    f.ds.ring_my_phone(&phone());
    f.ds.stop_ringing(&phone());
    f.ds.stop_ringing(&phone());

    settle().await;
    assert_eq!(shown(&f), RingShown::Off);
}

#[tokio::test(start_paused = true)]
async fn without_the_stop_option_the_ring_runs_out() {
    let f =
        ringable_with(DeviceStatusConfig { stop_ring_my_phone_enabled: false, ..DeviceStatusConfig::default() }).await;
    f.ds.ring_my_phone(&phone());
    assert_eq!(ring(&f), Some(RingControl { shown: ringing(20, false), enabled: true, can_stop: false }));
    assert_eq!(f.ds.stop_ringing(&phone()), ControlOutcome::NotAvailable);

    tokio::time::sleep(Duration::from_secs(20)).await;
    settle().await;
    assert_eq!(shown(&f), RingShown::Off);
}

// ------------------------------------------------------------------------------------------------
// The App layer's DRM request
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_drm_request_maps_outcomes_like_phone_links_sender() {
    use phonelink_engine::features::device_status::ring::drm_request;
    let f = ringable().await;
    let send = |r: PlatformResponse| {
        f.h.messenger.answer_all(r);
        let app = f.h.app.clone();
        async move { app.request_drm(drm_request(RingRequest::On, Duration::from_secs(120))).await.unwrap() }
    };

    let ok = send(ok_reply()).await;
    assert_eq!((ok.outcome, ok.result), (AppOutcome::Success, Some(0)));
    assert_eq!(ok.resource_payload, Some(vec![0x08, 0x01]));

    for (reply, outcome) in [
        (
            drm_reply(1, DeviceResourceManagerResponseMessageType::ResourceHandlerNotRegistered),
            AppOutcome::ResourceFailed(ResourceFailure::PayloadError),
        ),
        (
            status_response(PlatformRequestStatus::RemoteNotSubscribedToRoute),
            AppOutcome::ResourceFailed(ResourceFailure::PayloadError),
        ),
        (
            status_response(PlatformRequestStatus::RemoteMeteredSyncDisabled),
            AppOutcome::ResourceFailed(ResourceFailure::RemoteMeteredSyncDisabled),
        ),
        (
            status_response(PlatformRequestStatus::RemoteSyncNotAllowed),
            AppOutcome::ResourceFailed(ResourceFailure::RemoteSyncNotAllowed),
        ),
        (
            status_response(PlatformRequestStatus::RemoteVersionTooLow),
            AppOutcome::ResourceFailed(ResourceFailure::NotAllowed),
        ),
        (status_response(PlatformRequestStatus::NoResponse), AppOutcome::TimedOut),
        (status_response(PlatformRequestStatus::Success), AppOutcome::ResourceFailed(ResourceFailure::PayloadError)),
    ] {
        let r = send(reply).await;
        assert_eq!((r.outcome, r.resource_payload), (outcome, None));
    }

    assert!(!f.record().all_features_disabled);
    let r = send(status_response(PlatformRequestStatus::RemoteAllSyncDisabled)).await;
    assert_eq!(r.outcome, AppOutcome::ResourceFailed(ResourceFailure::RemoteAllSyncDisabled));
    assert!(f.record().all_features_disabled, "LTW disabled sets the flag");

    let _ = send(ok_reply()).await;
    assert!(!f.record().all_features_disabled, "a success clears it");
}

#[tokio::test(start_paused = true)]
async fn the_drm_request_passes_the_app_layers_gates_and_resets_the_wifi_state_on_failure() {
    use phonelink_engine::features::device_status::ring::drm_request;
    let f = ringable().await;
    let wifi = |f: &Fixture| f.record().status_keys.wifi_state;
    let request = || drm_request(RingRequest::On, Duration::from_secs(120));

    assert_eq!(f.status_item(ValueSet::new().with("wifiState", true)).await, ValueSet::new().with("result", 0i32));
    let sent_before = f.h.messenger.requests_on(DRM).len();

    // The lock gate: nothing sent, Wi-Fi kept.
    f.h.lock.set(LockState::Locked);
    let r = f.h.app.request_drm(request()).await.unwrap();
    assert_eq!(r.outcome, AppOutcome::Canceled);
    f.h.lock.set(LockState::Unlocked);
    assert_eq!(f.h.messenger.requests_on(DRM).len(), sent_before);
    assert_eq!(wifi(&f), Some(true));

    // A success keeps it.
    f.h.messenger.answer_all(ok_reply());
    assert!(f.h.app.request_drm(request()).await.unwrap().outcome.is_success());
    assert_eq!(wifi(&f), Some(true));

    // A failure resets it.
    f.h.messenger.answer_all(status_response(PlatformRequestStatus::ResponseTimedOut));
    f.h.app.request_drm(request()).await.unwrap();
    assert_eq!(wifi(&f), None);

    // A not-connected phone is connected first, with the user-initiated policy.
    f.h.messenger.answer_all(ok_reply());
    f.h.sessions.set_connected(false);
    f.h.sessions.set_behavior(ConnectBehavior::Connect);
    assert!(f.h.app.request_drm(request()).await.unwrap().outcome.is_success());
    let (_, connect) = f.h.sessions.connects().pop().expect("a connect");
    assert_eq!(connect.policy, phonelink_engine::seams::StartPolicy::NewSession);
    assert!(connect.ignore_device_disabled);
}
