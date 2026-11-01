//! The hub connection against a fake hub on 127.0.0.1: the endpoint and HTTP layer, the connection
//! headers, the hub protocol, the connection states and the open sequence, the start retry policy
//! with HTTP 401 and 429, `OpenStatus`, the automatic reconnect, and send results. Real time with
//! short configured timers.

use crate::support::common::retry_until;
use crate::support::fake_hub;
use crate::support::relay_world as fixtures;

use std::collections::BTreeSet;
use std::time::Duration;

use fake_hub::{
    CompletionMode, HandshakeReply, HttpRecord, NegotiateReply, UpgradeReply, close_message, on_connected,
    on_partner_connected,
};
use fixtures::{Harness, PHONE, drain_events, ident, next_event, phone};
use phonelink_engine::codec::dcg_packet::Ack;
use phonelink_engine::codec::hub::{self, method};
use phonelink_engine::seams::{HubControl, HubLink, RelayEvent, TokenError, WakeHub};
use phonelink_engine::tap::{Direction, TapLayer};
use phonelink_engine::types::{
    ConnectReason, DcgClient, Environment, HubSendResult, HubState, OpenStatus, PartnerDisconnectReason, TraceContext,
};
use rmpv::Value;
use tokio::time::Instant;

const SECOND: Duration = Duration::from_secs(1);

fn is_state(state: HubState) -> impl Fn(&RelayEvent) -> bool {
    move |e| matches!(e, RelayEvent::HubState { env: Environment::Prod, state: s } if *s == state)
}

fn trace() -> TraceContext {
    TraceContext::new_root(Vec::new())
}

fn ack_packet() -> hub::MultiplexPacket {
    Ack::success("00000000-0000-0000-0000-000000000001", 1).to_packet()
}

/// The `HubState` events in `events`, in order.
fn states(events: &[RelayEvent]) -> Vec<HubState> {
    events
        .iter()
        .filter_map(|e| match e {
            RelayEvent::HubState { env: Environment::Prod, state } => Some(*state),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------
// Endpoint, headers, handshake
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn negotiate_and_upgrade_carry_exactly_the_connection_headers() {
    let h = Harness::new(|_| {}).await;
    let creation_trace = TraceContext {
        trace_id: "0af7651916cd43dd8448eb211c80319c".into(),
        parent_id: None,
        correlation_id: "b7ad6b7169203331".into(),
        flags: 0,
        trace_state: vec![
            ("ypScenarioId".into(), "ConnectScenario".into()),
            ("ypTriggerId".into(), "UserConnect".into()),
        ],
    };
    let status = h.relay.open_with_trace(&phone(), ConnectReason::ConnectRequested, creation_trace).await.status;
    assert_eq!(status, OpenStatus::Success);

    let negotiates = h.hub.negotiates();
    assert_eq!(negotiates.len(), 1);
    let negotiate = &negotiates[0];
    assert_eq!(negotiate.method, "POST");
    assert_eq!(negotiate.path, "/relayhub/negotiate");
    assert_eq!(negotiate.query.as_deref(), Some("negotiateVersion=1"));
    assert_eq!(negotiate.header("content-length"), Some("0"), "the hub refuses an empty POST without it (411)");

    let upgrades = h.hub.upgrades();
    assert_eq!(upgrades.len(), 1);
    let upgrade = &upgrades[0];
    assert_eq!(upgrade.method, "GET");
    assert_eq!(upgrade.path, "/relayhub/");
    assert_eq!(upgrade.query.as_deref(), Some("id=token-1"));
    assert_eq!(upgrade.header("upgrade"), Some("websocket"));

    let id = ident();
    let tracestate = "ypScenarioId=ConnectScenario,ypTriggerId=UserConnect,yppHttpParent=b7ad6b7169203331";
    let expected: Vec<(&str, &str)> = vec![
        ("dcg-logicaldeviceid", &id.logical_device_id),
        ("dcg-appversion", &id.app_version),
        ("dcg-appid", &id.app_id),
        ("dcg-sessionid", &id.session_id),
        ("dcg-ringname", &id.ring_name),
        ("dcg-os", &id.os),
        ("dcg-osversion", &id.os_version),
        ("dcg-partnerid", PHONE),
        ("dcg-heartbeatfrequency", "15"),
        ("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00"),
        ("tracestate", tracestate),
        ("dcg-tracestate", tracestate),
        ("authorization", "Bearer services-token"),
    ];
    let expected_dcg: BTreeSet<&str> = expected.iter().map(|(k, _)| *k).filter(|k| k.starts_with("dcg-")).collect();

    for record in [negotiate, upgrade] {
        for (name, value) in &expected {
            assert_eq!(record.header(name), Some(*value), "{name} on {} {}", record.method, record.path);
        }
        assert_eq!(record.header("dcg-hubregion"), None, "PC-initiated connections send no DCG-HubRegion");
        let dcg: BTreeSet<&str> =
            record.headers.iter().map(|(k, _)| k.as_str()).filter(|k| k.starts_with("dcg-")).collect();
        assert_eq!(dcg, expected_dcg, "exactly the identification and connection DCG-* headers");
        assert_eq!(record.query_param("access_token"), None, "the bearer is a header, not a query parameter");
    }

    assert_eq!(h.tokens.calls(), vec![(Environment::Prod, false)]);
}

#[tokio::test]
async fn hub_control_open_puts_the_callers_trace_on_the_connection_headers() {
    let mut h = Harness::new(|_| {}).await;
    let creation = TraceContext {
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
        parent_id: Some("1122334455667788".into()),
        correlation_id: "a3ce929d0e0e4736".into(),
        flags: 0,
        trace_state: vec![
            ("ypScenarioId".into(), "WakeScenario".into()),
            ("ypTriggerId".into(), "IncomingWake".into()),
        ],
    };

    let status = HubControl::open(h.relay.as_ref(), &phone(), ConnectReason::WakeRequest, &creation).await.status;
    assert_eq!(status, OpenStatus::Success);

    let tracestate = "ypScenarioId=WakeScenario,ypTriggerId=IncomingWake,yppHttpParent=a3ce929d0e0e4736";
    let carries_creation_trace = |record: &HttpRecord| {
        assert_eq!(
            record.header("traceparent"),
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-a3ce929d0e0e4736-00"),
            "traceparent on {} {}",
            record.method,
            record.path
        );
        assert_eq!(record.header("tracestate"), Some(tracestate), "tracestate on {} {}", record.method, record.path);
        assert_eq!(record.header("dcg-tracestate"), Some(tracestate), "DCG-TraceState on {}", record.path);
        assert_eq!(record.header("dcg-partnerid"), Some(PHONE));
    };

    carries_creation_trace(&h.hub.negotiates()[0]);
    carries_creation_trace(&h.hub.upgrades()[0]);

    // The hub closes without allowReconnect: the connection stays in the map, Disconnected. A later
    // open of it, for another partner and with another trace, re-sends the creation headers.
    h.hub.session(0).await.send(close_message(Some("bye"), None));
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Disconnected)).await;

    let later = TraceContext::new_root(vec![("ypScenarioId".into(), "LaterScenario".into())]);
    let other = DcgClient::new("second-partner", Environment::Prod);
    let status = HubControl::open(h.relay.as_ref(), &other, ConnectReason::SendFragment, &later).await.status;
    assert_eq!(status, OpenStatus::Success);

    assert_eq!(h.hub.negotiates().len(), 2);
    carries_creation_trace(&h.hub.negotiates()[1]);
    carries_creation_trace(&h.hub.upgrades()[1]);
}

#[tokio::test]
async fn handshake_is_sent_binary_and_byte_exact() {
    let h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    let handshake = session.handshake().expect("handshake recorded");
    assert!(handshake.binary, "the handshake goes out as a binary WebSocket message");
    assert_eq!(handshake.bytes, hub::HANDSHAKE_REQUEST);
    assert_eq!(handshake.bytes, b"{\"protocol\":\"messagepack\",\"version\":2}\x1e");
}

#[tokio::test]
async fn handshake_answer_with_coalesced_frames_is_processed() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_coalesce_after_handshake(vec![
        on_connected(Some("westus2"), &["someone-else"]),
        on_partner_connected(PHONE, Value::Nil),
    ]);
    let session = h.open_session().await;

    assert_eq!(h.relay.hub_region(Environment::Prod, 2 * SECOND).await, Ok(Some("westus2".to_string())));
    let reply = session.wait_invocation(2 * SECOND, |i| i.target == method::SEND_CONNECTED).await;
    assert_eq!(reply.target_id(), Some(PHONE));
    assert!(h.relay.is_partner_connected(&phone()));

    // OnConnected.Partners is informational only.
    assert!(!h.relay.is_partner_connected(&DcgClient::new("someone-else", Environment::Prod)));
}

#[tokio::test]
async fn handshake_answer_in_a_text_message_is_accepted() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_handshake_as_text(true);
    h.hub.set_coalesce_after_handshake(vec![on_connected(Some("eastus"), &[])]);
    let _session = h.open_session().await;
    assert_eq!(h.relay.hub_region(Environment::Prod, 2 * SECOND).await, Ok(Some("eastus".to_string())));
}

#[tokio::test]
async fn handshake_error_fails_every_attempt() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_handshake(HandshakeReply::Error("unsupported protocol".into()));
    assert_eq!(h.open().await, OpenStatus::Failure);
    assert_eq!(h.hub.upgrades().len(), 4, "the first start and three retries");
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);
}

// ------------------------------------------------------------------------------------------------
// Start retry policy, 401, 429, OpenStatus
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn http_401_on_negotiate_forces_one_token_refresh() {
    let h = Harness::new(|_| {}).await;
    h.hub.push_negotiate(NegotiateReply::Status(401));
    h.hub.push_negotiate(NegotiateReply::Status(401));
    assert_eq!(h.open().await, OpenStatus::Success);
    assert_eq!(
        h.tokens.calls(),
        vec![(Environment::Prod, false), (Environment::Prod, true), (Environment::Prod, false)],
        "only the second attempt (after the first 401) forces a refresh"
    );
}

#[tokio::test]
async fn http_401_on_the_upgrade_also_forces_the_refresh() {
    let h = Harness::new(|_| {}).await;
    h.hub.push_upgrade(UpgradeReply::Status(401));
    assert_eq!(h.open().await, OpenStatus::Success);
    assert_eq!(h.tokens.calls(), vec![(Environment::Prod, false), (Environment::Prod, true)]);
}

#[tokio::test]
async fn http_401_after_the_first_retry_forces_nothing() {
    let h = Harness::new(|_| {}).await;
    h.hub.push_negotiate(NegotiateReply::Status(500));
    h.hub.push_negotiate(NegotiateReply::Status(401));
    assert_eq!(h.open().await, OpenStatus::Success);
    assert_eq!(h.tokens.calls().iter().filter(|(_, force)| *force).count(), 0);
    assert_eq!(h.tokens.calls().len(), 3);
}

#[tokio::test]
async fn persistent_401_maps_to_auth_transient_failure() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_default_negotiate(NegotiateReply::Status(401));
    assert_eq!(h.open().await, OpenStatus::AuthTransientFailure);
    assert_eq!(h.hub.negotiates().len(), 4);
}

#[tokio::test]
async fn http_429_opens_the_breaker_for_the_drawn_delay() {
    // Breaker delay 400 ms (variance 0), retry sleep 20 ms: the three retries hit the open breaker.
    let h = Harness::new(|_| {}).await;
    h.hub.push_negotiate(NegotiateReply::Status(429));

    let started = Instant::now();
    assert_eq!(h.open().await, OpenStatus::BrokenCircuitError);
    assert!(started.elapsed() < Duration::from_millis(390), "the retries did not wait the breaker out");
    assert_eq!(h.hub.negotiates().len(), 1, "attempts while the breaker is open never reach the hub");

    // Still open: nothing reaches the hub.
    assert_eq!(h.open().await, OpenStatus::BrokenCircuitError);
    assert_eq!(h.hub.negotiates().len(), 1);

    // After the break the next start goes through; a start while it is open reaches nothing.
    let opened = retry_until(|| async {
        match h.open().await {
            OpenStatus::BrokenCircuitError => None,
            status => Some(status),
        }
    })
    .await;
    assert_eq!(opened, Some(OpenStatus::Success));
    assert!(started.elapsed() >= Duration::from_millis(400), "the break lasted {:?}", started.elapsed());
    assert_eq!(h.hub.negotiates().len(), 2);
}

#[tokio::test]
async fn token_failures_follow_the_retry_filter_and_their_open_status() {
    let h = Harness::new(|_| {}).await;
    h.tokens.push(Err(TokenError::UserInteractionRequired));
    assert_eq!(h.open().await, OpenStatus::MsaTokenUserInteractionRequired);
    assert_eq!(h.tokens.calls().len(), 1, "an MSA-token failure is not retried");

    for _ in 0..4 {
        h.tokens.push(Err(TokenError::Crypto("key unavailable".into())));
    }
    assert_eq!(h.open().await, OpenStatus::CryptoManagerFailure);
    assert_eq!(h.tokens.calls().len(), 5, "other token failures are retried three times");

    for _ in 0..4 {
        h.tokens.push(Ok(String::new()));
    }
    assert_eq!(h.open().await, OpenStatus::NonTransientAuthFailure, "an empty token is a hard error");
    assert!(h.hub.negotiates().is_empty(), "no anonymous connect");
}

#[tokio::test]
async fn msa_token_failures_are_not_retried() {
    let h = Harness::new(|_| {}).await;
    h.tokens.push(Err(TokenError::Msa("the token broker failed".into())));
    assert_eq!(h.open().await, OpenStatus::NonTransientAuthFailure);
    assert_eq!(h.tokens.calls().len(), 1, "an MSA-token failure is not retried");
    assert!(h.hub.negotiates().is_empty());
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);

    // Without internet the auth failure's root cause is the missing connectivity.
    h.reachability.set(false);
    h.tokens.push(Err(TokenError::Msa("no route to the token service".into())));
    assert_eq!(h.open().await, OpenStatus::InternetError);
    assert_eq!(h.tokens.calls().len(), 2, "still not retried");

    for _ in 0..4 {
        h.tokens.push(Err(TokenError::Crypto("key unavailable".into())));
    }
    assert_eq!(h.open().await, OpenStatus::InternetError, "every token failure without internet");
    assert_eq!(h.tokens.calls().len(), 6, "a crypto failure is retried three times");
    assert!(h.hub.negotiates().is_empty());
}

#[tokio::test]
async fn hub_start_failures_map_to_failure_or_internet_error() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_default_negotiate(NegotiateReply::Status(503));
    assert_eq!(h.open().await, OpenStatus::Failure);
    h.reachability.set(false);
    assert_eq!(h.open().await, OpenStatus::InternetError);
}

/// The failure's root cause is lack of internet connectivity when its cause chain says so, also
/// while the device reports connectivity: a hub host that does not resolve, or a connection the hub
/// end resets. A refused connection is not such a cause.
#[tokio::test]
async fn hub_start_failures_with_an_internet_cause_open_as_internet_error() {
    let h = Harness::new(|c| c.endpoints.prod_base = "http://relayhub.invalid/".into()).await;
    assert_eq!(h.open().await, OpenStatus::InternetError, "a name that does not resolve");
    assert_eq!(h.tokens.calls().len(), 4, "retried like any other hub start failure");

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let h = Harness::new(|c| c.endpoints.prod_base = format!("http://{closed}/")).await;
    assert_eq!(h.open().await, OpenStatus::Failure, "a refused connection");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            // A zero linger closes with RST.
            socket.set_zero_linger().unwrap();
            drop(socket);
        }
    });

    let h = Harness::new(|c| c.endpoints.prod_base = format!("http://{addr}/")).await;
    assert_eq!(h.open().await, OpenStatus::InternetError, "a reset connection");
}

#[tokio::test]
async fn negotiate_redirect_is_followed_with_its_access_token() {
    let h = Harness::new(|_| {}).await;
    let redirected = format!("{}redirected/", h.hub.base_url());
    h.hub.push_negotiate(NegotiateReply::Redirect { url: redirected, access_token: Some("redirect-token".into()) });
    assert_eq!(h.open().await, OpenStatus::Success);

    let negotiates = h.hub.negotiates();
    assert_eq!(negotiates.len(), 2);
    assert_eq!(negotiates[0].header("authorization"), Some("Bearer services-token"));
    assert_eq!(negotiates[1].path, "/redirected/negotiate");
    assert_eq!(negotiates[1].query.as_deref(), Some("negotiateVersion=1"));
    assert_eq!(negotiates[1].header("authorization"), Some("Bearer redirect-token"));
    assert_eq!(negotiates[1].header("dcg-partnerid"), Some(PHONE));

    let upgrade = &h.hub.upgrades()[0];
    assert_eq!(upgrade.path, "/redirected/");
    assert_eq!(upgrade.header("authorization"), Some("Bearer redirect-token"));
}

#[tokio::test]
async fn endless_redirects_fail_loudly() {
    let h = Harness::new(|_| {}).await;
    let url = format!("{}relayhub/", h.hub.base_url());
    h.hub.set_default_negotiate(NegotiateReply::Redirect { url, access_token: None });
    assert_eq!(h.open().await, OpenStatus::Failure);
    assert_eq!(h.hub.negotiates().len(), 4 * 101, "the redirect bound (100) per attempt");
    assert!(h.hub.upgrades().is_empty());
}

#[tokio::test]
async fn negotiate_without_a_websocket_binary_transport_fails() {
    let h = Harness::new(|_| {}).await;
    h.hub.set_default_negotiate(NegotiateReply::Json(serde_json::json!({
        "negotiateVersion": 1, "connectionId": "c", "connectionToken": "t",
        "availableTransports": [{ "transport": "LongPolling", "transferFormats": ["Text", "Binary"] }]
    })));
    assert_eq!(h.open().await, OpenStatus::Failure);
    assert!(h.hub.upgrades().is_empty());
}

// ------------------------------------------------------------------------------------------------
// Keepalive and server timeout
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn client_pings_on_the_keepalive_and_drops_after_the_server_timeout() {
    let mut h = Harness::new(|c| {
        c.relay.keep_alive = Duration::from_millis(100);
        c.relay.server_timeout = Duration::from_millis(700);
        c.relay.reconnect_delays = vec![Duration::from_secs(30)];
    })
    .await;

    let session = h.open_session().await;
    let opened = Instant::now();
    let pings = session.wait_pings(4, 2 * SECOND).await;

    for pair in pings.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(gap >= Duration::from_millis(80) && gap <= Duration::from_millis(400), "ping gap {gap:?}");
    }

    // Pings from the client do not keep the connection alive: nothing comes from the hub.
    next_event(&mut h.events, 3 * SECOND, is_state(HubState::Reconnecting)).await;
    let elapsed = opened.elapsed();
    assert!(elapsed >= Duration::from_millis(600), "server timeout fired after {elapsed:?}");
    assert!(session.wait_closed(2 * SECOND).await);
}

/// Waits until the client has read `count` more hub pings.
async fn wait_hub_pings_read(h: &Harness, count: usize) {
    let deadline = Instant::now() + 10 * SECOND;
    let mut read = 0;

    loop {
        read += h
            .tap
            .take()
            .into_iter()
            .filter(|e| {
                e.layer == TapLayer::HubMessage && e.direction == Direction::In && e.summary == serde_json::json!([6])
            })
            .count();

        if read >= count {
            return;
        }

        assert!(Instant::now() < deadline, "the client read {read} hub pings, not {count}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn hub_pings_keep_the_connection_alive() {
    let mut h = Harness::new(|c| {
        c.relay.keep_alive = Duration::from_millis(100);
        c.relay.server_timeout = Duration::from_millis(400);
    })
    .await;

    h.hub.set_ping_interval(Some(Duration::from_millis(100)));
    let session = h.open_session().await;

    // Hub pings read over three server timeouts: one every 100 ms, so 13 span 1.2 s.
    wait_hub_pings_read(&h, 13).await;
    assert!(!session.is_closed());
    assert!(drain_events(&mut h.events).iter().all(|e| !is_state(HubState::Reconnecting)(e)));
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);
}

// ------------------------------------------------------------------------------------------------
// Automatic reconnect
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn reconnect_runs_the_schedule_then_closes() {
    // Schedule 0, 150, 300 ms; every reconnect attempt fails.
    let mut h = Harness::new(|_| {}).await;
    h.hub.set_coalesce_after_handshake(vec![
        on_connected(Some("uksouth"), &[]),
        on_partner_connected(PHONE, Value::Nil),
    ]);

    let session = h.open_session().await;
    next_event(&mut h.events, 2 * SECOND, |e| matches!(e, RelayEvent::PartnerConnected(p) if *p == phone())).await;
    assert_eq!(h.relay.resolved_region(Environment::Prod).as_deref(), Some("uksouth"));

    h.hub.set_default_negotiate(NegotiateReply::Status(500));

    let dropped = Instant::now();
    session.drop_socket();
    let removed = next_event(&mut h.events, 2 * SECOND, |e| matches!(e, RelayEvent::PartnerDisconnected { .. })).await;
    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::LocalHubDisconnect }
    );

    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    assert_eq!(h.relay.resolved_region(Environment::Prod).as_deref(), Some("uksouth"), "Reconnecting keeps the region");
    assert!(!h.relay.is_partner_connected(&phone()));

    next_event(&mut h.events, 3 * SECOND, is_state(HubState::Disconnected)).await;

    let attempts: Vec<Instant> = h.hub.negotiates().iter().skip(1).map(|r| r.at).collect();
    assert_eq!(attempts.len(), 3, "one attempt per schedule entry, then Closed");

    let first = attempts[0] - dropped;
    let second = attempts[1] - attempts[0];
    let third = attempts[2] - attempts[1];

    assert!(first < Duration::from_millis(150), "first attempt after {first:?}");
    assert!(second >= Duration::from_millis(140) && second < Duration::from_millis(450), "second gap {second:?}");
    assert!(third >= Duration::from_millis(290) && third < Duration::from_millis(700), "third gap {third:?}");

    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);
    assert_eq!(h.relay.resolved_region(Environment::Prod), None, "Closed forgets the region");
    assert!(h.relay.hub_region(Environment::Prod, Duration::from_millis(100)).await.is_err());
}

#[tokio::test]
async fn reconnect_succeeds_on_a_new_session() {
    let mut h = Harness::new(|_| {}).await;
    let first = h.open_session().await;
    first.drop_socket();

    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Connected)).await;
    assert_eq!(h.tokens.calls().len(), 2, "every reconnect attempt asks the token provider");

    let second = h.hub.session(1).await;
    assert_eq!(second.upgrade.header("dcg-partnerid"), Some(PHONE), "the connection headers are re-sent");
    assert_eq!(
        second.upgrade.header("traceparent"),
        first.upgrade.header("traceparent"),
        "the creation trace context stays the connection's"
    );

    assert_eq!(h.relay.send_connected_to(&phone()).await, HubSendResult::Success);
    let reply = second.wait_invocation(SECOND, |i| i.target == method::SEND_CONNECTED).await;
    assert_eq!(reply.invocation_id.as_deref(), Some("1"), "invocation ids restart per session");
}

#[tokio::test]
async fn a_429_during_reconnect_waits_the_breaker_delay() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    h.hub.push_negotiate(NegotiateReply::Status(429));
    session.drop_socket();

    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    next_event(&mut h.events, 3 * SECOND, is_state(HubState::Connected)).await;

    let negotiates = h.hub.negotiates();
    assert_eq!(negotiates.len(), 3);
    let gap = negotiates[2].at - negotiates[1].at;
    assert!(gap >= Duration::from_millis(380), "after a 429 the breaker delay (400 ms) applies, not 150 ms: {gap:?}");
}

#[tokio::test]
async fn a_429_at_the_last_schedule_index_keeps_the_reconnect_going() {
    // Schedule 0, 150, 300 ms; breaker delay 400 ms. Attempts 1 and 2 fail with 500, attempt 3 (the
    // last index) and attempt 4 (past the schedule) with 429; attempt 5 succeeds.
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    for status in [500, 500, 429, 429] {
        h.hub.push_negotiate(NegotiateReply::Status(status));
    }

    session.drop_socket();
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    let outcome = next_event(&mut h.events, 5 * SECOND, |e| {
        is_state(HubState::Connected)(e) || is_state(HubState::Disconnected)(e)
    })
    .await;
    assert!(is_state(HubState::Connected)(&outcome), "the schedule gave up during the 429s: {outcome:?}");

    let attempts: Vec<Instant> = h.hub.negotiates().iter().skip(1).map(|r| r.at).collect();
    assert_eq!(attempts.len(), 5);
    for k in [3, 4] {
        let gap = attempts[k] - attempts[k - 1];
        assert!(gap >= Duration::from_millis(380), "attempt {} came {gap:?} after a 429", k + 1);
    }
}

#[tokio::test]
async fn attempts_that_failed_with_429_still_count_toward_the_schedule_index() {
    // Schedule 0, 150, 300 ms. Attempt 1 fails with 500, attempt 2 with 429, attempt 3 (after the
    // breaker delay) with 500: three attempts were made, the schedule has no fourth entry → Closed.
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    for status in [500, 429, 500] {
        h.hub.push_negotiate(NegotiateReply::Status(status));
    }

    session.drop_socket();
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    let outcome = next_event(&mut h.events, 5 * SECOND, |e| {
        is_state(HubState::Connected)(e) || is_state(HubState::Disconnected)(e)
    })
    .await;

    assert!(is_state(HubState::Disconnected)(&outcome), "{outcome:?}");
    assert_eq!(h.hub.negotiates().len() - 1, 3, "no attempt past the schedule after a non-429 failure");
}

#[tokio::test]
async fn server_close_reconnects_only_with_allow_reconnect() {
    let mut h = Harness::new(|_| {}).await;
    let first = h.open_session().await;
    first.send(close_message(Some("rebalancing"), Some(true)));
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Connected)).await;

    let second = h.hub.session(1).await;
    second.send(close_message(Some("bye"), None));
    // The close decides: `Reconnecting` with allowReconnect, else `Disconnected`, which nothing leaves
    // but an open.
    let closed = next_event(&mut h.events, 2 * SECOND, |e| {
        is_state(HubState::Reconnecting)(e) || is_state(HubState::Disconnected)(e)
    })
    .await;

    assert!(is_state(HubState::Disconnected)(&closed), "no reconnect without allowReconnect: {closed:?}");
    assert_eq!(h.hub.upgrades().len(), 2, "no reconnect without allowReconnect");
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);
}

#[tokio::test]
async fn open_while_reconnecting_abandons_the_library_reconnect() {
    let mut h = Harness::new(|c| c.relay.reconnect_delays = vec![Duration::from_secs(30)]).await;
    let session = h.open_session().await;
    session.drop_socket();
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Reconnecting)).await;

    let started = Instant::now();
    assert_eq!(h.open().await, OpenStatus::Success);
    assert!(started.elapsed() < Duration::from_secs(5), "the 30 s reconnect delay was not waited out");

    let events = drain_events(&mut h.events);
    let states: Vec<HubState> = events
        .iter()
        .filter_map(|e| match e {
            RelayEvent::HubState { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(states, vec![HubState::Disconnected, HubState::Connecting, HubState::Connected]);
}

#[tokio::test]
async fn a_dropped_open_leaves_the_connection_disconnected_and_the_next_open_starts_afresh() {
    let mut h = Harness::new(|_| {}).await;
    // The hub never answers the handshake: every start stays in flight until its caller gives up.
    h.hub.set_handshake(HandshakeReply::Silent);

    // 1. The caller's select picks another branch once the start is in flight (its WebSocket open).
    tokio::select! {
        _ = h.hub.session(0) => {}
        status = h.open() => panic!("the open completed ({status:?}) although the handshake was never answered"),
    }

    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Disconnected)).await;
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);
    assert!(h.hub.session(0).await.wait_closed(2 * SECOND).await, "the abandoned start closed its WebSocket");

    // 2. The caller's timeout fires.
    let open = tokio::time::timeout(Duration::from_millis(200), h.open()).await;
    assert!(open.is_err(), "the open completed ({open:?}) although the handshake was never answered");
    next_event(&mut h.events, 2 * SECOND, is_state(HubState::Disconnected)).await;
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Disconnected);
    assert!(h.hub.session(1).await.wait_closed(2 * SECOND).await);

    // 3. The next open starts afresh.
    h.hub.set_handshake(HandshakeReply::Ok);
    assert_eq!(h.open().await, OpenStatus::Success);
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);
    assert_eq!(h.hub.upgrades().len(), 3);

    let reported = states(&drain_events(&mut h.events));
    assert_eq!(reported, vec![HubState::Connecting, HubState::Connected]);
}

#[tokio::test]
async fn an_open_queued_behind_a_dropped_one_runs_after_its_cleanup() {
    let mut h = Harness::new(|_| {}).await;
    h.hub.set_handshake(HandshakeReply::Silent);
    let relay = h.relay.clone();
    let first = tokio::spawn(async move {
        HubControl::open(relay.as_ref(), &phone(), ConnectReason::ConnectRequested, &trace()).await
    });
    h.hub.session(0).await;

    // Serialized: the second open waits for the first.
    let relay = h.relay.clone();
    let other = DcgClient::new("second-partner", Environment::Prod);
    let second =
        tokio::spawn(
            async move { HubControl::open(relay.as_ref(), &other, ConnectReason::SendFragment, &trace()).await },
        );

    // The second start will be answered; then the first caller goes away. That the second waited
    // for the first's cleanup shows in the states reported below.
    h.hub.set_handshake(HandshakeReply::Ok);
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let status = tokio::time::timeout(5 * SECOND, second).await.expect("the queued open finishes").unwrap().status;
    assert_eq!(status, OpenStatus::Success);
    assert_eq!(h.hub.upgrades().len(), 2);

    let reported = states(&drain_events(&mut h.events));
    assert_eq!(
        reported,
        vec![HubState::Connecting, HubState::Disconnected, HubState::Connecting, HubState::Connected],
        "the dropped open reported Disconnected before the queued one started"
    );
    assert!(h.hub.session(0).await.wait_closed(2 * SECOND).await);
}

#[tokio::test]
async fn a_close_during_the_start_answers_the_open_cancelled() {
    let mut h = Harness::new(|_| {}).await;
    h.hub.set_handshake(HandshakeReply::Silent);
    let relay = h.relay.clone();
    let open = tokio::spawn(async move {
        HubControl::open(relay.as_ref(), &phone(), ConnectReason::ConnectRequested, &trace()).await
    });
    h.hub.session(0).await;

    h.relay.close(Environment::Prod).await;
    let status = tokio::time::timeout(2 * SECOND, open).await.expect("the open returns").unwrap().status;
    assert_eq!(status, OpenStatus::Cancelled);
    assert!(h.hub.session(0).await.wait_closed(2 * SECOND).await);

    // The next open runs after the cancelled start's cleanup, so its states follow everything that
    // start reported.
    h.hub.set_handshake(HandshakeReply::Ok);
    assert_eq!(h.open().await, OpenStatus::Success);

    let reported = states(&drain_events(&mut h.events));
    assert_eq!(
        reported,
        vec![HubState::Connecting, HubState::Disconnected, HubState::Connecting, HubState::Connected],
        "Disconnected is reported once"
    );
}

#[tokio::test]
async fn close_stops_the_connection_and_a_new_open_creates_a_new_one() {
    let mut h = Harness::new(|_| {}).await;
    let session = h.open_session().await;
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, 2 * SECOND, |e| matches!(e, RelayEvent::PartnerConnected(_))).await;

    h.relay.close(Environment::Prod).await;
    let removed = next_event(&mut h.events, SECOND, |e| matches!(e, RelayEvent::PartnerDisconnected { .. })).await;
    assert_eq!(
        removed,
        RelayEvent::PartnerDisconnected { partner: phone(), reason: PartnerDisconnectReason::LocalHubDisconnect }
    );
    next_event(&mut h.events, SECOND, is_state(HubState::Disconnected)).await;
    assert!(session.wait_closed(2 * SECOND).await);
    assert_eq!(h.relay.send_connected_to(&phone()).await, HubSendResult::NotConnected);

    let other = DcgClient::new("second-partner", Environment::Prod);
    assert_eq!(
        h.relay.open(&other, ConnectReason::ConnectRequested, &TraceContext::new_root(Vec::new())).await.status,
        OpenStatus::Success
    );
    assert_eq!(h.hub.negotiates().last().unwrap().header("dcg-partnerid"), Some("second-partner"));
}

#[tokio::test]
async fn partners_share_the_first_partners_connection() {
    let h = Harness::new(|_| {}).await;
    assert_eq!(h.open().await, OpenStatus::Success);
    let other = DcgClient::new("second-partner", Environment::Prod);
    assert_eq!(
        h.relay.open(&other, ConnectReason::SendFragment, &TraceContext::new_root(Vec::new())).await.status,
        OpenStatus::Success
    );

    assert_eq!(h.hub.negotiates().len(), 1, "already connected: Success without touching the hub");
    assert_eq!(h.hub.upgrades()[0].header("dcg-partnerid"), Some(PHONE));
    assert_eq!(h.relay.hub_state(Environment::Beta), HubState::Disconnected, "environments are never shared");
}

// ------------------------------------------------------------------------------------------------
// Send results
// ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn sends_before_the_connection_is_up_are_not_connected() {
    let h = Harness::new(|_| {}).await;
    assert_eq!(h.relay.send_packet(&phone(), &trace(), ack_packet(), None).await, HubSendResult::NotConnected);
    assert_eq!(h.relay.flush_connected(&phone()).await, HubSendResult::NotConnected);
    assert!(h.hub.sessions().is_empty());

    h.hub.set_default_negotiate(NegotiateReply::Status(500));
    assert_eq!(h.open().await, OpenStatus::Failure);
    assert_eq!(h.relay.send_packet(&phone(), &trace(), ack_packet(), None).await, HubSendResult::NotConnected);
}

#[tokio::test]
async fn sends_map_completions_to_send_results() {
    let h = Harness::new(|_| {}).await;
    let session = h.open_session().await;

    assert_eq!(h.relay.send_packet(&phone(), &trace(), ack_packet(), None).await, HubSendResult::Success);
    assert_eq!(
        h.relay.send_packet(&phone(), &trace(), ack_packet(), Some("cs-9".into())).await,
        HubSendResult::Success
    );

    h.hub.set_completion_for(method::SEND_MESSAGE, CompletionMode::Error("hub says no".into()));
    assert_eq!(h.relay.send_packet(&phone(), &trace(), ack_packet(), None).await, HubSendResult::Failure);

    h.reachability.set(false);
    assert_eq!(h.relay.send_packet(&phone(), &trace(), ack_packet(), None).await, HubSendResult::InternetError);

    let invocations = session.invocations();
    let ids: Vec<&str> = invocations.iter().filter_map(|i| i.invocation_id.as_deref()).collect();
    assert_eq!(ids, vec!["1", "2", "3", "4"], "invocation ids are incrementing decimal strings");

    assert_eq!(invocations[0].target, method::SEND_MESSAGE);
    assert_eq!(invocations[0].arguments.len(), 3);
    assert_eq!(invocations[1].target, method::SEND_SESSION_BASED_MESSAGE);
    assert_eq!(invocations[1].connection_session_id(), Some("cs-9"));
    assert_eq!(invocations[0].packet().unwrap(), ack_packet());

    // [1, {}, "1", "SendMessageAsync", [...], []] — headers empty, streamIds empty.
    let payload = &invocations[0].payload;
    assert_eq!(&payload[..4], &[0x96, 0x01, 0x80, 0xA1]);
    assert_eq!(*payload.last().unwrap(), 0x90);
}

#[tokio::test]
async fn pending_invocations_fail_when_the_connection_drops() {
    let h = Harness::new(|c| c.relay.reconnect_delays = vec![Duration::from_secs(30)]).await;
    h.hub.set_completion_for(method::SEND_MESSAGE, CompletionMode::Never);
    let session = h.open_session().await;

    let relay = h.relay.clone();
    let send = tokio::spawn(async move { relay.send_packet(&phone(), &trace(), ack_packet(), None).await });
    session.wait_invocation(SECOND, |i| i.target == method::SEND_MESSAGE).await;

    session.drop_socket();
    let result = tokio::time::timeout(2 * SECOND, send).await.expect("the send completes").unwrap();
    assert_eq!(result, HubSendResult::Failure);
}
