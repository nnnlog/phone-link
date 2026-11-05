//! The platform routes over a loopback PC ↔ phone pair: `/SessionValidation`, session control
//! (inbound disconnect), key rotation, `/Context/Publish`, and `/DeviceResourceManager` (receivers,
//! the request wrapper and the device-proxy variant).

use crate::assert_matches;
use crate::support::platform_world as fakes;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use fakes::*;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::LockState;
use phonelink_engine::platform::{
    AcceptedCapabilities, AnswerDelivery, DeviceFilter, DrmAddress, DrmAnswer, DrmChannel, DrmFollowUpReceiver,
    DrmReceiver, DrmReceiverError, DrmRegistrationError, DrmRequest, DrmSendOutcome, DrmSendRequest, Publication,
    PublicationObserver, PublishOutcome, SessionPhase, ValidationOutcome, route,
};
use phonelink_engine::proto::platform_sdk::v1 as pb;
use phonelink_engine::seams::{
    ConnectOutcome, ConnectionFailure, MessageSink, PartnerSessionControl, PlatformMessenger, PlatformRequestStatus,
    SessionEvent, TrustRotationError,
};
use phonelink_engine::types::{
    DcgSendStatus, HubState, MessageType, PartnerDisconnectReason, SendOptions, TraceContext, WakeStatus,
};
use prost::Message;
use tokio_util::sync::CancellationToken;

fn world() -> World {
    World::new(EngineConfig::default())
}

fn kvp_body(response: &phonelink_engine::seams::PlatformResponse) -> serde_json::Value {
    serde_json::from_slice(&response.message.as_ref().expect("a response").payload).unwrap()
}

// ------------------------------------------------------------------------------------------------
// /SessionValidation
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn session_validation_on_connect_sends_the_baseline_request_and_caches() {
    let w = world();
    w.connect_phone().await;

    let request = w.phone_validation.next().await;
    assert_eq!(request.source, pc_id());
    assert_eq!(request.message.message_type, MessageType::Platform);

    // A decimal id from the process-wide counter (other tests of this process draw from it too).
    let minted = request.message.headers.get("_requestId").unwrap().to_owned();
    assert!(minted.parse::<u32>().is_ok(), "{minted}");
    assert_eq!(
        pairs(&request.message.headers),
        owned(&[
            ("_requestId", minted.as_str()),
            ("_route", "/SessionValidation"),
            ("ms-content-type", "application/x-binary"),
            ("_rejectionVersion", "1")
        ])
    );
    assert_eq!(request.message.payload, vec![0x0A, 0x01, 0x01], "[SESSION_VALIDATION]");

    let sent = &w.pair.pc_link.sent_on(route::SESSION_VALIDATION)[0];
    assert_eq!(sent.priority, 0);
    assert_eq!(sent.options, SendOptions { skip_wake_if_not_connected: true, ..SendOptions::NONE });

    settle().await;
    let cached = w.routes.validator.cached(&phone_id()).expect("validated and cached");
    assert_eq!(
        cached.capabilities,
        Some(AcceptedCapabilities {
            capabilities: vec![pb::ProtoPlatformCapabilities::SessionValidation],
            nano_transport_preference_version: None
        })
    );

    // Cached: no second request.
    assert!(matches!(w.routes.validator.validate(&phone_id()).await, ValidationOutcome::Validated(_)));
    assert_eq!(w.phone_validation.count(), 1);

    // The decode seam shows both bodies decoded from their protobufs.
    let events = w.pair.pc_tap.snapshot();
    let out = events
        .iter()
        .find(|e| e.direction == phonelink_engine::tap::Direction::Out && e.summary["route"] == "/SessionValidation")
        .expect("tapped request");
    assert_eq!(
        out.summary["body"]["platform_capabilities"],
        serde_json::json!(["PROTO_PLATFORM_CAPABILITIES_SESSION_VALIDATION"])
    );
    assert_eq!(out.bytes_hex.as_deref(), Some("0a0101"));
    let inbound = events
        .iter()
        .find(|e| e.direction == phonelink_engine::tap::Direction::In && e.summary["route"] == "/internal/response")
        .expect("tapped response");
    assert_eq!(inbound.summary["body"]["nano_transport_preference_version"], 3);
    assert_eq!(inbound.summary["body"]["persistent_messaging_channel_version"], 14);
}

#[tokio::test(start_paused = true)]
async fn session_validation_with_nano_transport_preference() {
    let mut config = EngineConfig::default();
    config.platform.enable_nano_transport_preference = true;
    let w = World::new(config);
    w.connect_phone().await;

    let request = w.phone_validation.next().await;
    assert_eq!(request.message.payload, vec![0x0A, 0x02, 0x01, 0x03]);

    settle().await;
    assert_eq!(
        w.routes.validator.cached(&phone_id()).unwrap().capabilities.unwrap().nano_transport_preference_version,
        Some(3)
    );

    // A phone whose nano version is too low fails hard and caches nothing.
    let low = pb::ProtoPlatformSessionValidationResponse {
        platform_capabilities: vec![1, 3],
        persistent_messaging_channel_version: 0,
        nano_transport_preference_version: 2,
    }
    .encode_to_vec();

    w.phone_validation
        .set_reply(Box::new(move |_| Some(platform(&[("ms-content-type", "application/x-binary")], &low))));

    w.routes.validator.on_partner_connected(&phone_id());
    settle().await;

    assert_eq!(w.phone_validation.count(), 2);
    assert_eq!(w.routes.validator.cached(&phone_id()), None);
    assert_eq!(w.routes.validator.validate(&phone_id()).await, ValidationOutcome::NanoTransportPreferenceVersionTooLow);
}

#[tokio::test(start_paused = true)]
async fn session_validation_rejects_a_response_without_session_validation_and_retries_on_the_next_trigger() {
    let w = world();
    let without =
        pb::ProtoPlatformSessionValidationResponse { platform_capabilities: vec![2, 3], ..Default::default() }
            .encode_to_vec();

    w.phone_validation
        .set_reply(Box::new(move |_| Some(platform(&[("ms-content-type", "application/x-binary")], &without))));

    w.connect_phone().await;
    settle().await;

    assert_eq!(w.phone_validation.count(), 1);
    assert_eq!(w.routes.validator.cached(&phone_id()), None, "no cached context source");
    assert_eq!(w.routes.validator.validate(&phone_id()).await, ValidationOutcome::NotSupported);
    assert_eq!(w.phone_validation.count(), 2, "validated again: no retry inside the handshake, but no cache either");

    // The context source is created without validation and the session stays up.
    assert_eq!(w.routes.validator.context_source(&phone_id()).await.capabilities, None);
    assert!(w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn session_validation_failure_mapping() {
    let w = world();

    // An empty payload (the on-connect validation gets it; nothing is cached).
    w.phone_validation.set_reply(Box::new(|_| Some(platform(&[("ms-content-type", "application/x-binary")], b""))));
    w.connect_phone().await;
    settle().await;

    let validator = w.routes.validator.clone();
    assert_eq!(validator.validate(&phone_id()).await, ValidationOutcome::NotSupported);
    assert_eq!(w.phone_validation.count(), 2);

    // A rejection by name.
    for (reason, outcome) in [
        ("NotSubscribedToRoute", ValidationOutcome::NotSupported),
        ("VersionTooLow", ValidationOutcome::NotSupported),
        ("Unspecified", ValidationOutcome::NotSupported),
        ("AllSyncDisabled", ValidationOutcome::Failed(PlatformRequestStatus::RemoteAllSyncDisabled)),
        ("MeteredSyncDisabled", ValidationOutcome::Failed(PlatformRequestStatus::RemoteMeteredSyncDisabled)),
    ] {
        w.phone_validation.set_reply(Box::new(move |_| {
            Some(platform(&[("_rejectedReason", reason), ("ms-content-type", "text/json")], b"{}"))
        }));
        assert_eq!(validator.validate(&phone_id()).await, outcome, "{reason}");
    }

    // A failed send keeps its cause.
    w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::LostTrustWithPartner));
    assert_eq!(
        validator.validate(&phone_id()).await,
        ValidationOutcome::Failed(PlatformRequestStatus::RequestSendFailed(DcgSendStatus::LostTrustWithPartner))
    );

    // No answer: ResponseTimedOut after PlatformRequestTimeout.
    w.phone_validation.set_reply(Box::new(|_| None));
    let start = tokio::time::Instant::now();
    assert_eq!(
        validator.validate(&phone_id()).await,
        ValidationOutcome::Failed(PlatformRequestStatus::ResponseTimedOut)
    );
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    assert!(w.sessions.is_connected(&phone_id()), "validation failure never tears down the session");
}

#[tokio::test(start_paused = true)]
async fn a_context_source_without_capabilities_keeps_its_refresh_unless_the_partner_was_unreachable() {
    let w = world();
    validation_not_supported(&w);
    w.connect_phone().await;
    settle().await;
    let validator = w.routes.validator.clone();

    // Validation does not succeed, so the source is created without capabilities.
    assert_eq!(validator.context_source(&phone_id()).await.capabilities, None);
    assert_eq!(w.phone_validation.count(), 2, "the on-connect validation and the context source's");

    // A refresh the phone rejects for disabled sync is not kept: the next caller asks again.
    for reason in ["AllSyncDisabled", "MeteredSyncDisabled"] {
        w.phone_validation.set_reply(Box::new(move |_| {
            Some(platform(&[("_rejectedReason", reason), ("ms-content-type", "text/json")], b"{}"))
        }));
        let before = w.phone_validation.count();
        assert!(matches!(validator.validate(&phone_id()).await, ValidationOutcome::Failed(_)), "{reason}");
        assert!(matches!(validator.validate(&phone_id()).await, ValidationOutcome::Failed(_)), "{reason}");
        assert_eq!(w.phone_validation.count(), before + 2, "{reason}: asked twice");
    }

    // Nor is a refresh whose send found the partner not connected.
    w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::PartnerNotConnected));
    assert_eq!(
        validator.validate(&phone_id()).await,
        ValidationOutcome::Failed(PlatformRequestStatus::RequestSendFailed(DcgSendStatus::PartnerNotConnected))
    );

    // Any other outcome is kept until the source goes: no second request.
    validation_not_supported(&w);
    let before = w.phone_validation.count();
    assert_eq!(validator.validate(&phone_id()).await, ValidationOutcome::NotSupported);
    w.phone_validation.set_reply(Box::new(|_| Some(full_validation_response())));
    assert_eq!(validator.validate(&phone_id()).await, ValidationOutcome::NotSupported, "the kept outcome");
    assert_eq!(validator.context_source(&phone_id()).await.capabilities, None);
    assert_eq!(w.phone_validation.count(), before + 1);

    // The source goes with the session; the next connect validates afresh.
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    w.connect_phone().await;
    settle().await;
    assert_eq!(w.phone_validation.count(), before + 2);
    assert_eq!(validator.cached(&phone_id()).unwrap().capabilities, validated_baseline());
}

#[tokio::test(start_paused = true)]
async fn a_capability_refresh_that_succeeds_records_the_capabilities_on_the_source() {
    let w = world();
    validation_not_supported(&w);
    w.connect_phone().await;
    settle().await;
    let validator = w.routes.validator.clone();
    assert_eq!(validator.context_source(&phone_id()).await.capabilities, None);

    w.phone_validation.set_reply(Box::new(|_| Some(full_validation_response())));
    assert_eq!(validator.validate(&phone_id()).await, ValidationOutcome::Validated(validated_baseline().unwrap()));
    assert_eq!(validator.cached(&phone_id()).unwrap().capabilities, validated_baseline());

    let count = w.phone_validation.count();
    assert!(matches!(validator.validate(&phone_id()).await, ValidationOutcome::Validated(_)));
    assert_eq!(w.phone_validation.count(), count, "cached");
}

#[tokio::test(start_paused = true)]
async fn concurrent_callers_share_one_validation() {
    let w = world();
    w.phone_validation.set_reply(Box::new(|_| None));
    w.connect_phone().await;
    let first = w.phone_validation.next().await; // the on-connect validation, unanswered

    let validator = w.routes.validator.clone();
    let a = tokio::spawn({
        let v = validator.clone();
        async move { v.validate(&phone_id()).await }
    });
    let b = tokio::spawn({
        let v = validator.clone();
        async move { v.validate(&phone_id()).await }
    });

    settle().await;
    assert_eq!(w.phone_validation.count(), 1, "the callers joined the validation in flight");

    w.pair
        .phone
        .respond(&pc_id(), &first.message, full_validation_response(), 0, TraceContext::new_root(Vec::new()))
        .await;

    assert!(matches!(a.await.unwrap(), ValidationOutcome::Validated(_)));
    assert!(matches!(b.await.unwrap(), ValidationOutcome::Validated(_)));
    assert!(w.routes.validator.cached(&phone_id()).is_some());
}

#[tokio::test(start_paused = true)]
async fn a_second_connect_after_a_disconnect_validates_again() {
    let w = world();
    w.connect_phone().await;
    settle().await;
    assert_eq!(w.phone_validation.count(), 1);
    assert!(w.routes.validator.cached(&phone_id()).is_some());

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert!(!w.sessions.is_connected(&phone_id()));
    assert_eq!(w.routes.validator.cached(&phone_id()), None, "forgotten with the session");

    // The phone comes back: "once per partner connect" validates it again.
    w.connect_phone().await;
    settle().await;
    assert_eq!(w.phone_validation.count(), 2, "validated again on the new connect");
    assert!(w.routes.validator.cached(&phone_id()).is_some());
}

#[tokio::test(start_paused = true)]
async fn session_validation_gate_off_sends_nothing() {
    let mut config = EngineConfig::default();
    config.platform.session_validation_enabled = false;
    let w = World::new(config);

    w.connect_phone().await;
    settle().await;

    assert_eq!(w.phone_validation.count(), 0);
    assert_eq!(w.routes.validator.validate(&phone_id()).await, ValidationOutcome::NotSupported);
    assert_eq!(w.routes.validator.context_source(&phone_id()).await.capabilities, None);
    assert!(w.pair.pc_link.sent_on(route::SESSION_VALIDATION).is_empty());
}

#[tokio::test(start_paused = true)]
async fn session_validation_needs_a_connected_partner_but_a_context_source_does_not() {
    let w = world();
    assert_eq!(w.routes.validator.validate(&phone_id()).await, ValidationOutcome::NotConnected);

    // Session validation: "not connected" is a validation that did not succeed,
    // so the context source is created without validation (and cached, with its route).
    let source = w.routes.validator.context_source(&phone_id()).await;
    assert_eq!((source.partner.clone(), source.capabilities), (phone_id(), None));
    assert_eq!(w.routes.validator.cached(&phone_id()).map(|c| c.capabilities), Some(None));
    assert!(w.pair.pc.is_route_registered("/Context/Message/phone-22222222"));
    assert!(w.pair.pc_link.sent().is_empty(), "nothing sent");
}

#[tokio::test(start_paused = true)]
async fn inbound_session_validation_and_activate_are_rejected() {
    let w = world();
    for r in [
        route::SESSION_VALIDATION,
        route::PLATFORM_ACTIVATE_SESSION_REQUEST,
        "/Context/Message",
        "/Context/Message/phone-22222222",
    ] {
        let response = phone_request(
            &w.pair,
            platform(&[("_route", r), ("ms-content-type", "application/x-binary")], &[0x0A, 0x01, 0x01]),
        )
        .await;
        assert_eq!(response.status, PlatformRequestStatus::RemoteNotSubscribedToRoute, "{r}");
    }
}

// ------------------------------------------------------------------------------------------------
// /Context/Message/{partnerId}
// ------------------------------------------------------------------------------------------------

fn context_message(
    route: &str,
    request_id: &str,
    message: pb::ContextSourceMessage,
) -> phonelink_engine::types::TransportMessage {
    platform(
        &[
            ("_route", route),
            ("_requestId", request_id),
            ("_rejectionVersion", "1"),
            ("ms-content-type", "application/x-binary"),
        ],
        &message.encode_to_vec(),
    )
}

fn error_info(trace_id: Option<&str>) -> pb::ContextSourceMessage {
    let trace_context = trace_id.map(|id| pb::ProtoTraceContext { trace_id: id.into(), ..Default::default() });
    pb::ContextSourceMessage {
        r#type: Some(pb::context_source_message::Type::CsErrorInfo(pb::CsErrorInfo {
            context_source_error_type: "Unknown".into(),
            trace_context,
        })),
    }
}

#[tokio::test(start_paused = true)]
async fn a_context_source_answers_its_partners_context_messages() {
    let w = world();
    let own_route = "/Context/Message/phone-22222222";
    assert!(!w.pair.pc.is_route_registered(own_route));
    w.connect_phone().await; // the on-connect validation creates the context source
    settle().await;
    assert!(w.pair.pc.is_route_registered(own_route));

    let reply_to = |id: &str| {
        w.pair
            .pc_link
            .sent_on(route::INTERNAL_RESPONSE)
            .into_iter()
            .find(|m| m.message.headers.get("_originalRequestId") == Some(id))
    };

    w.pair.inject(context_message(own_route, "ok", error_info(Some("0af7651916cd43dd8448eb211c80319c"))));
    w.pair.inject(context_message(own_route, "no-trace", error_info(None)));
    w.pair.inject(platform(
        &[
            ("_route", own_route),
            ("_requestId", "garbage"),
            ("_rejectionVersion", "1"),
            ("ms-content-type", "application/x-binary"),
        ],
        &[0xFF],
    ));

    // Another partner on this partner's route: dropped, no reply.
    let other = phonelink_engine::types::DcgClient::new("phone-33333333", phonelink_engine::types::Environment::Prod);
    w.pair.pc.on_message(
        other,
        None,
        TraceContext::new_root(Vec::new()),
        context_message(own_route, "other", error_info(None)),
    );
    settle().await;

    let ok = reply_to("ok").expect("a reply");
    assert_eq!(String::from_utf8(ok.message.payload.clone()).unwrap(), r#"{"processStatus":"Success"}"#);
    assert_eq!(ok.message.headers.get("ms-content-type"), Some("text/json"));
    assert_eq!(ok.priority, 0);
    for id in ["no-trace", "garbage"] {
        let error = reply_to(id).expect("a reply");
        assert_eq!(String::from_utf8(error.message.payload.clone()).unwrap(), r#"{"processStatus":"Error"}"#, "{id}");
    }
    assert!(reply_to("other").is_none());

    // The partner leaves: its context source and the route go with it.
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert!(!w.pair.pc.is_route_registered(own_route));
    let response = phone_request(&w.pair, context_message(own_route, "late", error_info(None))).await;
    assert_eq!(response.status, PlatformRequestStatus::RemoteNotSubscribedToRoute);
}

#[tokio::test(start_paused = true)]
async fn a_context_handle_exchanges_nano_connection_information() {
    use pb::context_source_message::Type;
    use phonelink_engine::nano::connect::{Attempt, ContextExchange, Exchange, initialize_context};
    use phonelink_engine::nano::connection::ConnectionType;
    use phonelink_engine::nano::contract::IceServers;

    let w = world();
    w.connect_phone().await;
    settle().await;

    let handle = w.routes.validator.context_handle(&phone_id()).await.expect("a context source");

    let ok: Reply = Box::new(|_| Some(kvp_reply(r#"{"processStatus":"Success"}"#)));
    let phone_init = Scripted::install(&w.pair.phone, "/Context/Message", ok);
    let ok: Reply = Box::new(|_| Some(kvp_reply(r#"{"processStatus":"Success"}"#)));
    let phone_own = Scripted::install(&w.pair.phone, "/Context/Message/pc-11111111", ok);

    let own_route = "/Context/Message/phone-22222222";
    let trace =
        || Some(pb::ProtoTraceContext { trace_id: "0af7651916cd43dd8448eb211c80319c".into(), ..Default::default() });

    // Initialize: the PC's message on the bare route, the phone's acknowledgement on the PC's.
    let init = initialize_context(&handle, 0, Duration::from_secs(30));
    let phone = async {
        let m = phone_init.next().await;
        let message = pb::ContextSourceMessage::decode(m.message.payload.as_slice()).unwrap();
        let Some(Type::CsInitializeMessage(i)) = message.r#type else { panic!("{message:?}") };
        assert_eq!((i.context_source_id.as_str(), i.transport_preference), ("phone-22222222", 0));
        assert_eq!(m.message.headers.get("_route"), Some("/Context/Message"));

        let ack = pb::ContextSourceMessage {
            r#type: Some(Type::CsInitializeAckMessage(pb::CsInitializeAckMessage {
                status: 1,
                trace_context: trace(),
            })),
        };
        w.pair.inject(context_message(own_route, "ack", ack));
    };

    let (r, ()) = tokio::join!(init, phone);
    r.unwrap();

    // One attempt's connection information, answered with its connection id.
    let attempt = Attempt::new(1, ConnectionType::IceSrtp, IceServers::default());
    let exchange = ContextExchange { handle: handle.clone() };
    let mut offer = serde_json::Map::new();
    offer.insert("security".into(), serde_json::json!({"key": "00"}));

    let pc = exchange.exchange(&attempt, offer);
    let phone = async {
        let m = phone_own.next().await;
        let message = pb::ContextSourceMessage::decode(m.message.payload.as_slice()).unwrap();
        let Some(Type::CsConnectionInfo(info)) = message.r#type else { panic!("{message:?}") };

        let wrapper: serde_json::Value = serde_json::from_str(&info.connection_info).unwrap();
        assert_eq!(wrapper["connectiontype"], 1);
        assert_eq!(wrapper["multipleconnections"], true);
        assert_eq!(wrapper["connectioninformation"]["security"]["key"], "00");

        let other =
            serde_json::json!({"connectioninformation": {}, "connectionid": "someone-else", "connectiontype": 1});
        let mine = serde_json::json!({"connectioninformation": {"keepalive": {"warningtimeout": "5000"}},
            "connectionid": wrapper["connectionid"], "connectiontype": 1, "multipleconnections": true});

        // Not JSON and without a connection id: dropped; the attempt keeps waiting.
        let not_json = "not json".to_string();
        let no_id = serde_json::json!({"connectioninformation": {}}).to_string();
        for (id, body) in [("x", not_json), ("n", no_id), ("o", other.to_string()), ("m", mine.to_string())] {
            let reply = pb::ContextSourceMessage {
                r#type: Some(Type::CsConnectionInfo(pb::CsConnectionInfo {
                    connection_info: body,
                    trace_context: trace(),
                })),
            };
            w.pair.inject(context_message(own_route, id, reply));
        }
    };

    let (answer, ()) = tokio::join!(pc, phone);
    assert_eq!(answer.unwrap()["keepalive"]["warningtimeout"], "5000");

    // A matching connection id whose connectioninformation cannot be read fails that attempt.
    let attempt = Attempt::new(0, ConnectionType::IpSrtp, IceServers::default());
    let pc = exchange.exchange(&attempt, serde_json::Map::new());

    let phone = async {
        let m = phone_own.next().await;
        let message = pb::ContextSourceMessage::decode(m.message.payload.as_slice()).unwrap();
        let Some(Type::CsConnectionInfo(info)) = message.r#type else { panic!("{message:?}") };
        let wrapper: serde_json::Value = serde_json::from_str(&info.connection_info).unwrap();

        let broken = serde_json::json!({"connectionid": wrapper["connectionid"], "connectioninformation": "x"});
        let reply = pb::ContextSourceMessage {
            r#type: Some(Type::CsConnectionInfo(pb::CsConnectionInfo {
                connection_info: broken.to_string(),
                trace_context: trace(),
            })),
        };
        w.pair.inject(context_message(own_route, "b", reply));
    };

    let (failed, ()) = tokio::join!(pc, phone);
    assert!(matches!(failed, Err(phonelink_engine::nano::connect::AttemptError::FailedToReceive(_))));

    // A phone-initiated initialize is refused.
    let initialize = pb::ContextSourceMessage {
        r#type: Some(Type::CsInitializeMessage(pb::CsInitializeMessage {
            context_source_id: "pc-11111111".into(),
            trace_context: trace(),
            transport_preference: 0,
        })),
    };
    w.pair.inject(context_message(own_route, "init", initialize));

    let refusal = phone_own.next().await;
    let message = pb::ContextSourceMessage::decode(refusal.message.payload.as_slice()).unwrap();
    assert!(matches!(message.r#type, Some(Type::CsInitializeAckMessage(pb::CsInitializeAckMessage { status: 2, .. }))));

    // The partner leaves: the handle is dropped and exchanges fail at once.
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert!(handle.is_dropped());

    let attempt = Attempt::new(0, ConnectionType::IpSrtp, IceServers::default());
    let dropped = exchange.exchange(&attempt, serde_json::Map::new()).await;
    assert_eq!(dropped.unwrap_err().to_string(), "the context handle was dropped");
}

// ------------------------------------------------------------------------------------------------
// Inbound /PlatformDisconnectRequest
// ------------------------------------------------------------------------------------------------

fn disconnect_request(reason: i32) -> phonelink_engine::types::TransportMessage {
    let body = pb::PlatformDisconnectRequest { trace_context: None, reason }.encode_to_vec();
    platform(&[("_route", route::PLATFORM_DISCONNECT_REQUEST), ("ms-content-type", "application/x-binary")], &body)
}

#[tokio::test(start_paused = true)]
async fn inbound_disconnect_ends_the_session_without_reply() {
    let w = world();
    let mut events = w.sessions.subscribe();
    w.connect_phone().await;
    assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));
    let before = w.pair.pc_link.sent().len();

    let mut request = request(pc_id(), disconnect_request(pb::PlatformDisconnectRequestReason::UserRequested as i32));
    request.options.one_way = true;
    assert_eq!(w.pair.phone.request(request).await.status, PlatformRequestStatus::Success);
    settle().await;

    assert_eq!(w.pair.pc_link.sent().len(), before, "no response is sent to a disconnect request");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 1 });
    assert!(!w.sessions.is_connected(&phone_id()));
    assert!(w.sessions.is_intentional_disconnect_capable(&phone_id()));
    assert_eq!(
        events.recv().await.unwrap(),
        SessionEvent::Disconnected { partner: phone_id(), failure: ConnectionFailure::SessionEnded { reason: 1 } }
    );
    settle().await;
    assert_eq!(w.routes.validator.cached(&phone_id()), None, "the context source is forgotten with the session");
}

#[tokio::test(start_paused = true)]
async fn an_unparsable_disconnect_reason_is_an_unknown_session_end() {
    let w = world();
    w.connect_phone().await;

    // The PC's own `{}` text/json form fails the protobuf parser.
    w.pair.inject(platform(
        &[
            ("_route", route::PLATFORM_DISCONNECT_REQUEST),
            ("_requestId", "5"),
            ("_rejectionVersion", "1"),
            ("ms-content-type", "text/json"),
        ],
        b"{}",
    ));

    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 0 });
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::SessionEnded { reason: 0 }));
    assert!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).is_empty());
}

#[tokio::test(start_paused = true)]
async fn keep_session_on_peer_disconnect_suppresses_the_session_end() {
    let mut config = EngineConfig::default();
    config.relay.keep_session_on_peer_disconnect = true;
    let w = World::new(config);
    w.connect_phone().await;

    w.pair.inject(disconnect_request(4));
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert!(w.sessions.is_connected(&phone_id()));
    assert!(w.sessions.is_intentional_disconnect_capable(&phone_id()), "still marked capable");
}

// ------------------------------------------------------------------------------------------------
// Key rotation (inbound)
// ------------------------------------------------------------------------------------------------

fn key_rotation(route: &str, pairs: &[(&str, &str)], body: &str) -> phonelink_engine::types::TransportMessage {
    let mut h = vec![("_route", route)];
    h.extend_from_slice(pairs);
    platform(&h, body.as_bytes())
}

#[tokio::test(start_paused = true)]
async fn key_rotation_check_replies() {
    let w = world();
    let check = || key_rotation(route::CRYPTO_TRUST_KEY_ROTATION_CHECK, &[("ms-content-type", "text/json")], "{}");

    let response = phone_request(&w.pair, check()).await;
    assert_eq!(response.status, PlatformRequestStatus::Success);
    assert_eq!(
        kvp_body(&response),
        serde_json::json!({"CryptoKeyRotationResult": "1", "ResultDetails": "crypto trust relationship not found"})
    );

    let reply = w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).pop().unwrap();
    assert_eq!(reply.message.message_type, MessageType::Platform);
    assert_eq!(reply.message.headers.get("ms-content-type"), Some("text/json"));
    assert_eq!(reply.priority, 0);
    assert_eq!(
        String::from_utf8(reply.message.payload.clone()).unwrap(),
        r#"{"CryptoKeyRotationResult":"1","ResultDetails":"crypto trust relationship not found"}"#
    );

    use phonelink_engine::edge::Clock;
    let now = w.clock.now();
    *w.trust.expiration.lock().unwrap() = Some(now + Duration::from_millis(1_000_500));

    let response = phone_request(&w.pair, check()).await;
    assert_eq!(
        String::from_utf8(response.message.unwrap().payload).unwrap(),
        r#"{"TimeToExpireInSeconds":"1001","CryptoKeyRotationResult":"0"}"#,
        "the result follows the extra keys"
    );
}

#[tokio::test(start_paused = true)]
async fn key_rotation_replies_for_every_result() {
    use phonelink_engine::auth::crypto;
    use phonelink_engine::edge::Clock;

    let w = world();
    let now = w.clock.now();
    let key = crypto::generate_signing_key();
    let der = crypto::self_signed_certificate(&key, "trust_phone-22222222", now).unwrap();
    let b64 = base64::prelude::BASE64_STANDARD.encode(&der);
    let with_cert = format!("{{\"Certificate\":\"{b64}\"}}");

    let rotation =
        |body: &str, ct: &str| key_rotation(route::CRYPTO_TRUST_KEY_ROTATION, &[("ms-content-type", ct)], body);
    let not_found =
        serde_json::json!({"CryptoKeyRotationResult": "1", "ResultDetails": "crypto trust relationship not found"});

    // No relationship → "1", with its details.
    let response = phone_request(&w.pair, rotation(&with_cert, "text/json")).await;
    assert_eq!(kvp_body(&response), not_found);
    assert!(w.trust.rotated.lock().unwrap().is_empty());

    *w.trust.expiration.lock().unwrap() = Some(now + Duration::from_secs(86_400));
    // Stored → "0".
    let response = phone_request(&w.pair, rotation(&with_cert, "text/json")).await;
    assert_eq!(kvp_body(&response), serde_json::json!({"CryptoKeyRotationResult": "0"}));
    assert_eq!(w.trust.rotated.lock().unwrap().clone(), vec![(phone_id(), der.clone())]);

    // Values that are not strings do not spoil the map.
    let extra = format!("{{\"Certificate\":\"{b64}\",\"x\":true,\"n\":null}}");
    let response = phone_request(&w.pair, rotation(&extra, "text/json")).await;
    assert_eq!(kvp_body(&response), serde_json::json!({"CryptoKeyRotationResult": "0"}));

    // Missing, undecodable, empty or not-a-certificate Certificate, a non-KVP content type → "2".
    let invalid =
        serde_json::json!({"CryptoKeyRotationResult": "2", "ResultDetails": "Can't get valid partner certificate."});
    let not_a_certificate =
        format!("{{\"Certificate\":\"{}\"}}", base64::prelude::BASE64_STANDARD.encode([0x30, 0x82, 0x01, 0x0A]));

    for (body, ct) in [
        ("{}", "text/json"),
        ("{\"Certificate\":\"%%%\"}", "text/json"),
        ("{\"Certificate\":\"\"}", "text/json"),
        (not_a_certificate.as_str(), "text/json"),
        (with_cert.as_str(), "application/x-binary"),
    ] {
        let response = phone_request(&w.pair, rotation(body, ct)).await;
        assert_eq!(kvp_body(&response), invalid, "{body} / {ct}");
    }

    // A certificate that has already expired → "3" with the trust-manager error type's name.
    let expired =
        crypto::self_signed_certificate(&key, "trust_phone-22222222", now - Duration::from_secs(400 * 86_400)).unwrap();
    let expired = format!("{{\"Certificate\":\"{}\"}}", base64::prelude::BASE64_STANDARD.encode(&expired));
    let response = phone_request(&w.pair, rotation(&expired, "text/json")).await;
    assert_eq!(
        kvp_body(&response),
        serde_json::json!({"CryptoKeyRotationResult": "3", "ResultDetails": "PartnerCertExpirationTimeStampInvalid"})
    );

    // Trust-store outcomes.
    *w.trust.rotate_result.lock().unwrap() = Err(TrustRotationError::NotFound);
    let response = phone_request(&w.pair, rotation(&with_cert, "text/json")).await;
    assert_eq!(kvp_body(&response), not_found);

    *w.trust.rotate_result.lock().unwrap() = Err(TrustRotationError::InvalidCertificate);
    let response = phone_request(&w.pair, rotation(&with_cert, "text/json")).await;
    assert_eq!(kvp_body(&response), invalid);
}

#[tokio::test(start_paused = true)]
async fn key_rotation_failure_sends_no_reply() {
    use phonelink_engine::auth::crypto;
    use phonelink_engine::edge::Clock;

    let w = world();
    let now = w.clock.now();
    *w.trust.expiration.lock().unwrap() = Some(now + Duration::from_secs(86_400));
    let der = crypto::self_signed_certificate(&crypto::generate_signing_key(), "trust_phone-22222222", now).unwrap();
    let with_cert = format!("{{\"Certificate\":\"{}\"}}", base64::prelude::BASE64_STANDARD.encode(&der));

    let rotation = |body: &str, id: &str| {
        key_rotation(
            route::CRYPTO_TRUST_KEY_ROTATION,
            &[("_requestId", id), ("_rejectionVersion", "1"), ("ms-content-type", "text/json")],
            body,
        )
    };

    // A body the KVP reader rejects: not JSON, not an object, a repeated key.
    w.pair.inject(rotation("not json", "a"));
    w.pair.inject(rotation("[1]", "b"));
    w.pair.inject(rotation(r#"{"Certificate":"AA==","Certificate":"AA=="}"#, "c"));

    // The trust store fails to persist.
    *w.trust.rotate_result.lock().unwrap() = Err(TrustRotationError::Store("KeychainError".into()));
    w.pair.inject(rotation(&with_cert, "d"));

    settle().await;
    assert!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).is_empty(), "no reply to any of them");
    assert_eq!(w.trust.rotated.lock().unwrap().len(), 1, "only the last reached the trust store");
}

#[tokio::test(start_paused = true)]
async fn key_rotation_without_request_id_gets_no_reply() {
    let w = world();
    w.pair.inject(key_rotation(
        route::CRYPTO_TRUST_KEY_ROTATION_CHECK,
        &[("ms-content-type", "text/json"), ("_rejectionVersion", "1")],
        "{}",
    ));
    settle().await;
    assert!(w.pair.pc_link.sent().is_empty());
}

// ------------------------------------------------------------------------------------------------
// /Context/Publish
// ------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Observed(Mutex<Vec<Publication>>);

impl Observed {
    fn ids(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().map(|p| p.message.message_id.clone()).collect()
    }
}

impl PublicationObserver for Observed {
    fn on_publication(&self, publication: &Publication) {
        self.0.lock().unwrap().push(publication.clone());
    }
}

fn publication(tag: i32, publisher: &str, id: &str, additional: &[u8]) -> phonelink_engine::types::TransportMessage {
    let payload = pb::PubSubPayload { data: Vec::new(), additional: additional.to_vec() }.encode_to_vec();

    let body = pb::MsaepMessage {
        message_tag: tag,
        dcg_client_id: publisher.into(),
        message_id: id.into(),
        payload,
        platform_major_version: 1,
        platform_minor_version: 1,
    }
    .encode_to_vec();

    platform(&[("_route", route::CONTEXT_PUBLISH), ("ms-content-type", "application/x-binary")], &body)
}

async fn phone_publishes(w: &World, message: phonelink_engine::types::TransportMessage) {
    let mut r = request(pc_id(), message);
    r.options.one_way = true;
    assert_eq!(w.pair.phone.request(r).await.status, PlatformRequestStatus::Success);
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn inbound_publication_is_acknowledged_and_delivered_by_tag() {
    let w = world();
    w.connect_phone().await;
    let clipboard = Arc::new(Observed::default());
    let status = Arc::new(Observed::default());
    let _a = w.routes.msaep.subscribe(9, DeviceFilter::Any, clipboard.clone());
    let _b = w.routes.msaep.subscribe(8, DeviceFilter::Any, status.clone());
    let acks_before = w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len();

    phone_publishes(&w, publication(9, "phone-22222222", "m-1", &[1, 2])).await;

    let acks = w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE);
    assert_eq!(acks.len(), acks_before + 1, "an immediate {{}} acknowledgement");
    let ack = acks.last().unwrap();
    assert_eq!(ack.message.payload, b"{}");
    assert_eq!(ack.message.headers.get("ms-content-type"), Some("text/json"));
    assert_eq!(ack.message.message_type, MessageType::Platform);

    assert_eq!(clipboard.ids(), vec!["m-1"]);
    assert!(status.ids().is_empty(), "tag 8 does not match 9");
    let delivered = clipboard.0.lock().unwrap()[0].clone();
    assert_eq!(delivered.source, phone_id());
    assert_eq!(delivered.payload.additional, vec![1, 2]);
    assert_eq!(delivered.context.partner, phone_id());
}

#[tokio::test(start_paused = true)]
async fn publications_are_deduplicated_per_observer_by_the_last_id() {
    let w = world();
    w.connect_phone().await;
    let one = Arc::new(Observed::default());
    let two = Arc::new(Observed::default());
    let _a = w.routes.msaep.subscribe(9, DeviceFilter::Any, one.clone());

    for id in ["m-1", "m-1", "m-2", "m-1"] {
        phone_publishes(&w, publication(9, "phone-22222222", id, b"")).await;
    }
    assert_eq!(one.ids(), vec!["m-1", "m-2", "m-1"], "one id remembered, not a set");

    // A later subscriber has its own memory.
    let _b = w.routes.msaep.subscribe(9, DeviceFilter::Any, two.clone());
    phone_publishes(&w, publication(9, "phone-22222222", "m-1", b"")).await;
    assert_eq!(two.ids(), vec!["m-1"]);
    assert_eq!(one.ids().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn device_filters_and_unsubscribe() {
    let w = world();
    w.connect_phone().await;

    let any = Arc::new(Observed::default());
    let only_phone = Arc::new(Observed::default());
    let only_other = Arc::new(Observed::default());
    let rejecting = Arc::new(Observed::default());
    let sub_any = w.routes.msaep.subscribe(9, DeviceFilter::Any, any.clone());
    let _b = w.routes.msaep.subscribe(9, DeviceFilter::Only(vec!["phone-22222222".into()]), only_phone.clone());
    let _c = w.routes.msaep.subscribe(9, DeviceFilter::Only(vec!["someone-else".into()]), only_other.clone());
    let _d = w.routes.msaep.subscribe(9, DeviceFilter::RejectAll, rejecting.clone());

    phone_publishes(&w, publication(9, "phone-22222222", "m-1", b"")).await;
    assert_eq!(any.ids().len(), 1);
    assert_eq!(only_phone.ids().len(), 1);
    assert!(only_other.ids().is_empty());
    assert!(rejecting.ids().is_empty());

    drop(sub_any);
    phone_publishes(&w, publication(9, "phone-22222222", "m-2", b"")).await;
    assert_eq!(any.ids().len(), 1, "unsubscribed on drop");
    assert_eq!(only_phone.ids().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn an_unparsable_publication_gets_no_reply() {
    let w = world();
    w.connect_phone().await;

    let seen = Arc::new(Observed::default());
    let _s = w.routes.msaep.subscribe(0, DeviceFilter::Any, seen.clone());
    let before = w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len();

    w.pair.inject(platform(
        &[
            ("_route", route::CONTEXT_PUBLISH),
            ("_requestId", "p"),
            ("_rejectionVersion", "1"),
            ("ms-content-type", "application/x-binary"),
        ],
        &[0xFF, 0xFF, 0xFF],
    ));
    settle().await;
    assert_eq!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len(), before);
    assert!(seen.ids().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_publication_from_a_partner_that_is_not_connected_is_delivered_without_validation() {
    let w = world(); // the phone is not connected: validation answers "not connected"

    let seen = Arc::new(Observed::default());
    let _s = w.routes.msaep.subscribe(9, DeviceFilter::Any, seen.clone());

    w.pair.inject(platform(&[("_requestId", "p")], b"")); // noise: rejected without _rejectionVersion → nothing
    let mut message = publication(9, "phone-22222222", "m-1", b"");
    message.headers.insert("_requestId", "p-1");
    w.pair.inject(message);
    settle().await;

    assert_eq!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).len(), 1, "acknowledged");
    assert!(w.pair.pc_link.sent_on(route::SESSION_VALIDATION).is_empty(), "not connected: nothing sent");
    assert_eq!(seen.ids(), vec!["m-1"], "delivered with a context source created without validation");
    assert_eq!(seen.0.lock().unwrap()[0].context.capabilities, None);
}

#[tokio::test(start_paused = true)]
async fn publish_sends_one_way_to_every_connected_partner() {
    let w = world();
    let empty = w.routes.msaep.publish(9, pb::PubSubPayload::default()).await;
    assert!(empty.targets.is_empty());
    assert_eq!(empty.outcome().await, PublishOutcome::NoResults);

    w.connect_phone().await;
    let handle = w.routes.msaep.publish(9, pb::PubSubPayload { data: vec![7, 7], additional: Vec::new() }).await;
    assert_eq!(handle.targets, vec![phone_id()]);
    let message_id = handle.message_id.clone();
    assert_eq!(handle.outcome().await, PublishOutcome::Success);

    let received = w.phone_publish.next().await;
    let body = pb::MsaepMessage::decode(received.message.payload.as_slice()).unwrap();
    assert_eq!(body.message_tag, 9);
    assert_eq!(body.dcg_client_id, "pc-11111111");
    assert_eq!(body.message_id, message_id);
    assert!(uuid::Uuid::parse_str(&body.message_id).is_ok());
    assert_eq!(body.message_id, body.message_id.to_lowercase());
    assert_eq!((body.platform_major_version, body.platform_minor_version), (1, 1));
    assert_eq!(pb::PubSubPayload::decode(body.payload.as_slice()).unwrap().data, vec![7, 7]);

    let sent = w.pair.pc_link.sent_on(route::CONTEXT_PUBLISH).pop().unwrap();
    assert_eq!(sent.options, SendOptions { one_way: true, skip_wake_if_not_connected: true, ..SendOptions::NONE });
    assert_eq!(sent.priority, 100);
    assert_eq!(sent.message.headers.get("ms-content-type"), Some("application/x-binary"));

    // A failed send makes the outcome a failure.
    w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));
    let handle = w.routes.msaep.publish(9, pb::PubSubPayload::default()).await;
    assert_eq!(handle.outcome().await, PublishOutcome::Failure);
}

/// The phone answers `/SessionValidation` without `SESSION_VALIDATION`: "validation not supported",
/// nothing is cached.
fn validation_not_supported(w: &World) {
    let without =
        pb::ProtoPlatformSessionValidationResponse { platform_capabilities: vec![2, 3], ..Default::default() }
            .encode_to_vec();
    w.phone_validation
        .set_reply(Box::new(move |_| Some(platform(&[("ms-content-type", "application/x-binary")], &without))));
}

fn validated_baseline() -> Option<AcceptedCapabilities> {
    Some(AcceptedCapabilities {
        capabilities: vec![pb::ProtoPlatformCapabilities::SessionValidation],
        nano_transport_preference_version: None,
    })
}

#[tokio::test(start_paused = true)]
async fn an_inbound_publication_validates_a_partner_with_nothing_cached() {
    let w = world();
    validation_not_supported(&w);
    w.connect_phone().await;
    settle().await;
    assert_eq!(w.phone_validation.count(), 1, "the on-connect validation");
    assert_eq!(w.routes.validator.cached(&phone_id()), None, "nothing cached");

    // Session validation runs on demand when an inbound publish arrives.
    w.phone_validation.set_reply(Box::new(|_| Some(full_validation_response())));
    let seen = Arc::new(Observed::default());
    let _s = w.routes.msaep.subscribe(9, DeviceFilter::Any, seen.clone());
    phone_publishes(&w, publication(9, "phone-22222222", "m-1", b"")).await;

    assert_eq!(w.phone_validation.count(), 2, "the publication validated the partner");
    assert_eq!(w.routes.validator.cached(&phone_id()).unwrap().capabilities, validated_baseline());
    assert_eq!(seen.0.lock().unwrap()[0].context.capabilities, validated_baseline());

    // Cached now: the next publication is not validated again.
    phone_publishes(&w, publication(9, "phone-22222222", "m-2", b"")).await;
    assert_eq!(w.phone_validation.count(), 2);
    assert_eq!(seen.ids(), vec!["m-1", "m-2"]);
}

#[tokio::test(start_paused = true)]
async fn an_outbound_publish_validates_a_partner_with_nothing_cached() {
    let w = world();
    validation_not_supported(&w);
    w.connect_phone().await;
    settle().await;
    assert_eq!(w.phone_validation.count(), 1);
    assert_eq!(w.routes.validator.cached(&phone_id()), None);

    // Session validation runs on demand before a publish.
    w.phone_validation.set_reply(Box::new(|_| Some(full_validation_response())));
    let handle = w.routes.msaep.publish(9, pb::PubSubPayload { data: vec![1], additional: Vec::new() }).await;

    assert_eq!(w.phone_validation.count(), 2, "the publish validated the partner first");
    assert_eq!(handle.targets, vec![phone_id()]);
    assert_eq!(handle.outcome().await, PublishOutcome::Success);
    assert_eq!(w.routes.validator.cached(&phone_id()).unwrap().capabilities, validated_baseline());

    // Validation comes before the publication on the wire.
    let routes: Vec<String> = w
        .pair
        .pc_link
        .sent()
        .iter()
        .filter_map(|m| m.message.route().map(str::to_owned))
        .filter(|r| r == route::SESSION_VALIDATION || r == route::CONTEXT_PUBLISH)
        .collect();
    assert_eq!(routes, vec![route::SESSION_VALIDATION, route::SESSION_VALIDATION, route::CONTEXT_PUBLISH]);

    // Cached now: the next publish does not validate again.
    let handle = w.routes.msaep.publish(9, pb::PubSubPayload::default()).await;
    assert_eq!(handle.outcome().await, PublishOutcome::Success);
    assert_eq!(w.phone_validation.count(), 2);
    assert_eq!(w.phone_publish.count(), 2);
}

// ------------------------------------------------------------------------------------------------
// /DeviceResourceManager: receivers
// ------------------------------------------------------------------------------------------------

struct Receiver {
    seen: Mutex<Vec<DrmRequest>>,
    answer: Mutex<Result<Option<Vec<u8>>, DrmReceiverError>>,
    panics: bool,
}

impl Receiver {
    fn answering(answer: &[u8]) -> Arc<Self> {
        Arc::new(Self { seen: Mutex::default(), answer: Mutex::new(Ok(Some(answer.to_vec()))), panics: false })
    }

    fn seen(&self) -> Vec<DrmRequest> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl DrmReceiver for Receiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        self.seen.lock().unwrap().push(request);
        if self.panics {
            panic!("receiver bug");
        }
        self.answer.lock().unwrap().clone()
    }
}

/// A receiver whose answers carry a follow-up recording how each answer's send ended.
struct FollowingReceiver {
    answer: Vec<u8>,
    deliveries: Arc<Mutex<Vec<AnswerDelivery>>>,
}

impl FollowingReceiver {
    fn answering(answer: &[u8]) -> Arc<Self> {
        Arc::new(Self { answer: answer.to_vec(), deliveries: Arc::default() })
    }

    fn deliveries(&self) -> Vec<AnswerDelivery> {
        self.deliveries.lock().unwrap().clone()
    }
}

#[async_trait]
impl DrmFollowUpReceiver for FollowingReceiver {
    async fn receive(&self, _request: DrmRequest) -> Result<Option<DrmAnswer>, DrmReceiverError> {
        let deliveries = self.deliveries.clone();
        Ok(Some(DrmAnswer::then(self.answer.clone(), move |delivery| async move {
            deliveries.lock().unwrap().push(delivery);
        })))
    }
}

fn drm(resource_type: i32, request_type: i32, path: &str, payload: &[u8]) -> phonelink_engine::types::TransportMessage {
    let body = pb::DeviceResourceManagerMessage {
        resource_type,
        request_type,
        payload: payload.to_vec(),
        resource_path: path.into(),
    }
    .encode_to_vec();

    platform(&[("_route", route::DEVICE_RESOURCE_MANAGER), ("ms-content-type", "application/x-binary")], &body)
}

async fn phone_drm_request(
    w: &World,
    message: phonelink_engine::types::TransportMessage,
) -> phonelink_engine::seams::PlatformResponse {
    let mut r = request(pc_id(), message);
    r.timeout = Duration::from_secs(5);
    w.pair.phone.request(r).await
}

#[tokio::test(start_paused = true)]
async fn drm_route_is_subscribed_with_the_first_receiver() {
    let w = world();
    let response = phone_drm_request(&w, drm(4, 1, "/clipboard", b"x")).await;
    assert_eq!(response.status, PlatformRequestStatus::RemoteNotSubscribedToRoute, "no receiver yet");

    w.routes.drm.register_receiver("/clipboard", Receiver::answering(b"a")).unwrap();
    assert_eq!(phone_drm_request(&w, drm(4, 1, "/clipboard", b"x")).await.status, PlatformRequestStatus::Success);

    assert_eq!(
        w.routes.drm.register_receiver("/clipboard", Receiver::answering(b"b")),
        Err(DrmRegistrationError::AlreadyRegistered("/clipboard".into()))
    );
    assert_eq!(
        w.routes.drm.register_receiver("/x?y=1", Receiver::answering(b"b")),
        Err(DrmRegistrationError::InvalidPath("/x?y=1".into()))
    );
    assert_eq!(
        w.routes.drm.register_receiver("x", Receiver::answering(b"b")),
        Err(DrmRegistrationError::InvalidPath("x".into()))
    );
}

#[tokio::test(start_paused = true)]
async fn drm_request_reaches_its_receiver_with_the_full_path_and_is_answered() {
    let w = world();
    let receiver = Receiver::answering(b"answer");
    w.routes.drm.register_receiver("/clipboard", receiver.clone()).unwrap();

    let response = phone_drm_request(&w, drm(4, 1, "/clipboard?since=5", &[9, 9])).await;
    assert_eq!(response.status, PlatformRequestStatus::Success);

    let seen = receiver.seen().pop().unwrap();
    assert_eq!(seen.path, "/clipboard?since=5", "the receiver gets the full path");
    assert_eq!(seen.resource_type, pb::ProtoDeviceResourceType::Unknown);
    assert_eq!(seen.request_type, pb::ProtoDeviceResourceRequestType::Get);
    assert_eq!(seen.payload, vec![9, 9]);
    assert_eq!(seen.source, phone_id());
    assert_eq!(seen.channel, DrmChannel::Platform);

    // The phone router minted the id from the process-wide counter.
    let minted = w.pair.phone_link.sent_on(route::DEVICE_RESOURCE_MANAGER)[0]
        .message
        .headers
        .get("_requestId")
        .unwrap()
        .to_owned();
    assert_eq!(seen.request_id.as_deref(), Some(minted.as_str()));

    let message = response.message.unwrap();
    assert_eq!(
        pairs(&message.headers),
        owned(&[
            ("_originalRequestId", minted.as_str()),
            ("_route", "/internal/response"),
            ("ms-content-type", "application/x-binary"),
            ("_rejectionVersion", "1")
        ])
    );

    let body = pb::DeviceResourceManagerResponseMessage::decode(message.payload.as_slice()).unwrap();
    assert_eq!(body.payload, b"answer");
    assert_eq!(body.response_type, Some(pb::DeviceResourceManagerResponseMessageType::Success as i32));
    assert_eq!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE)[0].priority, 100);
}

#[tokio::test(start_paused = true)]
async fn drm_path_derived_from_the_type() {
    let w = world();
    let receiver = Receiver::answering(b"p");
    w.routes.drm.register_receiver("/permissions", receiver.clone()).unwrap();
    let unknown = Receiver::answering(b"u");
    w.routes.drm.register_receiver("/unknown", unknown.clone()).unwrap();

    assert_eq!(phone_drm_request(&w, drm(3, 4, "", b"")).await.status, PlatformRequestStatus::Success);
    let seen = receiver.seen().pop().unwrap();
    assert_eq!(seen.path, "/permissions");
    assert_eq!(seen.resource_type, pb::ProtoDeviceResourceType::Permissions);
    assert_eq!(seen.request_type, pb::ProtoDeviceResourceRequestType::Sync);

    // UNSPECIFIED resource type maps to unknown.
    assert_eq!(phone_drm_request(&w, drm(0, 1, "", b"")).await.status, PlatformRequestStatus::Success);
    assert_eq!(unknown.seen().pop().unwrap().resource_type, pb::ProtoDeviceResourceType::Unknown);
}

#[tokio::test(start_paused = true)]
async fn drm_requests_that_get_no_reply() {
    let w = world();
    let receiver = Receiver::answering(b"a");
    w.routes.drm.register_receiver("/clipboard", receiver.clone()).unwrap();

    // Unknown path: logged, no response (the phone times out).
    assert_eq!(
        phone_drm_request(&w, drm(4, 1, "/nothing-here", b"")).await.status,
        PlatformRequestStatus::ResponseTimedOut
    );

    // Faulting request type and resource types: never dispatched.
    for (resource_type, request_type) in [(4, 0), (7, 2), (8, 1), (99, 1), (4, 9)] {
        assert_eq!(
            phone_drm_request(&w, drm(resource_type, request_type, "/clipboard", b"")).await.status,
            PlatformRequestStatus::ResponseTimedOut,
            "{resource_type}/{request_type}"
        );
    }

    // Empty and undecodable bodies.
    for body in [&b""[..], &[0xFF, 0xFF][..]] {
        let message =
            platform(&[("_route", route::DEVICE_RESOURCE_MANAGER), ("ms-content-type", "application/x-binary")], body);
        assert_eq!(phone_drm_request(&w, message).await.status, PlatformRequestStatus::ResponseTimedOut);
    }
    assert!(receiver.seen().is_empty());
    assert!(w.pair.pc_link.sent_on(route::INTERNAL_RESPONSE).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_receivers_follow_up_runs_once_its_answer_was_sent_or_could_not_be() {
    use AnswerDelivery::{NotSent, Sent};
    let w = world();
    let receiver = FollowingReceiver::answering(b"a");
    w.routes.drm.register_follow_up_receiver("/remotecontrol", receiver.clone()).unwrap();

    // Sent: the phone has the answer.
    let response = phone_drm_request(&w, drm(4, 2, "/remotecontrol", b"")).await;
    assert_eq!(response.status, PlatformRequestStatus::Success);
    settle().await;
    assert_eq!(receiver.deliveries(), [Sent]);

    // The answer's send failed.
    w.pair.pc_link.script(Behavior::Fail(DcgSendStatus::TimedOut));
    let response = phone_drm_request(&w, drm(4, 2, "/remotecontrol", b"")).await;
    assert_eq!(response.status, PlatformRequestStatus::ResponseTimedOut);
    settle().await;
    assert_eq!(receiver.deliveries(), [Sent, NotSent]);

    // No _requestId: no answer can be sent.
    w.pair.inject(drm(4, 2, "/remotecontrol", b""));
    settle().await;
    assert_eq!(receiver.deliveries(), [Sent, NotSent, NotSent]);

    // The device-proxy variant: a reply that is sent, and one over its size limit.
    w.pair.inject(proxy_message(Some("r-1"), "64", proxy_command(4.into(), 2.into(), b"", "/remotecontrol")));
    settle().await;
    assert_eq!(receiver.deliveries(), [Sent, NotSent, NotSent, Sent]);

    let big = FollowingReceiver::answering(&[0u8; 2048]);
    w.routes.drm.register_follow_up_receiver("/big", big.clone()).unwrap();
    w.pair.inject(proxy_message(Some("r-2"), "1", proxy_command(4.into(), 2.into(), b"", "/big")));
    settle().await;
    assert_eq!(big.deliveries(), [NotSent]);
}

#[tokio::test(start_paused = true)]
async fn a_failing_drm_receiver_is_isolated() {
    let w = world();
    let failing = Arc::new(Receiver {
        seen: Mutex::default(),
        answer: Mutex::new(Err(DrmReceiverError("bad payload".into()))),
        panics: false,
    });
    let panicking = Arc::new(Receiver { seen: Mutex::default(), answer: Mutex::new(Ok(None)), panics: true });
    let silent = Arc::new(Receiver { seen: Mutex::default(), answer: Mutex::new(Ok(None)), panics: false });
    let healthy = Receiver::answering(b"ok");

    w.routes.drm.register_receiver("/failing", failing.clone()).unwrap();
    w.routes.drm.register_receiver("/panicking", panicking.clone()).unwrap();
    w.routes.drm.register_receiver("/silent", silent.clone()).unwrap();
    w.routes.drm.register_receiver("/healthy", healthy).unwrap();

    for path in ["/failing", "/panicking", "/silent"] {
        assert_eq!(
            phone_drm_request(&w, drm(4, 1, path, b"")).await.status,
            PlatformRequestStatus::ResponseTimedOut,
            "{path}"
        );
    }
    assert_eq!((failing.seen().len(), panicking.seen().len(), silent.seen().len()), (1, 1, 1));

    // The session continues.
    assert_eq!(phone_drm_request(&w, drm(4, 1, "/healthy", b"")).await.status, PlatformRequestStatus::Success);
}

#[tokio::test(start_paused = true)]
async fn a_drm_request_without_request_id_is_dispatched_but_not_answered() {
    let w = world();
    let receiver = Receiver::answering(b"a");
    w.routes.drm.register_receiver("/clipboard", receiver.clone()).unwrap();

    let mut message = drm(4, 1, "/clipboard", b"");
    message.headers.insert("_rejectionVersion", "1");
    w.pair.inject(message);
    settle().await;

    assert_eq!(receiver.seen().pop().unwrap().request_id, None);
    assert!(w.pair.pc_link.sent().is_empty());
}

// ------------------------------------------------------------------------------------------------
// /DeviceResourceManager: the request wrapper
// ------------------------------------------------------------------------------------------------

fn drm_send(address: DrmAddress) -> DrmSendRequest {
    DrmSendRequest {
        target: phone_id(),
        address,
        request_type: pb::ProtoDeviceResourceRequestType::Get,
        payload: vec![0x08, 0x01],
        client_version: Some("1.24072.123.0".into()),
        phone_enabled: true,
        force_session_start: false,
        timeout: Duration::from_secs(120),
        trace: TraceContext::new_root(Vec::new()),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_refusals_send_nothing() {
    let w = world();
    w.connect_phone().await;
    let sent_before = w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).len();
    let drm = &w.routes.drm;

    for bad in ["https://example.com/clipboard", "", "//host/x", "/a b"] {
        assert_matches!(
            drm.send(drm_send(DrmAddress::Path(bad.into()))).await,
            DrmSendOutcome::InvalidRequest(_),
            "{bad:?}"
        );
    }

    assert_matches!(
        drm.send(drm_send(DrmAddress::Type(pb::ProtoDeviceResourceType::Unspecified))).await,
        DrmSendOutcome::InvalidRequest(_)
    );
    let mut unspecified = drm_send(DrmAddress::Path("/clipboard".into()));
    unspecified.request_type = pb::ProtoDeviceResourceRequestType::Unspecified;
    assert_matches!(drm.send(unspecified).await, DrmSendOutcome::InvalidRequest(_));

    let mut disabled = drm_send(DrmAddress::Path("/clipboard".into()));
    disabled.phone_enabled = false;
    assert_eq!(drm.send(disabled).await, DrmSendOutcome::UserDisabled);

    for state in [LockState::Locked, LockState::Unknown] {
        w.lock.set(state);
        assert_eq!(
            drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
            DrmSendOutcome::Locked,
            "{state:?}"
        );
    }
    w.lock.set(LockState::Unlocked);

    for version in [
        None,
        Some(""),
        Some("1.23112.189"),
        Some("1.23112.188.9"),
        Some("1.0"),
        Some("1.0.0.1"),
        Some("garbage"),
        Some("1"),
        Some("1.2.3.4.5"),
    ] {
        let mut old = drm_send(DrmAddress::Path("/clipboard".into()));
        old.client_version = version.map(str::to_owned);
        assert_eq!(drm.send(old).await, DrmSendOutcome::LtwMinVersionRequired, "{version:?}");
    }

    let cancelled = drm_send(DrmAddress::Path("/clipboard".into()));
    cancelled.cancel.cancel();
    assert_eq!(drm.send(cancelled).await, DrmSendOutcome::Canceled);

    assert_eq!(w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).len(), sent_before);
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_sends_path_and_type_addressed_requests() {
    let w = world();
    w.connect_phone().await;
    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"pong", Some(1)))));

    for version in ["1.23112.189.0", "1.0.0.0", "1.0.0", "2.0", "1.23112.190-beta"] {
        let mut ok = drm_send(DrmAddress::Path("/clipboard".into()));
        ok.client_version = Some(version.into());
        assert_eq!(w.routes.drm.send(ok).await, DrmSendOutcome::Success { payload: b"pong".to_vec() }, "{version}");
    }

    let sent = w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).pop().unwrap();
    assert_eq!(
        sent.options,
        SendOptions { ignore_device_disabled: true, skip_wake_if_not_connected: true, ..SendOptions::NONE }
    );
    assert_eq!(sent.priority, 100);
    assert_eq!(sent.message.headers.get("ms-content-type"), Some("application/x-binary"));

    let body = pb::DeviceResourceManagerMessage::decode(sent.message.payload.as_slice()).unwrap();
    assert_eq!(body.resource_type, pb::ProtoDeviceResourceType::Unknown as i32, "path-addressed ⇒ UNKNOWN");
    assert_eq!(body.resource_path, "/clipboard");
    assert_eq!(body.request_type, 1);
    assert_eq!(body.payload, vec![0x08, 0x01]);

    let typed = drm_send(DrmAddress::Type(pb::ProtoDeviceResourceType::Permissions));
    assert!(matches!(w.routes.drm.send(typed).await, DrmSendOutcome::Success { .. }));

    let sent = w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).pop().unwrap();
    let body = pb::DeviceResourceManagerMessage::decode(sent.message.payload.as_slice()).unwrap();
    assert_eq!(body.resource_type, pb::ProtoDeviceResourceType::Permissions as i32);
    assert_eq!(body.resource_path, "/permissions");
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_status_mapping_and_the_metered_flag() {
    let w = world();
    w.connect_phone().await;
    let drm = w.routes.drm.clone();
    let send = || drm_send(DrmAddress::Path("/devicestatus".into()));

    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"", Some(2)))));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::UnsupportedResourceType);

    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"x", None))));
    assert_eq!(
        drm.send(send()).await,
        DrmSendOutcome::Success { payload: b"x".to_vec() },
        "absent response_type is success"
    );

    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"x", Some(0)))));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::Success { payload: b"x".to_vec() }, "UNSPECIFIED is success");

    w.phone_drm.set_reply(Box::new(|_| Some(platform(&[("ms-content-type", "application/x-binary")], &[0xFF, 0xFF]))));
    assert!(matches!(drm.send(send()).await, DrmSendOutcome::InvalidResponse(_)));

    let reject = |reason: &'static str| -> Reply {
        Box::new(move |_| Some(platform(&[("_rejectedReason", reason), ("ms-content-type", "text/json")], b"{}")))
    };

    w.phone_drm.set_reply(reject("NotSubscribedToRoute"));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::UnsupportedResourceType);
    w.phone_drm.set_reply(reject("Unspecified"));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::ResponseTimeout);

    w.phone_drm.set_reply(reject("MeteredSyncDisabled"));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::Request(PlatformRequestStatus::RemoteMeteredSyncDisabled));
    assert!(drm.metered_sync_disabled(&phone_id()));

    w.phone_drm.set_reply(reject("AllSyncDisabled"));
    assert_eq!(drm.send(send()).await, DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled));
    assert!(!drm.metered_sync_disabled(&phone_id()), "reassigned after every send");

    w.phone_drm.set_reply(Box::new(|_| None));
    let start = tokio::time::Instant::now();
    assert_eq!(drm.send(send()).await, DrmSendOutcome::ResponseTimeout);
    assert_eq!(start.elapsed(), Duration::from_secs(120), "nothing is retried");
    assert_eq!(w.phone_drm.count(), 9);
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_starts_a_session_when_the_phone_is_not_available() {
    let w = world();
    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"pong", Some(1)))));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let mut forced = drm_send(DrmAddress::Path("/clipboard".into()));
    forced.force_session_start = true;
    assert_eq!(w.routes.drm.send(forced).await, DrmSendOutcome::Success { payload: b"pong".to_vec() });
    assert!(w.waker.options()[0].request_new_session, "a forced start is NewSession");

    let w = world();
    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"pong", Some(1)))));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert!(matches!(
        w.routes.drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
        DrmSendOutcome::Success { .. }
    ));
    assert!(!w.waker.options()[0].request_new_session, "an unforced start is Default");
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_reports_a_failed_session_start() {
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::LtwDisabled)));
    assert_eq!(
        w.routes.drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
        DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled),
        "PeerDisabled is reported as RemoteAllSyncDisabled"
    );

    // A wake the phone does not answer is not a terminal state: the start waits out its window.
    let w = world();
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.routes.drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
        DrmSendOutcome::SessionStartFailed(ConnectOutcome::Pending {
            last_failure: Some(ConnectionFailure::Wake(WakeStatus::DeviceUnreachable))
        })
    );
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    assert!(w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).is_empty());

    // A fatal wake result stops the attempts and fails the start at once.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceNotFoundError)));
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.routes.drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
        DrmSendOutcome::SessionStartFailed(ConnectOutcome::Failed(ConnectionFailure::Wake(
            WakeStatus::DeviceNotFoundError
        )))
    );
    assert_eq!(start.elapsed(), Duration::ZERO);

    // Cancelled while the session starts.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::UntilCancelled));
    let request = drm_send(DrmAddress::Path("/clipboard".into()));
    let cancel = request.cancel.clone();
    let drm = w.routes.drm.clone();
    let task = tokio::spawn(async move { drm.send(request).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    cancel.cancel();
    assert_eq!(task.await.unwrap(), DrmSendOutcome::Canceled);
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_checks_its_refusals_in_order() {
    // The phone is not connected; every refusal below is reported before a session start.
    let w = world();
    let drm = &w.routes.drm;
    let clipboard = || drm_send(DrmAddress::Path("/clipboard".into()));
    let too_old = Some("1.23112.188".to_owned());
    w.lock.set(LockState::Locked);

    // A path that is not relative comes first.
    let mut request = drm_send(DrmAddress::Path("https://example.com/clipboard".into()));
    request.phone_enabled = false;
    request.client_version = too_old.clone();
    assert_matches!(drm.send(request).await, DrmSendOutcome::InvalidRequest(_));

    // Disabled + locked → UserDisabled.
    let mut request = clipboard();
    request.phone_enabled = false;
    request.client_version = too_old.clone();
    assert_eq!(drm.send(request).await, DrmSendOutcome::UserDisabled);

    // Locked + version too old → Locked.
    let mut request = clipboard();
    request.client_version = too_old.clone();
    assert_eq!(drm.send(request).await, DrmSendOutcome::Locked);

    // Version too old + not connected → LtwMinVersionRequired, and no session start.
    w.lock.set(LockState::Unlocked);
    let mut request = clipboard();
    request.client_version = too_old;
    request.force_session_start = true;
    assert_eq!(drm.send(request).await, DrmSendOutcome::LtwMinVersionRequired);

    settle().await;
    assert!(w.waker.calls().is_empty() && w.hub.opens().is_empty(), "no session start");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (0, 0), "no NewSession start was counted");
    assert!(w.pair.pc_link.sent().is_empty(), "nothing sent");
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_waits_platform_session_maximum_wait_for_the_session_start() {
    let w = world();
    w.waker.script(WakeScript { delay: Duration::from_secs(600), end: WakeEnd::Succeed });
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.routes.drm.send(drm_send(DrmAddress::Path("/clipboard".into()))).await,
        DrmSendOutcome::SessionStartFailed(ConnectOutcome::Pending { last_failure: None })
    );
    assert_eq!(start.elapsed(), Duration::from_secs(120), "session_maximum_wait");

    // The window is the configured PlatformSessionMaximumWaitTime, not the request's timeout.
    let mut config = EngineConfig::default();
    config.platform.session_maximum_wait = Duration::from_secs(45);
    let w = World::new(config);
    w.waker.script(WakeScript { delay: Duration::from_secs(600), end: WakeEnd::Succeed });
    let mut request = drm_send(DrmAddress::Path("/clipboard".into()));
    request.timeout = Duration::from_secs(90);
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.routes.drm.send(request).await,
        DrmSendOutcome::SessionStartFailed(ConnectOutcome::Pending { last_failure: None })
    );
    assert_eq!(start.elapsed(), Duration::from_secs(45));
    assert!(w.pair.pc_link.sent_on(route::DEVICE_RESOURCE_MANAGER).is_empty());
}

#[tokio::test(start_paused = true)]
async fn drm_wrapper_session_start_opens_the_hub_with_the_requests_trace() {
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.phone_drm.set_reply(Box::new(|_| Some(drm_answer(b"pong", Some(1)))));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let request = drm_send(DrmAddress::Path("/clipboard".into()));
    let trace = request.trace.clone();
    assert_eq!(w.routes.drm.send(request).await, DrmSendOutcome::Success { payload: b"pong".to_vec() });
    assert_eq!(w.hub.open_traces(), vec![trace], "the open carries the trace in its connection headers");
}

// ------------------------------------------------------------------------------------------------
// The device-proxy variant
// ------------------------------------------------------------------------------------------------

fn proxy_command(
    resource_type: serde_json::Value,
    request_type: serde_json::Value,
    payload: &[u8],
    path: &str,
) -> Vec<u8> {
    let content = serde_json::json!({
        "resourceType": resource_type,
        "requestType": request_type,
        "payload": base64::prelude::BASE64_STANDARD.encode(payload),
        "resourcePath": path,
    })
    .to_string();

    serde_json::json!({ "name": "SendCommand", "parameters": { "commandContent": content } }).to_string().into_bytes()
}

fn proxy_message(
    request_id: Option<&str>,
    chunk_size: &str,
    body: Vec<u8>,
) -> phonelink_engine::types::TransportMessage {
    let mut h = vec![("_route", route::DEVICE_PROXY), ("ChunkSize", chunk_size), ("ContentType", "application/json")];
    if let Some(id) = request_id {
        h.push(("RequestId", id));
    }

    app(&h, &body)
}

#[tokio::test(start_paused = true)]
async fn device_proxy_round_trip() {
    let w = world();
    let receiver = Receiver::answering(&[1, 2]);
    w.routes.drm.register_receiver("/clipboard", receiver.clone()).unwrap();
    w.pair.inject_session_based(
        proxy_message(Some("req-1"), "64", proxy_command(4.into(), 1.into(), &[5, 6], "/clipboard")),
        "csid-7",
    );
    settle().await;

    let seen = receiver.seen().pop().unwrap();
    assert_eq!(seen.channel, DrmChannel::DeviceProxy);
    assert_eq!(seen.path, "/clipboard");
    assert_eq!(seen.payload, vec![5, 6]);
    assert_eq!(seen.request_type, pb::ProtoDeviceResourceRequestType::Get);
    assert_eq!(seen.request_id.as_deref(), Some("req-1"));
    assert!(w.pair.pc_app.messages().is_empty(), "not handed to the App layer");

    let reply = w.pair.pc_link.sent().pop().expect("a reply");
    assert_eq!(reply.target, phone_id());
    assert_eq!(reply.message.message_type, MessageType::App);
    let body = r#"{"type":"SendCommandResponse","value":"{\"responseType\":1,\"payload\":\"AQI=\"}"}"#;
    assert_eq!(String::from_utf8(reply.message.payload.clone()).unwrap(), body);
    let size = body.len().to_string();
    assert_eq!(
        pairs(&reply.message.headers),
        owned(&[
            ("RequestId", "req-1"),
            ("ContentType", "application/json"),
            ("ChunkId", "0"),
            ("ChunkCount", "1"),
            ("FileSize", size.as_str())
        ])
    );
    assert_eq!(reply.connection_session_id.as_deref(), Some("csid-7"), "the reply echoes the session id");
    assert_eq!(reply.priority, 0);
    assert!(reply.options.skip_wake_if_not_connected);
}

#[tokio::test(start_paused = true)]
async fn device_proxy_accepts_enum_names() {
    let w = world();
    let receiver = Receiver::answering(b"");
    w.routes.drm.register_receiver("/permissions", receiver.clone()).unwrap();

    w.pair.inject(proxy_message(Some("r"), "64", proxy_command("Permissions".into(), "sync".into(), b"", "")));
    settle().await;

    let seen = receiver.seen().pop().unwrap();
    assert_eq!(seen.path, "/permissions", "derived from the type");
    assert_eq!(seen.request_type, pb::ProtoDeviceResourceRequestType::Sync);
    assert_eq!(w.pair.pc_link.sent().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn device_proxy_commands_without_a_reply() {
    let w = world();
    let receiver = Receiver::answering(&[0u8; 2048]);
    w.routes.drm.register_receiver("/clipboard", receiver.clone()).unwrap();
    let command = || proxy_command(4.into(), 1.into(), b"", "/clipboard");

    // The reply would exceed ChunkSize × 1024.
    w.pair.inject(proxy_message(Some("big"), "1", command()));
    // No RequestId, no usable ChunkSize, malformed JSON, unknown path, faulting type.
    w.pair.inject(proxy_message(None, "64", command()));
    w.pair.inject(proxy_message(Some("r"), "lots", command()));
    w.pair.inject(proxy_message(Some("r"), "-1", command()));
    w.pair.inject(proxy_message(Some("r"), "64", b"{not json".to_vec()));
    w.pair.inject(proxy_message(Some("r"), "64", proxy_command(4.into(), 1.into(), b"", "/elsewhere")));
    w.pair.inject(proxy_message(Some("r"), "64", proxy_command(7.into(), 1.into(), b"", "/clipboard")));
    w.pair.inject(proxy_message(Some("r"), "64", proxy_command("Mystery".into(), 1.into(), b"", "/clipboard")));

    settle().await;
    assert!(w.pair.pc_link.sent().is_empty());
    assert_eq!(receiver.seen().len(), 1, "only the oversized one reached the receiver");
}

#[tokio::test(start_paused = true)]
async fn the_phone_session_survives_everything_above() {
    // A connected phone stays connected while unknown and malformed traffic flows.
    let w = world();
    w.connect_phone().await;
    w.pair.inject(platform(
        &[("_route", "/Nope"), ("_requestId", "1"), ("_rejectionVersion", "1"), ("ms-content-type", "text/json")],
        b"{}",
    ));
    w.pair.inject(platform(
        &[("_route", route::CONTEXT_PUBLISH), ("_requestId", "2"), ("ms-content-type", "application/x-binary")],
        &[0xFF],
    ));
    w.pair.inject(proxy_message(Some("r"), "64", b"{".to_vec()));

    settle().await;
    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
}
