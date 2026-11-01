//! The platform router over a loopback PC ↔ phone pair: receiving, unknown or unsubscribed routes,
//! sending a request, `/internal/response` and rejection.

use crate::support::platform_world as fakes;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use fakes::*;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::platform::{InboundMessage, PlatformRouteHandler, RouteRegistrationError, Router, route};
use phonelink_engine::seams::{MessageSink, PlatformMessenger, PlatformRequestStatus};
use phonelink_engine::tap::{Direction, TapLayer, null_tap};
use phonelink_engine::types::{DcgSendStatus, MessageType, SendOptions, TraceContext};
use serde_json::json;

const GUID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn pair() -> Pair {
    Pair::new(EngineConfig::default())
}

// ------------------------------------------------------------------------------------------------
// Unknown or unsubscribed routes
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn unknown_platform_route_gets_the_not_subscribed_rejection() {
    let p = pair();
    let response = phone_request(
        &p,
        platform(&[("_route", "/No/Such/Route"), ("ms-content-type", "text/json")], b"{\"a\":\"b\"}"),
    )
    .await;
    assert_eq!(response.status, PlatformRequestStatus::RemoteNotSubscribedToRoute);
    assert_eq!(response.message, None);

    let sent = p.pc_link.sent();
    assert_eq!(sent.len(), 1, "exactly the rejection");
    let rejection = &sent[0];
    assert_eq!(rejection.target, phone_id());
    assert_eq!(rejection.message.message_type, MessageType::Platform, "same message type as the request");
    let minted = p.phone_link.sent()[0].message.headers.get("_requestId").unwrap().to_owned();
    assert_eq!(
        pairs(&rejection.message.headers),
        owned(&[
            ("_originalRequestId", minted.as_str()),
            ("_route", "/internal/response"),
            ("_rejectedReason", "NotSubscribedToRoute"),
            ("ms-content-type", "text/json"),
            ("_rejectionVersion", "1"),
        ])
    );
    assert_eq!(rejection.message.payload, b"{}");
    assert_eq!(rejection.priority, 0);
    assert_eq!(
        rejection.options,
        SendOptions { ignore_device_disabled: true, skip_wake_if_not_connected: true, ..SendOptions::NONE }
    );
    assert_eq!(rejection.connection_session_id, None);

    // The tap shows the inbound request and the outbound rejection, bodies decoded as KVP.
    let events: Vec<_> = p.pc_tap.snapshot().into_iter().filter(|e| e.layer == TapLayer::Platform).collect();
    let inbound = events.iter().find(|e| e.direction == Direction::In).unwrap();
    assert_eq!(inbound.summary["route"], "/No/Such/Route");
    assert_eq!(inbound.summary["body"], json!({"a": "b"}));

    let outbound = events.iter().find(|e| e.direction == Direction::Out).unwrap();
    assert_eq!(outbound.summary["route"], "/internal/response");
    assert_eq!(outbound.summary["body"], json!({}));
    assert_eq!(outbound.partner.as_deref(), Some("phone-22222222.Prod"));
}

#[tokio::test(start_paused = true)]
async fn a_platform_message_without_route_is_rejected_too() {
    let p = pair();
    p.inject(platform(
        &[("_requestId", "r-9"), ("_rejectionVersion", "1"), ("ms-content-type", "application/x-binary")],
        &[1, 2],
    ));
    settle().await;

    let sent = p.pc_link.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.headers.get("_originalRequestId"), Some("r-9"));
    assert_eq!(sent[0].message.headers.get("_rejectedReason"), Some("NotSubscribedToRoute"));
}

#[tokio::test(start_paused = true)]
async fn rejection_only_when_the_rejection_version_parses_to_at_least_one() {
    for (version, rejected) in [
        (None, false),
        (Some("0"), false),
        (Some("-1"), false),
        (Some("abc"), false),
        (Some(""), false),
        (Some("1.0"), false),
        (Some("1"), true),
        (Some("2"), true),
        (Some(" 2 "), true),
        (Some("+1"), true),
    ] {
        let p = pair();
        let mut h = vec![("_route", "/Nope"), ("_requestId", "7"), ("ms-content-type", "text/json")];
        if let Some(v) = version {
            h.push(("_rejectionVersion", v));
        }

        p.inject(platform(&h, b"{}"));
        settle().await;
        assert_eq!(p.pc_link.sent().len(), usize::from(rejected), "_rejectionVersion {version:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn no_request_id_means_silence_and_the_router_keeps_working() {
    let p = pair();
    p.inject(platform(&[("_route", "/Nope"), ("_rejectionVersion", "1"), ("ms-content-type", "text/json")], b"{}"));
    settle().await;
    assert!(p.pc_link.sent().is_empty(), "no reply without _requestId");

    // The router still answers and correlates.
    let echo = Scripted::install(&p.phone, "/Echo", Box::new(|_| Some(kvp_reply("{\"ok\":\"1\"}"))));
    let response =
        p.pc.request(request(phone_id(), platform(&[("_route", "/Echo"), ("ms-content-type", "text/json")], b"{}")))
            .await;
    assert_eq!(response.status, PlatformRequestStatus::Success);
    assert_eq!(response.message.unwrap().payload, b"{\"ok\":\"1\"}");
    assert_eq!(echo.count(), 1);

    p.inject(platform(
        &[("_route", "/Nope"), ("_rejectionVersion", "1"), ("_requestId", "x"), ("ms-content-type", "text/json")],
        b"{}",
    ));
    settle().await;
    assert_eq!(p.pc_link.sent_on("/internal/response").len(), 1, "a later unknown request is rejected");
}

#[tokio::test(start_paused = true)]
async fn app_messages_never_produce_a_platform_rejection() {
    let p = pair();
    p.inject(app(&[("_route", "/whatever"), ("_requestId", "9"), ("_rejectionVersion", "1")], b""));
    settle().await;
    assert!(p.pc_link.sent().is_empty());
    assert_eq!(p.pc_app.messages().len(), 1, "handed to the App layer");
}

// ------------------------------------------------------------------------------------------------
// Registration
// ------------------------------------------------------------------------------------------------

struct Panics;

#[async_trait]
impl PlatformRouteHandler for Panics {
    async fn handle(&self, _inbound: InboundMessage) {
        panic!("handler bug");
    }
}

#[tokio::test(start_paused = true)]
async fn one_handler_per_route() {
    let p = pair();
    let handler: Arc<dyn PlatformRouteHandler> = Arc::new(Panics);
    p.pc.register_route("/X", handler.clone()).unwrap();
    assert!(p.pc.is_route_registered("/X"));
    assert_eq!(p.pc.register_route("/X", handler.clone()), Err(RouteRegistrationError::AlreadyRegistered("/X".into())));
    assert_eq!(
        p.pc.register_route("/internal/response", handler.clone()),
        Err(RouteRegistrationError::Reserved("/internal/response".into()))
    );
    assert_eq!(p.pc.register_route("X", handler), Err(RouteRegistrationError::Malformed("X".into())));
}

#[tokio::test(start_paused = true)]
async fn an_unregistered_route_is_rejected_again() {
    let p = pair();
    Scripted::silent(&p.pc, "/Temporary");
    assert!(p.pc.unregister_route("/Temporary"));
    assert!(!p.pc.is_route_registered("/Temporary"));
    assert!(!p.pc.unregister_route("/Temporary"), "nothing left to remove");

    let response =
        phone_request(&p, platform(&[("_route", "/Temporary"), ("ms-content-type", "text/json")], b"{}")).await;
    assert_eq!(response.status, PlatformRequestStatus::RemoteNotSubscribedToRoute);
}

#[tokio::test(start_paused = true)]
async fn a_panicking_handler_is_isolated() {
    let p = pair();
    p.pc.register_route("/Boom", Arc::new(Panics)).unwrap();
    p.inject(platform(
        &[("_route", "/Boom"), ("_requestId", "1"), ("_rejectionVersion", "1"), ("ms-content-type", "text/json")],
        b"{}",
    ));
    settle().await;

    // Still alive: an unknown route is rejected afterwards.
    p.inject(platform(
        &[("_route", "/Nope"), ("_requestId", "2"), ("_rejectionVersion", "1"), ("ms-content-type", "text/json")],
        b"{}",
    ));
    settle().await;

    let sent = p.pc_link.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.headers.get("_originalRequestId"), Some("2"));
}

// ------------------------------------------------------------------------------------------------
// App message routing
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn app_request_is_correlated_by_the_response_route_header() {
    let p = pair();
    let pc = p.pc.clone();
    let requester = tokio::spawn(async move {
        pc.request(request(phone_id(), app(&[("_requestId", GUID), ("_route", "/connect/v1")], &[0x0A, 0x00]))).await
    });
    settle().await;

    // What the PC sent: the caller's headers in order, then _rejectionVersion; no ms-content-type.
    let sent = p.pc_link.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message.message_type, MessageType::App);
    assert_eq!(
        pairs(&sent[0].message.headers),
        owned(&[("_requestId", GUID), ("_route", "/connect/v1"), ("_rejectionVersion", "1")])
    );
    assert!(sent[0].options.skip_wake_if_not_connected, "the router adds the connection-level skip-wake");
    assert_eq!(sent[0].connection_session_id, None);

    // The phone's App layer got it; it answers with an App response.
    let (source, _, received) = p.phone_app.messages().pop().unwrap();
    assert_eq!(source, pc_id());

    let answer = app(&[("result", "0")], &[0x0A, 0x01]);
    let status = p.phone.respond(&pc_id(), &received, answer, 50, TraceContext::new_root(Vec::new())).await;
    assert_eq!(status, DcgSendStatus::Success);

    let response = requester.await.unwrap();
    assert_eq!(response.status, PlatformRequestStatus::Success);
    let message = response.message.unwrap();
    assert_eq!(message.message_type, MessageType::App, "a feature reply stays App-typed");
    assert_eq!(
        pairs(&message.headers),
        owned(&[
            ("_originalRequestId", GUID),
            ("_route", "/internal/response"),
            ("result", "0"),
            ("_rejectionVersion", "1")
        ])
    );
    assert!(p.pc_app.messages().is_empty(), "a response never reaches the App layer");
    assert_eq!(p.pc.pending_requests(), 0);
}

#[tokio::test(start_paused = true)]
async fn an_app_response_recognizable_only_from_its_payload_goes_to_the_app_layer() {
    let p = pair();
    let pc = p.pc.clone();
    let requester = tokio::spawn(async move {
        let mut r = request(phone_id(), app(&[("_requestId", GUID), ("_route", "/connect/v1")], b""));
        r.timeout = Duration::from_secs(5);
        pc.request(r).await
    });
    settle().await;

    // Header carries _originalRequestId but no _route: not a response.
    p.inject(app(&[("_originalRequestId", GUID)], b"payload-with-route-inside"));
    settle().await;

    assert_eq!(p.pc_app.messages().len(), 1);
    assert_eq!(requester.await.unwrap().status, PlatformRequestStatus::ResponseTimedOut);
}

#[tokio::test(start_paused = true)]
async fn device_proxy_messages_skip_the_app_layer() {
    let p = pair();
    // No receiver installed on a bare router: logged and dropped, never handed to the App layer.
    p.inject(app(&[("_route", route::DEVICE_PROXY), ("RequestId", "abc"), ("ChunkSize", "64")], b"{}"));
    settle().await;
    assert!(p.pc_app.messages().is_empty());
    assert!(p.pc_link.sent().is_empty());
}

#[tokio::test(start_paused = true)]
async fn without_an_app_layer_app_messages_are_dropped() {
    let config = Arc::new(EngineConfig::default());
    let link = Loopback::new(pc_id());
    let router = Router::new(config, link.clone(), null_tap());

    router.on_message(
        phone_id(),
        None,
        TraceContext::new_root(Vec::new()),
        app(&[("_route", "/legacy/phonecontent"), ("_requestId", "1")], b""),
    );
    settle().await;
    assert!(link.sent().is_empty());
}

// ------------------------------------------------------------------------------------------------
// Sending requests
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn request_ids_are_decimal_and_preset_ids_are_kept() {
    let p = pair();
    Scripted::install(&p.phone, "/Echo", Box::new(|_| Some(kvp_reply("{}"))));
    let echo = || platform(&[("_route", "/Echo"), ("ms-content-type", "text/json")], b"{}");
    assert_eq!(p.pc.request(request(phone_id(), echo())).await.status, PlatformRequestStatus::Success);

    // A pre-set id is kept and consumes no counter value.
    let preset = platform(&[("_requestId", GUID), ("_route", "/Echo"), ("ms-content-type", "text/json")], b"{}");
    assert_eq!(p.pc.request(request(phone_id(), preset)).await.status, PlatformRequestStatus::Success);
    assert_eq!(p.pc.request(request(phone_id(), echo())).await.status, PlatformRequestStatus::Success);

    let ids: Vec<String> =
        p.pc_link.sent_on("/Echo").iter().map(|m| m.message.headers.get("_requestId").unwrap().to_owned()).collect();
    assert_eq!(ids[1], GUID);
    let (first, third): (u32, u32) = (ids[0].parse().unwrap(), ids[2].parse().unwrap());
    assert!(third > first, "the counter only grows: {ids:?}");

    let first_message = &p.pc_link.sent_on("/Echo")[0];
    assert_eq!(
        pairs(&first_message.message.headers),
        owned(&[
            ("_requestId", ids[0].as_str()),
            ("_route", "/Echo"),
            ("ms-content-type", "text/json"),
            ("_rejectionVersion", "1")
        ])
    );
    assert_eq!(first_message.priority, 100);
}

#[tokio::test(start_paused = true)]
async fn the_request_id_counter_is_process_wide() {
    // A router built later in the process (an engine core rebuilt after an account removal) does not
    // restart at "1": it continues from the counter every router shares.
    let first = pair();
    first.pc_link.set_default(Behavior::Swallow);
    let echo = || {
        let mut r = request(phone_id(), platform(&[("_route", "/Echo"), ("ms-content-type", "text/json")], b"{}"));
        r.options.one_way = true;
        r
    };
    first.pc.request(echo()).await;

    let rebuilt = pair();
    rebuilt.pc_link.set_default(Behavior::Swallow);
    rebuilt.pc.request(echo()).await;

    let id = |p: &Pair| p.pc_link.sent()[0].message.headers.get("_requestId").unwrap().parse::<u32>().unwrap();
    assert!(id(&rebuilt) > id(&first));
    assert_ne!(id(&rebuilt), 1);
}

#[tokio::test(start_paused = true)]
async fn one_way_completes_on_send_success_and_drops_the_late_response() {
    let p = pair();
    let echo = Scripted::install(&p.phone, "/Echo", Box::new(|_| Some(kvp_reply("{}"))));
    let mut r = request(phone_id(), platform(&[("_route", "/Echo"), ("ms-content-type", "text/json")], b"{}"));
    r.options.one_way = true;

    let response = p.pc.request(r).await;
    assert_eq!(response.status, PlatformRequestStatus::Success);
    assert_eq!(response.message, None);

    settle().await;
    assert_eq!(echo.count(), 1, "the phone answered");
    assert_eq!(p.pc.pending_requests(), 0, "the late response matched nothing");
}

#[tokio::test(start_paused = true)]
async fn one_timeout_bounds_send_plus_wait() {
    let p = pair();
    Scripted::silent(&p.phone, "/Silent");
    let silent = || platform(&[("_route", "/Silent"), ("ms-content-type", "text/json")], b"{}");

    let start = tokio::time::Instant::now();
    assert_eq!(p.pc.request(request(phone_id(), silent())).await.status, PlatformRequestStatus::ResponseTimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(120), "PlatformRequestTimeout");

    let start = tokio::time::Instant::now();
    let mut shorter = request(phone_id(), silent());
    shorter.timeout = Duration::from_secs(5);
    assert_eq!(p.pc.request(shorter).await.status, PlatformRequestStatus::ResponseTimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(5), "a caller may pass a shorter timeout");

    let start = tokio::time::Instant::now();
    let mut longer = request(phone_id(), silent());
    longer.timeout = Duration::from_secs(600);
    assert_eq!(p.pc.request(longer).await.status, PlatformRequestStatus::ResponseTimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(120), "never longer than PlatformRequestTimeout");

    // A send that never completes is bounded by the same timeout.
    p.pc_link.script(Behavior::Hang);
    let start = tokio::time::Instant::now();
    assert_eq!(p.pc.request(request(phone_id(), silent())).await.status, PlatformRequestStatus::ResponseTimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    assert_eq!(p.pc.pending_requests(), 0);
}

#[tokio::test(start_paused = true)]
async fn cancellation_yields_canceled() {
    let p = pair();
    Scripted::silent(&p.phone, "/Silent");
    let r = request(phone_id(), platform(&[("_route", "/Silent"), ("ms-content-type", "text/json")], b"{}"));
    let cancel = r.cancel.clone();
    let pc = p.pc.clone();
    let requester = tokio::spawn(async move { pc.request(r).await });

    tokio::time::sleep(Duration::from_secs(3)).await;
    cancel.cancel();
    assert_eq!(requester.await.unwrap().status, PlatformRequestStatus::Canceled);
    assert_eq!(p.pc.pending_requests(), 0);

    // Already cancelled: nothing is sent.
    let before = p.pc_link.sent().len();
    let r = request(phone_id(), platform(&[("_route", "/Silent"), ("ms-content-type", "text/json")], b"{}"));
    r.cancel.cancel();
    assert_eq!(p.pc.request(r).await.status, PlatformRequestStatus::Canceled);
    assert_eq!(p.pc_link.sent().len(), before);
}

#[tokio::test(start_paused = true)]
async fn a_failed_send_reports_its_cause() {
    let p = pair();
    for cause in [DcgSendStatus::PartnerNotConnected, DcgSendStatus::TimedOut, DcgSendStatus::LostTrustWithPartner] {
        p.pc_link.script(Behavior::Fail(cause.clone()));
        let response =
            p.pc.request(request(phone_id(), platform(&[("_route", "/Any"), ("ms-content-type", "text/json")], b"{}")))
                .await;
        assert_eq!(response.status, PlatformRequestStatus::RequestSendFailed(cause));
    }
    assert_eq!(p.pc.pending_requests(), 0);
}

#[tokio::test(start_paused = true)]
async fn every_rejection_reason_maps() {
    use PlatformRequestStatus::*;
    let p = pair();
    let reasons = [
        ("Unspecified", NoResponse),
        ("NotAllowed", RemoteSyncNotAllowed),
        ("VersionTooLow", RemoteVersionTooLow),
        ("VersionTooHigh", RemoteVersionTooHigh),
        ("AllSyncDisabled", RemoteAllSyncDisabled),
        ("MeteredSyncDisabled", RemoteMeteredSyncDisabled),
        ("NotSubscribedToRoute", RemoteNotSubscribedToRoute),
        ("FromTheFuture", RemoteVersionTooHigh),
        // `Enum.TryParse`: numbers parse to their value, defined or not; names are trimmed and OR-ed.
        ("7", RemoteNotSubscribedToRoute),
        ("99", NoResponse),
        (" NotAllowed, VersionTooLow ", RemoteVersionTooLow),
    ];

    let reason = Arc::new(std::sync::Mutex::new(String::new()));
    let current = reason.clone();
    Scripted::install(
        &p.phone,
        "/Reject",
        Box::new(move |_| {
            let r = current.lock().unwrap().clone();
            Some(platform(&[("_rejectedReason", r.as_str()), ("ms-content-type", "text/json")], b"{}"))
        }),
    );

    for (name, status) in reasons {
        *reason.lock().unwrap() = name.to_owned();
        let response = p
            .pc
            .request(request(phone_id(), platform(&[("_route", "/Reject"), ("ms-content-type", "text/json")], b"{}")))
            .await;
        assert_eq!(response.status, status, "{name}");
        assert_eq!(response.message, None);
    }

    // `None` is not a rejection: success with the payload.
    *reason.lock().unwrap() = "None".to_owned();
    let response =
        p.pc.request(request(phone_id(), platform(&[("_route", "/Reject"), ("ms-content-type", "text/json")], b"{}")))
            .await;
    assert_eq!(response.status, Success);
    assert_eq!(response.message.unwrap().payload, b"{}");
}

#[tokio::test(start_paused = true)]
async fn responses_without_or_with_unknown_original_ids_are_dropped() {
    let p = pair();
    Scripted::silent(&p.phone, "/Slow");
    let pc = p.pc.clone();
    let requester = tokio::spawn(async move {
        pc.request(request(phone_id(), platform(&[("_route", "/Slow"), ("ms-content-type", "text/json")], b"{}"))).await
    });
    settle().await;

    p.inject(platform(&[("_route", "/internal/response"), ("ms-content-type", "text/json")], b"{}"));
    p.inject(platform(
        &[("_route", "/internal/response"), ("_originalRequestId", "999"), ("ms-content-type", "text/json")],
        b"{}",
    ));
    p.inject(app(&[("_route", "/internal/response"), ("_originalRequestId", "998")], b""));
    settle().await;

    assert_eq!(p.pc.pending_requests(), 1, "still waiting for its own response");
    assert!(p.pc_app.messages().is_empty());
    assert!(p.pc_link.sent_on("/internal/response").is_empty(), "responses are never rejected");

    // Its own response completes it.
    let minted = p.pc_link.sent_on("/Slow")[0].message.headers.get("_requestId").unwrap().to_owned();
    p.inject(platform(
        &[("_route", "/internal/response"), ("_originalRequestId", minted.as_str()), ("ms-content-type", "text/json")],
        b"{\"k\":\"v\"}",
    ));
    let response = requester.await.unwrap();
    assert_eq!(response.status, PlatformRequestStatus::Success);
    assert_eq!(response.message.unwrap().payload, b"{\"k\":\"v\"}");
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "must carry ms-content-type")]
async fn a_platform_request_without_content_type_is_a_programming_error() {
    let p = pair();
    p.pc.request(request(phone_id(), platform(&[("_route", "/X")], b"{}"))).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "never carry ms-content-type")]
async fn an_app_request_with_content_type_is_a_programming_error() {
    let p = pair();
    p.pc.request(request(phone_id(), app(&[("_route", "/x"), ("ms-content-type", "text/json")], b""))).await;
}

#[tokio::test(start_paused = true)]
#[should_panic(expected = "carries no _requestId")]
async fn responding_to_a_request_without_request_id_is_a_programming_error() {
    let p = pair();
    let unanswerable = platform(&[("_route", "/X"), ("ms-content-type", "text/json")], b"{}");
    p.pc.respond(&phone_id(), &unanswerable, kvp_reply("{}"), 0, TraceContext::new_root(Vec::new())).await;
}

#[tokio::test(start_paused = true)]
async fn request_allowing_wake_keeps_the_callers_choice() {
    let p = pair();
    p.pc_link.set_default(Behavior::Swallow);
    let mut r = request(
        phone_id(),
        platform(&[("_route", "/CryptoTrustKeyRotationCheck"), ("ms-content-type", "text/json")], b"{}"),
    );
    r.options.one_way = true;

    assert_eq!(p.pc.request_allowing_wake(r).await.status, PlatformRequestStatus::Success);
    assert!(!p.pc_link.sent()[0].options.skip_wake_if_not_connected);
}
