//! The per-partner connection: the connection start policies and session control sequences; the
//! partner disconnect reasons and reconnection gating, the automatic reconnect (the per-partner
//! doubling delay and its resets); the connection's wake TTL.

use crate::support::platform_world as fakes;

use std::time::Duration;

use fakes::*;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::platform::session::WaitOutcome;
use phonelink_engine::platform::{SessionPhase, route};
use phonelink_engine::seams::{
    ConnectOutcome, ConnectRequest, ConnectionFailure, PartnerConnector, PartnerSessionControl, SessionEvent,
    StartPolicy,
};
use phonelink_engine::types::{
    ConnectReason, DcgSendStatus, Environment, HubSendResult, HubState, OpenStatus, PartnerDisconnectReason,
    SendOptions, TraceContext, WakeOptions, WakeStatus,
};
use prost::Message;

const WINDOW: Duration = Duration::from_secs(70);

fn connect(policy: StartPolicy) -> ConnectRequest {
    ConnectRequest {
        policy,
        ignore_device_disabled: false,
        feedback_window: WINDOW,
        trace: TraceContext::new_root(Vec::new()),
    }
}

fn world() -> World {
    World::new(EngineConfig::default())
}

fn wake_after(secs: u64, end: WakeEnd) -> WakeScript {
    WakeScript { delay: Duration::from_secs(secs), end }
}

/// A connect request with its own feedback window.
fn connect_within(policy: StartPolicy, window: Duration) -> ConnectRequest {
    ConnectRequest { feedback_window: window, ..connect(policy) }
}

/// Seconds since `start` of every wake call.
fn wake_times(w: &World, start: tokio::time::Instant) -> Vec<u64> {
    w.waker.calls().into_iter().map(|c| (c.at - start).as_secs()).collect()
}

/// Seconds between successive wake calls.
fn wake_gaps(w: &World) -> Vec<u64> {
    let times: Vec<_> = w.waker.calls().into_iter().map(|c| c.at).collect();
    times.windows(2).map(|pair| (pair[1] - pair[0]).as_secs()).collect()
}

/// The default and the long wake TTL, in seconds.
const TTL: u64 = 20;
const LONG_TTL: u64 = 45 * 60;

fn end_session(w: &World, reason: i32) {
    let body = phonelink_engine::proto::platform_sdk::v1::PlatformDisconnectRequest { trace_context: None, reason }
        .encode_to_vec();
    w.pair.inject(platform(
        &[("_route", route::PLATFORM_DISCONNECT_REQUEST), ("ms-content-type", "application/x-binary")],
        &body,
    ));
}

// ------------------------------------------------------------------------------------------------
// Attempts
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn an_attempt_wakes_a_partner_that_is_not_on_the_hub() {
    let w = world();
    let mut events = w.sessions.subscribe();
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(w.waker.calls().len(), 1);
    assert!(w.hub.opens().is_empty(), "the hub was already up");
    assert_eq!(w.waker.options()[0], WakeOptions { request_new_session: false, ignore_device_disabled: false });

    settle().await;
    assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert_eq!(w.waker.ttls(), vec![TTL], "the first wake uses the default TTL");
    assert_eq!(w.phone_validation.count(), 1, "/SessionValidation once per connect");
    assert_eq!(w.phone_activate.count(), 0, "no activate for a connect that started an attempt");
}

#[tokio::test(start_paused = true)]
async fn an_attempt_opens_a_hub_that_is_down_then_wakes() {
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::ConnectRequested)]);
    assert_eq!(w.waker.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_attempt_re_announces_a_partner_already_on_the_hub() {
    let w = world();
    w.connect_phone().await;
    end_session(&w, 1);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 1 });

    // The phone is still on the hub: a NewSession start re-announces instead of waking.
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await, ConnectOutcome::Connected);
    settle().await;
    assert_eq!(w.hub.announces(), vec![phone_id()]);
    assert!(w.waker.calls().is_empty());
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (1, 0), "no wake took the NewSession count");
}

#[tokio::test(start_paused = true)]
async fn hub_open_failures() {
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open(OpenStatus::InternetError);
    w.waker.set_default(WakeScript::now(WakeEnd::Succeed));

    // Retryable: not a terminal state; the request waits while the next attempt, after the 2 s
    // delay, opens and wakes.
    let start = tokio::time::Instant::now();
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    assert_eq!(w.hub.opens().len(), 2);
    assert!(w.sessions.is_connected(&phone_id()));

    // A fatal status stops further attempts.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open(OpenStatus::NonTransientAuthFailure);

    let outcome = w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await;
    assert_eq!(outcome, ConnectOutcome::Failed(ConnectionFailure::Hub(OpenStatus::NonTransientAuthFailure)));
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(w.hub.opens().len(), 1);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
}

// ------------------------------------------------------------------------------------------------
// Start policies on a connected partner
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_connect_request_on_a_connected_partner_activates_the_session() {
    let w = world();
    w.connect_phone().await;
    for policy in [StartPolicy::Default, StartPolicy::NewSession] {
        let request = ConnectRequest {
            policy,
            ignore_device_disabled: true,
            feedback_window: WINDOW,
            trace: TraceContext::new_root(Vec::new()),
        };
        assert_eq!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Connected);
    }

    settle().await;
    assert_eq!(w.phone_activate.count(), 2);
    let sent = w.pair.pc_link.sent_on(route::PLATFORM_ACTIVATE_SESSION_REQUEST);
    assert_eq!(sent.len(), 2);
    let minted = sent[0].message.headers.get("_requestId").unwrap().to_owned();
    assert!(minted.parse::<u32>().is_ok(), "a decimal id from the process-wide counter: {minted}");
    assert_eq!(
        pairs(&sent[0].message.headers),
        owned(&[
            ("_requestId", minted.as_str()),
            ("_route", "/PlatformActivateSessionRequest"),
            ("ms-content-type", "text/json"),
            ("_rejectionVersion", "1")
        ])
    );
    assert_eq!(sent[0].message.payload, b"{}");
    assert_eq!(sent[0].priority, 0);
    assert_eq!(sent[0].options, SendOptions { one_way: true, skip_wake_if_not_connected: true, ..SendOptions::NONE });
    assert!(w.waker.calls().is_empty(), "starts nothing");
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (0, 0), "no counter for a start that started nothing");
}

// ------------------------------------------------------------------------------------------------
// Session ended by the phone
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_phone_ended_session_ignores_default_and_new_session_leaves_it() {
    let w = world();
    w.connect_phone().await;
    end_session(&w, 2);
    settle().await;
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 2 });

    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason: 2 })
    );

    // The phone coming back on the hub by itself does not leave the state either: its last failure is
    // cleared and the hub connection it came back on is released (EnsureDisconnected).
    w.hub.partner_joins(&phone_id());
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 2 });
    assert_eq!(w.sessions.last_failure(&phone_id()), None);
    assert_eq!(w.hub.closes(), vec![Environment::Prod], "the hub connection is closed");
    assert!(!w.sessions.is_connected(&phone_id()));
    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason: 2 })
    );

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(w.waker.calls().is_empty(), "no automatic reconnect or wake");

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await, ConnectOutcome::Connected);
    assert_eq!(w.waker.options(), vec![WakeOptions { request_new_session: true, ignore_device_disabled: false }]);
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::ConnectRequested)], "the released hub reopens");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_session_ended_for_a_reason_that_is_not_persistent_stays_usable_while_the_phone_is_on_the_hub() {
    for reason in [5, 6] {
        let w = world();
        w.connect_phone().await;
        let mut events = w.sessions.subscribe();
        end_session(&w, reason);
        settle().await;
        assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason });

        // Externally connected: requests go out, a Default start is answered at once.
        assert!(w.sessions.is_connected(&phone_id()), "reason {reason}");
        assert_eq!(w.sessions.connected_partners(), vec![phone_id()]);
        assert!(events.try_recv().is_err(), "no Disconnected while the connection stays usable");

        assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
        settle().await;
        assert_eq!(w.phone_activate.count(), 0, "not Connected: no activate request");
        assert!(w.waker.calls().is_empty() && w.hub.announces().is_empty(), "nothing started");
        assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason });

        // The phone leaves the hub: not connected any more; still ended.
        w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
        settle().await;
        assert!(!w.sessions.is_connected(&phone_id()));
        assert_eq!(
            events.recv().await.unwrap(),
            SessionEvent::Disconnected {
                partner: phone_id(),
                failure: ConnectionFailure::PartnerDisconnect(PartnerDisconnectReason::NoticeFromHub)
            }
        );
        assert_eq!(
            w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
            ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason })
        );
    }

    // A persistent reason (0–4) is not usable even while the phone stays on the hub.
    let w = world();
    w.connect_phone().await;
    end_session(&w, 4);
    settle().await;
    assert!(!w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn a_local_disconnect_keeps_a_phone_ended_session_ended() {
    // `/PlatformDisconnectRequest` reason 4, then the engine stops and starts again: the stop must not
    // take the connection out of the phone-ended state, so the background Default connect wakes nothing.
    let w = world();
    w.connect_phone().await;
    end_session(&w, 4);
    settle().await;

    w.sessions.disconnect(&phone_id()).await;
    settle().await;

    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 4 });
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::SessionEnded { reason: 4 }));
    assert_eq!(w.hub.closes(), vec![Environment::Prod], "the hub connection is still released");
    assert!(w.pair.pc_link.sent_on(route::PLATFORM_DISCONNECT_REQUEST).is_empty(), "not connected: no request");
    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason: 4 })
    );

    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(w.waker.calls().is_empty() && w.hub.opens().is_empty(), "no wake, no open");

    // Externally connected (reason 5): the request is sent, and the session stays ended.
    let w = world();
    w.connect_phone().await;
    end_session(&w, 5);
    settle().await;

    w.sessions.disconnect(&phone_id()).await;
    settle().await;

    assert_eq!(w.pair.pc_link.sent_on(route::PLATFORM_DISCONNECT_REQUEST).len(), 1);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::SessionEnded { reason: 5 });
}

#[tokio::test(start_paused = true)]
async fn a_local_stop_records_a_local_stop_only_from_connected() {
    let w = world();
    // Attempting (the first wake failed): the stop keeps the last failure, which is not persistent.
    assert!(matches!(
        w.sessions.connect(&phone_id(), connect_within(StartPolicy::Default, Duration::from_secs(1))).await,
        ConnectOutcome::Pending { .. }
    ));

    w.sessions.disconnect(&phone_id()).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Stopped);
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::Wake(WakeStatus::DeviceUnreachable)));
}

// ------------------------------------------------------------------------------------------------
// A running attempt: queueing and the extended budget
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_second_request_queues_behind_a_short_attempt() {
    let w = world();
    w.waker.script(wake_after(10, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(wake_after(1, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sessions = w.sessions.clone();
    let second = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    // A failed attempt ends neither request: both wait for the connection.
    assert_eq!(first.await.unwrap(), ConnectOutcome::Connected);
    assert_eq!(second.await.unwrap(), ConnectOutcome::Connected);

    // The queued attempt ran at once at 10 s, without the retry delay — but the failure still doubled
    // the delay, so the retry after the second failure (11 s) waits 4 s.
    assert_eq!(wake_times(&w, start), vec![0, 10, 15]);
    assert_eq!(w.waker.cancelled.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn a_request_cancels_an_attempt_on_the_extended_budget() {
    // The first wake is unreachable: the retry (after 2 s) runs on the extended budget and wakes with
    // the 45-min TTL. A connect request cancels it, not queued behind it; the cancelled attempt reset
    // the delays, so the request's attempt wakes with the default TTL.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::UntilCancelled));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.waker.calls().len(), 2);

    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(w.waker.cancelled.load(std::sync::atomic::Ordering::SeqCst), 1, "the 45-min wake was cancelled");
    assert_eq!(wake_times(&w, start), vec![0, 2, 5]);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL]);
    assert_eq!(first.await.unwrap(), ConnectOutcome::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_request_during_the_retry_wait_starts_the_next_attempt_at_once() {
    let w = world();

    // Unreachable at t=0; the next attempt would wait 2 s.
    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);

    let times: Vec<_> = w.waker.calls().into_iter().map(|c| c.at).collect();
    assert_eq!(times[1] - times[0], Duration::from_millis(500), "the retry wait ended");
    assert_eq!(first.await.unwrap(), ConnectOutcome::Connected);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL], "ending a retry wait resets no delay: the TTL stays alternated");
}

#[tokio::test(start_paused = true)]
async fn the_extended_budget_is_judged_when_an_attempt_starts() {
    // The first wake fails without being unreachable: the retry (after 2 s) wakes with the default
    // TTL, yet runs on the extended budget because the retry delay grew — a connect request cancels it.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::TransportNetworkError)));
    w.waker.script(WakeScript::now(WakeEnd::UntilCancelled));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.waker.ttls(), vec![TTL, TTL], "default TTL both times");

    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(w.waker.cancelled.load(std::sync::atomic::Ordering::SeqCst), 1, "cancelled, not queued behind");
    assert_eq!(wake_times(&w, start), vec![0, 2, 5]);
    assert_eq!(first.await.unwrap(), ConnectOutcome::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_attempt_resets_the_delays() {
    // A wake left `DeviceUnreachable` (the next uses the 45-min TTL); the retry's 45-min wake is
    // cancelled by a request; the attempt that follows wakes with the default TTL again.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::UntilCancelled));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    tokio::time::sleep(Duration::from_secs(5)).await;
    let request = connect_within(StartPolicy::Default, Duration::from_secs(1));
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));

    // The wake after the cancel uses the default TTL, and its failure backs off from the start value
    // (2 s), not from the grown delay (4 s).
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(w.waker.ttls()[..4], [TTL, LONG_TTL, TTL, LONG_TTL]);
    assert_eq!(wake_times(&w, start)[..4], [0, 2, 5, 7]);
}

#[tokio::test(start_paused = true)]
async fn the_wake_ttl_alternates_after_unreachable_attempts_only() {
    // The wake sequence: the connection keeps the TTL; each attempt whose wake is unreachable flips
    // it, other failures leave it.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::TransportNetworkError)));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, LONG_TTL, TTL, LONG_TTL]);

    // The wake that connected reset the delays, so the next wake uses the default TTL again.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::SystemReset);
    settle().await;
    assert_eq!(w.waker.ttls()[5], TTL);
}

#[tokio::test(start_paused = true)]
async fn the_send_path_wakes_with_the_default_ttl_and_leaves_the_connection_ttl_alone() {
    // The wake sequence. An attempt's wake is unreachable: the connection's next wake uses the
    // 45-min TTL.
    let w = world();
    let request = connect_within(StartPolicy::Default, Duration::from_secs(1));
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));

    // A send during the retry wait wakes with the default TTL, and its unreachable result does not
    // flip the connection's TTL back.
    let trace = TraceContext::new_root(Vec::new());
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &trace).await,
        Err(DcgSendStatus::WakeFailed { status: WakeStatus::DeviceUnreachable, cause: None })
    );

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(w.sessions.is_connected(&phone_id()), "the retry at 2 s connected");
    assert_eq!(w.waker.ttls(), vec![TTL, TTL, LONG_TTL]);
}

#[tokio::test(start_paused = true)]
async fn the_delays_reset_when_the_hub_opens() {
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    // Attempt 1 opens and wakes at 0 s (fails); the hub connection closes at 1 s, which interrupts the
    // retry wait: the next attempt opens it again at once, and that open resets the delay, so the
    // retry after its failure waits 2 s, not 4 s.
    tokio::time::sleep(Duration::from_secs(1)).await;
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(w.hub.opens().len(), 2);
    assert_eq!(wake_times(&w, start)[..3], [0, 1, 3]);
}

/// Starts a `Default` connect whose first wake fails `DeviceUnreachable` at once (the retry delay is
/// then 2 s and the next wake would use the 45-min TTL), without waiting for its outcome.
fn connect_after_one_unreachable_wake(w: &World) {
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
}

#[tokio::test(start_paused = true)]
async fn the_partner_joining_by_itself_keeps_the_delays() {
    // The partner joins by itself during the retry wait and leaves before the wait runs out: the
    // interruption's wake still uses the 45-min TTL, and its failure waits 4 s, not 2 s.
    let w = world();
    let start = tokio::time::Instant::now();
    connect_after_one_unreachable_wake(&w);

    tokio::time::sleep(Duration::from_millis(500)).await;
    w.hub.partner_joins(&phone_id());
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);

    tokio::time::sleep(Duration::from_millis(500)).await;
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::SystemReset);
    tokio::time::sleep(Duration::from_secs(5)).await;

    assert_eq!(wake_times(&w, start), vec![0, 1, 5]);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL]);
}

#[tokio::test(start_paused = true)]
async fn a_retry_wait_that_runs_out_with_the_partner_on_the_hub_resets_the_delays() {
    // The partner joins by itself at 0.5 s; the retry wait runs out at 2 s with it on the hub, which
    // resets the delays and starts no attempt. A later interruption wakes with the default TTL, and its
    // failure waits 2 s.
    let w = world();
    let start = tokio::time::Instant::now();
    connect_after_one_unreachable_wake(&w);

    tokio::time::sleep(Duration::from_millis(500)).await;
    w.hub.partner_joins(&phone_id());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(w.waker.calls().len(), 1, "no attempt when the wait ran out");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::SystemReset);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(wake_times(&w, start), vec![0, 3, 5]);
    assert_eq!(w.waker.ttls(), vec![TTL, TTL, LONG_TTL]);
}

#[tokio::test(start_paused = true)]
async fn a_re_announce_keeps_the_delays() {
    // The delays grew (2 s, the 45-min TTL) before the partner joined by itself; the phone then ends the
    // session, and a NewSession start finds it on the hub and re-announces it. The re-announce resets
    // nothing: the interruption's wake uses the 45-min TTL and its failure waits 4 s.
    let w = world();
    let start = tokio::time::Instant::now();
    connect_after_one_unreachable_wake(&w);

    tokio::time::sleep(Duration::from_millis(500)).await;
    w.hub.partner_joins(&phone_id());
    settle().await;

    end_session(&w, 1);
    settle().await;
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await, ConnectOutcome::Connected);
    settle().await;
    assert_eq!(w.hub.announces(), vec![phone_id()]);

    tokio::time::sleep_until(start + Duration::from_secs(1)).await;
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::TooManyAckTimeouts);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(wake_times(&w, start), vec![0, 1, 5]);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL]);
}

#[tokio::test(start_paused = true)]
async fn a_failure_after_the_partner_joined_by_itself_waits_then_resets_the_delays() {
    // The retry at 2 s wakes for 5 s; the partner joins by itself at 3 s, and the wake then fails
    // without being fatal. The connection stays connected, the failure's 4-s wait runs out at 11 s with
    // the partner on the hub, and that resets the delays: the interruption at 12 s wakes with the default
    // TTL and its failure waits 2 s.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(wake_after(5, WakeEnd::Fail(WakeStatus::TransportNetworkError)));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    tokio::time::sleep(Duration::from_secs(3)).await;
    w.hub.partner_joins(&phone_id());
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert_eq!(w.sessions.last_failure(&phone_id()), None, "a retryable failure records nothing");

    tokio::time::sleep_until(start + Duration::from_secs(12)).await;
    assert_eq!(w.waker.calls().len(), 2, "no attempt when the wait ran out");

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::SystemReset);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(wake_times(&w, start), vec![0, 2, 12, 14]);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL, LONG_TTL]);
}

#[tokio::test(start_paused = true)]
async fn a_fatal_failure_after_the_partner_joined_by_itself_resets_the_delays_and_stays_connected() {
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(wake_after(5, WakeEnd::Fail(WakeStatus::LtwDisabled)));

    let start = tokio::time::Instant::now();
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    tokio::time::sleep(Duration::from_secs(3)).await;
    w.hub.partner_joins(&phone_id());
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::Wake(WakeStatus::LtwDisabled)));

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::SystemReset);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(wake_times(&w, start), vec![0, 2, 8, 10]);
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL, LONG_TTL]);
}

// ------------------------------------------------------------------------------------------------
// Backoff
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn failed_attempts_back_off_doubling_until_fatal() {
    let w = world();
    // The request waits out its 70 s window (attempts at 0, 2, 6, 14, 30 and 62 s) and the attempts go on.
    let start = tokio::time::Instant::now();
    let outcome = w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await;
    assert_eq!(
        outcome,
        ConnectOutcome::Pending { last_failure: Some(ConnectionFailure::Wake(WakeStatus::DeviceUnreachable)) }
    );
    assert_eq!(start.elapsed(), WINDOW);
    assert_eq!(wake_times(&w, start), vec![0, 2, 6, 14, 30, 62]);

    tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
    let expected: Vec<u64> = (1..=14).map(|k| 1u64 << k).collect(); // 2 s … 16384 s; 32768 s ≥ 6 h is fatal
    assert_eq!(wake_gaps(&w), expected);
    assert_eq!(w.waker.calls().len(), 15);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::Wake(WakeStatus::DeviceUnreachable)));

    tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    assert_eq!(w.waker.calls().len(), 15, "no attempt after the fatal delay");

    // An explicit request starts again from there.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
}

#[tokio::test(start_paused = true)]
async fn the_fatal_bound_resets_the_delays() {
    let w = world();
    let request = connect_within(StartPolicy::Default, Duration::from_secs(1));
    assert!(matches!(w.sessions.connect(&phone_id(), request.clone()).await, ConnectOutcome::Pending { .. }));

    tokio::time::sleep(Duration::from_secs(12 * 3600)).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
    assert_eq!(w.waker.calls().len(), 15);

    // A later start backs off from the start value again instead of going fatal after one attempt.
    let start = tokio::time::Instant::now();
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(wake_times(&w, start)[15..], [0, 2, 6]);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Attempting);
}

#[tokio::test(start_paused = true)]
async fn a_connect_request_waits_for_a_terminal_state() {
    // Two wakes fail, the third connects: the request reports connected, not the first failure.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::TransportNetworkError)));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let start = tokio::time::Instant::now();
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert_eq!(start.elapsed(), Duration::from_secs(6));

    // The attempts stopping is a terminal state.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open(OpenStatus::InternetError);
    w.hub.script_open(OpenStatus::MsaTokenUserInteractionRequired);

    let start = tokio::time::Instant::now();
    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Failed(ConnectionFailure::Hub(OpenStatus::MsaTokenUserInteractionRequired))
    );
    assert_eq!(start.elapsed(), Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn fatal_wake_results_stop_the_attempts() {
    for status in [
        WakeStatus::Failure,
        WakeStatus::DeviceNotFoundError,
        WakeStatus::IncompatibleRegionError,
        WakeStatus::LtwDisabled,
        WakeStatus::MsaTokenUserInteractionRequired,
    ] {
        let w = world();
        w.waker.set_default(WakeScript::now(WakeEnd::Fail(status)));
        let start = tokio::time::Instant::now();
        assert_eq!(
            w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
            ConnectOutcome::Failed(ConnectionFailure::Wake(status)),
            "{status:?}"
        );
        assert_eq!(start.elapsed(), Duration::ZERO);

        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        assert_eq!(w.waker.calls().len(), 1, "{status:?}: one wake, not one per back-off step");
        assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
        assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::Wake(status)));
    }
    for status in [
        WakeStatus::DeviceUnreachable,
        WakeStatus::TransportInternetError,
        WakeStatus::TransportNetworkError,
        WakeStatus::InternetError,
        WakeStatus::Unauthorized,
        WakeStatus::WakeBrokenCircuitError,
    ] {
        let w = world();
        w.waker.set_default(WakeScript::now(WakeEnd::Fail(status)));
        let request = connect_within(StartPolicy::Default, Duration::from_secs(1));
        assert_eq!(
            w.sessions.connect(&phone_id(), request).await,
            ConnectOutcome::Pending { last_failure: Some(ConnectionFailure::Wake(status)) }
        );

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(w.waker.calls().len(), 2, "{status:?}: retried after 2 s");
    }
}

#[tokio::test(start_paused = true)]
async fn success_resets_the_retry_delay() {
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));

    let mut events = w.sessions.subscribe();
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });

    // Wakes at 0 s and 2 s fail; the one at 6 s connects.
    assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));
    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(wake_gaps(&w), vec![2, 4]);

    // After an interruption that may reconnect the first attempt runs at once, and its failure waits
    // the base delay doubled once (2 s), not 8 s.
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::TooManyAckTimeouts);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(wake_gaps(&w)[2..], [0, 2]);
    assert!(w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn the_first_attempt_after_an_interruption_runs_at_once_and_later_failures_back_off() {
    // Reconnection gating: the interrupted connection starts an attempt at once; only failed
    // attempts wait the doubling delay.
    let w = world();
    w.connect_phone().await;

    let start = tokio::time::Instant::now();
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::ExceededMissedHeartbeats);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(wake_times(&w, start), vec![0, 2, 6], "at once, then after 2 s and 4 s");
    assert_eq!(w.waker.ttls(), vec![TTL, LONG_TTL, TTL]);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Attempting);
}

// ------------------------------------------------------------------------------------------------
// Wake options
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn pending_counters_give_one_wake_each() {
    let w = world();
    let request = ConnectRequest {
        policy: StartPolicy::NewSession,
        ignore_device_disabled: true,
        feedback_window: WINDOW,
        trace: TraceContext::new_root(Vec::new()),
    };

    let request = ConnectRequest { feedback_window: Duration::from_secs(1), ..request };
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));
    tokio::time::sleep(Duration::from_secs(3)).await; // the retry after 2 s

    let options = w.waker.options();
    assert_eq!(options[0], WakeOptions { request_new_session: true, ignore_device_disabled: true });
    assert_eq!(options[1], WakeOptions { request_new_session: false, ignore_device_disabled: false });

    // Two NewSession starts give two wakes the flag.
    let w = world();
    w.waker.script(wake_after(10, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(wake_after(10, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));

    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    let sessions = w.sessions.clone();
    let second = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await });

    let _ = (first.await.unwrap(), second.await.unwrap());
    let flags: Vec<bool> = w.waker.options().iter().map(|o| o.request_new_session).collect();
    assert_eq!(flags[..3], [true, true, false]);
}

#[tokio::test(start_paused = true)]
async fn a_count_no_wake_took_stays_for_the_automatic_re_wake() {
    let w = world();
    w.connect_phone().await;
    end_session(&w, 1);
    settle().await;
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await, ConnectOutcome::Connected);
    settle().await;
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (1, 0));

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::ExceededMissedHeartbeats);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(w.waker.options(), vec![WakeOptions { request_new_session: true, ignore_device_disabled: false }]);
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (0, 0));
}

#[tokio::test(start_paused = true)]
async fn keep_session_on_peer_disconnect_forces_request_new_session() {
    let mut config = EngineConfig::default();
    config.relay.keep_session_on_peer_disconnect = true;
    let w = World::new(config);
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await, ConnectOutcome::Connected);
    assert!(w.waker.options()[0].request_new_session);
}

// ------------------------------------------------------------------------------------------------
// Feedback window and last failure
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_feedback_window_reports_the_current_state_and_attempts_go_on() {
    let w = world();
    w.waker.script(wake_after(100, WakeEnd::Succeed));
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Pending { last_failure: None }
    );
    assert_eq!(start.elapsed(), WINDOW);
    assert_eq!(w.sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await, WaitOutcome::Connected);
    assert_eq!(start.elapsed(), Duration::from_secs(100));
}

#[tokio::test(start_paused = true)]
async fn persistent_failures_survive_a_default_request_only() {
    let w = world();
    w.connect_phone().await;
    w.sessions.disconnect(&phone_id()).await;

    let quick = |policy| ConnectRequest {
        policy,
        ignore_device_disabled: false,
        feedback_window: Duration::ZERO,
        trace: TraceContext::new_root(Vec::new()),
    };
    assert_eq!(
        w.sessions.connect(&phone_id(), quick(StartPolicy::Default)).await,
        ConnectOutcome::Pending { last_failure: Some(ConnectionFailure::LocalStop) },
        "a local stop is persistent"
    );

    settle().await;
    assert_eq!(
        w.sessions.connect(&phone_id(), quick(StartPolicy::NewSession)).await,
        ConnectOutcome::Pending { last_failure: None }
    );

    // LTW disabled from a wake is persistent too (and fatal); a transient failure is cleared.
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::LtwDisabled)));
    assert_eq!(
        w.sessions.connect(&phone_id(), connect(StartPolicy::Default)).await,
        ConnectOutcome::Failed(ConnectionFailure::Wake(WakeStatus::LtwDisabled))
    );

    w.waker.set_default(wake_after(30, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    assert_eq!(
        w.sessions.connect(&phone_id(), quick(StartPolicy::Default)).await,
        ConnectOutcome::Pending { last_failure: Some(ConnectionFailure::Wake(WakeStatus::LtwDisabled)) }
    );

    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    assert_eq!(
        w.sessions.connect(&phone_id(), connect_within(StartPolicy::Default, Duration::from_secs(1))).await,
        ConnectOutcome::Pending { last_failure: Some(ConnectionFailure::Wake(WakeStatus::DeviceUnreachable)) }
    );

    w.waker.set_default(wake_after(30, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    assert_eq!(
        w.sessions.connect(&phone_id(), quick(StartPolicy::Default)).await,
        ConnectOutcome::Pending { last_failure: None }
    );
}

// ------------------------------------------------------------------------------------------------
// Partner disconnects
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn notice_from_hub_ends_the_session_by_default() {
    let w = world();
    let mut events = w.sessions.subscribe();
    w.connect_phone().await;
    assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert_eq!(
        events.recv().await.unwrap(),
        SessionEvent::Disconnected {
            partner: phone_id(),
            failure: ConnectionFailure::PartnerDisconnect(PartnerDisconnectReason::NoticeFromHub)
        }
    );

    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(w.waker.calls().is_empty());
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn notice_from_hub_reconnects_only_when_capable_and_enabled_and_not_blocked() {
    for (capable, automatic, block, reconnects) in
        [(true, true, false, true), (false, true, false, false), (true, false, false, false), (true, true, true, false)]
    {
        let mut config = EngineConfig::default();
        config.relay.automatic_reconnects_enabled = automatic;
        config.relay.keep_session_on_peer_disconnect = block;
        let w = World::new(config);

        w.connect_phone().await;
        if capable {
            w.sessions.set_partner_ypp_capabilities(&phone_id(), 0x40 | 0x20);
        }

        w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert_eq!(!w.waker.calls().is_empty(), reconnects, "capable {capable}, automatic {automatic}, block {block}");

        if reconnects {
            assert_eq!(wake_gaps(&w).len(), w.waker.calls().len() - 1);
        }
    }

    // The capability bit is 0x40 exactly, and once set it stays: a later value without it (a connect
    // response, a refreshed device list) does not clear it.
    let w = world();
    w.sessions.set_partner_ypp_capabilities(&phone_id(), 0x3F);
    assert!(!w.sessions.is_intentional_disconnect_capable(&phone_id()));
    w.sessions.set_partner_ypp_capabilities(&phone_id(), 0x40);
    assert!(w.sessions.is_intentional_disconnect_capable(&phone_id()));
    w.sessions.set_partner_ypp_capabilities(&phone_id(), 0x20);
    assert!(w.sessions.is_intentional_disconnect_capable(&phone_id()), "latched");
}

#[tokio::test(start_paused = true)]
async fn other_disconnect_reasons_reconnect_at_once() {
    for reason in [
        PartnerDisconnectReason::TooManyAckTimeouts,
        PartnerDisconnectReason::SystemReset,
        PartnerDisconnectReason::ExceededMissedHeartbeats,
    ] {
        let w = world();
        w.connect_phone().await;
        let mut events = w.sessions.subscribe();

        w.waker.script(WakeScript::now(WakeEnd::Succeed));
        w.hub.partner_leaves(&phone_id(), reason);
        settle().await;
        assert_eq!(
            events.recv().await.unwrap(),
            SessionEvent::Disconnected { partner: phone_id(), failure: ConnectionFailure::PartnerDisconnect(reason) }
        );

        assert_eq!(w.waker.calls().len(), 1, "{reason:?}: the first attempt does not wait a retry delay");
        assert!(w.sessions.is_connected(&phone_id()));
        assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));
    }
}

#[tokio::test(start_paused = true)]
async fn a_closed_hub_connection_reconnects_at_once() {
    let w = world();
    w.connect_phone().await;
    let mut events = w.sessions.subscribe();

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.lose_connection(Environment::Prod, HubState::Disconnected);
    settle().await;

    assert_eq!(
        events.recv().await.unwrap(),
        SessionEvent::Disconnected { partner: phone_id(), failure: ConnectionFailure::ConnectionDropped }
    );
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::AutomaticReconnect)], "no retry wait first");
    assert_eq!(w.waker.calls().len(), 1);
    assert!(w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn a_reconnecting_hub_pauses_the_partner() {
    let w = world();
    w.connect_phone().await;

    w.hub.lose_connection(Environment::Prod, HubState::Reconnecting);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);
    assert!(w.hub.opens().is_empty() && w.waker.calls().is_empty(), "no attempts of its own");

    // The library reconnect succeeds and the phone re-announces itself.
    w.hub.set_state(Environment::Prod, HubState::Connected);
    w.hub.partner_joins(&phone_id());
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert!(w.waker.calls().is_empty());

    // A reconnect that gives up (Closed) restarts the attempts at once.
    w.hub.lose_connection(Environment::Prod, HubState::Reconnecting);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    settle().await;
    assert_eq!(w.hub.opens().len(), 1);
    assert!(w.sessions.is_connected(&phone_id()));
}

// ------------------------------------------------------------------------------------------------
// Local disconnect
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn local_disconnect_sends_the_request_then_closes() {
    let w = world();
    w.connect_phone().await;
    let mut events = w.sessions.subscribe();
    w.sessions.disconnect(&phone_id()).await;
    settle().await;

    assert_eq!(w.phone_disconnect.count(), 1);
    let sent = w.pair.pc_link.sent_on(route::PLATFORM_DISCONNECT_REQUEST).pop().unwrap();
    assert_eq!(sent.message.payload, b"{}");
    assert_eq!(sent.message.headers.get("ms-content-type"), Some("text/json"));
    assert_eq!(sent.priority, 0);
    assert_eq!(sent.options, SendOptions { one_way: true, skip_wake_if_not_connected: true, ..SendOptions::NONE });
    assert_eq!(w.hub.closes(), vec![Environment::Prod]);
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Stopped);
    assert_eq!(w.sessions.last_failure(&phone_id()), Some(ConnectionFailure::LocalStop));
    assert_eq!(
        events.recv().await.unwrap(),
        SessionEvent::Disconnected { partner: phone_id(), failure: ConnectionFailure::LocalStop }
    );

    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(
        w.waker.calls().is_empty() && w.hub.opens().is_empty(),
        "the hub's partner removal after the close reconnects nothing"
    );
}

#[tokio::test(start_paused = true)]
async fn local_disconnect_variants() {
    // SkipPartnerAck when asked.
    let w = world();
    w.connect_phone().await;
    w.sessions.disconnect_with(&phone_id(), true).await;

    let sent = w.pair.pc_link.sent_on(route::PLATFORM_DISCONNECT_REQUEST).pop().unwrap();
    assert!(sent.options.skip_partner_ack && sent.options.one_way && sent.options.skip_wake_if_not_connected);

    // Not connected: no request, still closes.
    let w = world();
    w.sessions.disconnect(&phone_id()).await;
    assert!(w.pair.pc_link.sent_on(route::PLATFORM_DISCONNECT_REQUEST).is_empty());
    assert_eq!(w.hub.closes(), vec![Environment::Prod]);

    // A request that is never acknowledged is bounded by 30 s, then the disconnect proceeds.
    let w = world();
    w.connect_phone().await;
    w.pair.pc_link.script(Behavior::Hang);
    let start = tokio::time::Instant::now();
    w.sessions.disconnect(&phone_id()).await;
    assert_eq!(start.elapsed(), Duration::from_secs(30));
    assert_eq!(w.hub.closes(), vec![Environment::Prod]);
}

// ------------------------------------------------------------------------------------------------
// The fragment layer's wake path
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn connect_for_send_wakes_with_the_messages_own_options() {
    let w = world();
    // Present: nothing to do.
    w.hub.partner_joins(&phone_id());
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Ok(())
    );

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;

    // Absent: one wake with the message's own options; a pending NewSession count stays untouched.
    let w = world();
    w.connect_phone().await;
    end_session(&w, 1);
    settle().await;
    assert_eq!(w.sessions.connect(&phone_id(), connect(StartPolicy::NewSession)).await, ConnectOutcome::Connected);
    settle().await;
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (1, 0));

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), true, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Ok(())
    );

    assert_eq!(w.waker.options(), vec![WakeOptions { request_new_session: false, ignore_device_disabled: true }]);
    assert_eq!(w.sessions.pending_wake_options(&phone_id()), (1, 0), "the send-path wake touches no counter");

    // Hub down: open first.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Ok(())
    );
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::SendFragment)]);

    // Failures.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open(OpenStatus::InternetError);
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Err(DcgSendStatus::HubSendFailed { result: HubSendResult::InternetError, cause: None })
    );

    // The open's cause travels with the status.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open_failure(OpenStatus::NonTransientAuthFailure, "negotiate answered 401");
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Err(DcgSendStatus::HubSendFailed {
            result: HubSendResult::NotConnected,
            cause: Some("negotiate answered 401".into())
        })
    );

    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Err(DcgSendStatus::WakeFailed { status: WakeStatus::DeviceUnreachable, cause: None })
    );

    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::UntilCancelled));
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.sessions
            .connect_for_send(&phone_id(), false, Duration::from_secs(20), &TraceContext::new_root(Vec::new()))
            .await,
        Err(DcgSendStatus::TimedOut)
    );

    assert_eq!(start.elapsed(), Duration::from_secs(20));
    settle().await;
    assert_eq!(w.waker.cancelled.load(std::sync::atomic::Ordering::SeqCst), 1, "the wake was cancelled");
}

#[tokio::test(start_paused = true)]
async fn should_block_also_reaches_the_send_path_wake() {
    let mut config = EngineConfig::default();
    config.relay.keep_session_on_peer_disconnect = true;
    let w = World::new(config);

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, WINDOW, &TraceContext::new_root(Vec::new())).await,
        Ok(())
    );
    assert!(w.waker.options()[0].request_new_session);
}

// ------------------------------------------------------------------------------------------------
// Presence
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_phone_contacting_the_pc_first_connects_the_session() {
    let w = world();
    let mut events = w.sessions.subscribe();

    w.hub.partner_joins(&phone_id());
    assert_eq!(events.recv().await.unwrap(), SessionEvent::Connected(phone_id()));
    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(w.sessions.connected_partners(), vec![phone_id()]);
    assert!(w.waker.calls().is_empty());
    assert_eq!(w.sessions.wait_connected(&phone_id(), Duration::ZERO).await, WaitOutcome::Connected);

    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    settle().await;
    assert_ne!(w.sessions.wait_connected(&phone_id(), Duration::from_secs(5)).await, WaitOutcome::Connected);
}

#[tokio::test(start_paused = true)]
async fn a_stale_disconnect_event_does_not_interrupt_a_partner_that_is_back() {
    let w = world();
    w.connect_phone().await;
    let mut events = w.sessions.subscribe();

    // Both events are queued before the session loop runs: the leave is stale when applied.
    w.hub.partner_leaves(&phone_id(), PartnerDisconnectReason::NoticeFromHub);
    w.hub.partner_joins(&phone_id());
    settle().await;

    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert!(events.try_recv().is_err(), "no Disconnected/Connected pair for a stale event");
}

// ------------------------------------------------------------------------------------------------
// The hub connection reconnecting: attempting partners pause too (the hub's automatic reconnect)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_reconnecting_hub_pauses_a_partner_in_its_retry_delay() {
    let w = world();

    // The first wake fails at once: the next attempt would follow after 2 s.
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    w.hub.set_state(Environment::Prod, HubState::Reconnecting);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(w.hub.opens().is_empty(), "no open of its own: it would abandon the library reconnect");
    assert_eq!(w.waker.calls().len(), 1, "no wake while the hub reconnects");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    // The library reconnect succeeds: the paused attempts resume at once, wake and connect.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.set_state(Environment::Prod, HubState::Connected);
    settle().await;

    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert_eq!(w.waker.calls().len(), 2);
    assert!(w.hub.opens().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_reconnecting_hub_cancels_a_wake_in_flight_and_a_close_restarts_the_attempts() {
    let w = world();
    w.waker.script(wake_after(30, WakeEnd::Succeed));

    let sessions = w.sessions.clone();
    let request = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(w.waker.calls().len(), 1, "the wake is in flight");

    w.hub.set_state(Environment::Prod, HubState::Reconnecting);
    settle().await;
    assert_eq!(w.waker.cancelled.load(std::sync::atomic::Ordering::SeqCst), 1, "the wake in flight is cancelled");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(w.hub.opens().is_empty() && w.waker.calls().len() == 1, "no attempts of its own while reconnecting");

    // The request's feedback window ended meanwhile: paused attempts are not failures.
    assert_eq!(request.await.unwrap(), ConnectOutcome::Pending { last_failure: None });

    // The library gives up (`Closed`): the attempts restart at once, without a retry wait first.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    settle().await;
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::AutomaticReconnect)]);
    assert_eq!(w.waker.calls().len(), 2);
    assert!(w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn a_close_of_the_hub_connection_interrupts_an_attempting_partner() {
    // The first wake is unreachable at 0 s; the retry would follow after 2 s. The established hub
    // connection closes at 0.5 s: the retry wait ends and the next attempt opens the hub at once.
    let w = world();
    let start = tokio::time::Instant::now();
    let request = connect_within(StartPolicy::Default, Duration::from_millis(100));
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));

    tokio::time::sleep(Duration::from_millis(400)).await;
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    settle().await;

    assert_eq!(w.hub.opens().len(), 1, "the interrupted connection opens the hub again");
    let times: Vec<_> = w.waker.calls().into_iter().map(|c| c.at - start).collect();
    assert_eq!(times.len(), 2);
    assert!(times[1] < Duration::from_secs(1), "no retry wait: {times:?}");
    assert!(w.sessions.is_connected(&phone_id()));
}

#[tokio::test(start_paused = true)]
async fn a_failed_hub_open_interrupts_no_attempting_partner() {
    // `Disconnected` after `Connecting` is an open that failed, not a close: the retry wait goes on.
    let w = world();
    let start = tokio::time::Instant::now();
    let request = connect_within(StartPolicy::Default, Duration::from_millis(100));
    assert!(matches!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { .. }));

    tokio::time::sleep(Duration::from_millis(400)).await;
    w.hub.set_state(Environment::Prod, HubState::Connecting);
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    settle().await;

    assert_eq!(w.waker.calls().len(), 1, "the retry wait goes on");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(wake_times(&w, start), vec![0, 2]);
    assert_eq!(w.hub.opens().len(), 1, "the retry opened the hub");
}

#[tokio::test(start_paused = true)]
async fn a_close_ends_the_pause_with_an_attempt_at_once_and_failures_back_off() {
    let w = World::with_hub(EngineConfig::default(), HubState::Connected);
    w.connect_phone().await;
    w.hub.lose_connection(Environment::Prod, HubState::Reconnecting);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    // The library gives up; the hub opens again, the phone stays unreachable.
    let start = tokio::time::Instant::now();
    w.hub.set_state(Environment::Prod, HubState::Disconnected);
    tokio::time::sleep(Duration::from_secs(10)).await;

    assert_eq!(w.hub.opens().len(), 1);
    assert_eq!(wake_times(&w, start), vec![0, 2, 6]);
}

#[tokio::test(start_paused = true)]
async fn a_paused_partner_that_re_appears_is_connected() {
    let w = world();
    w.waker.script(wake_after(30, WakeEnd::Succeed));

    let sessions = w.sessions.clone();
    let request = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_secs(5)).await;

    w.hub.set_state(Environment::Prod, HubState::Reconnecting);
    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    // The library reconnects and the phone re-announces itself.
    w.hub.set_state(Environment::Prod, HubState::Connected);
    w.hub.partner_joins(&phone_id());

    assert_eq!(request.await.unwrap(), ConnectOutcome::Connected, "the waiting request reports connected");
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Connected);
    assert_eq!(w.waker.calls().len(), 1, "no new wake: the partner is back");
    assert!(w.hub.opens().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_connect_request_during_the_pause_starts_an_attempt() {
    let w = world();
    w.connect_phone().await;
    w.hub.lose_connection(Environment::Prod, HubState::Reconnecting);
    settle().await;
    assert_eq!(w.sessions.phase(&phone_id()), SessionPhase::Paused);

    // The connection start policies: "the hub connection reconnecting" → start an attempt, whose open
    // abandons the library reconnect.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let request = connect(StartPolicy::Default);
    let trace = request.trace.clone();
    assert_eq!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Connected);
    assert_eq!(w.hub.opens(), vec![(phone_id(), ConnectReason::ConnectRequested)]);
    assert_eq!(w.hub.open_traces(), vec![trace]);
}

// ------------------------------------------------------------------------------------------------
// Waiting for the connection across its attempts (`PartnerSessions::wait_connected`)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn wait_connected_is_ended_by_stopped_attempts_not_by_a_failed_attempt() {
    let w = world();
    w.waker.script(wake_after(30, WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    let request = ConnectRequest { feedback_window: Duration::from_secs(5), ..connect(StartPolicy::Default) };
    assert_eq!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { last_failure: None });

    let start = tokio::time::Instant::now();
    assert_eq!(w.sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await, WaitOutcome::TimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(60), "the attempt failing at 25 s did not end it");

    // Attempts that stop during the wait end it, with their failure.
    let w = world();
    w.waker.script(wake_after(30, WakeEnd::Fail(WakeStatus::DeviceNotFoundError)));
    let request = ConnectRequest { feedback_window: Duration::from_secs(5), ..connect(StartPolicy::Default) };
    assert_eq!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Pending { last_failure: None });
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await,
        WaitOutcome::Failed(ConnectionFailure::Wake(WakeStatus::DeviceNotFoundError))
    );
    assert_eq!(start.elapsed(), Duration::from_secs(25));
}

#[tokio::test(start_paused = true)]
async fn wait_connected_fails_when_the_connection_reaches_a_state_it_cannot_leave_by_itself() {
    // The phone ends the session during the wait (`/PlatformDisconnectRequest`).
    let w = world();
    w.waker.script(wake_after(100, WakeEnd::Succeed));
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    settle().await;

    let sessions = w.sessions.clone();
    let wait = tokio::spawn(async move { sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await });
    tokio::time::sleep(Duration::from_secs(10)).await;
    end_session(&w, 3);
    assert_eq!(wait.await.unwrap(), WaitOutcome::Failed(ConnectionFailure::SessionEnded { reason: 3 }));

    // Already ended when the wait begins: at once.
    let start = tokio::time::Instant::now();
    assert_eq!(
        w.sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await,
        WaitOutcome::Failed(ConnectionFailure::SessionEnded { reason: 3 })
    );
    assert_eq!(start.elapsed(), Duration::ZERO);

    // A local stop during the wait.
    let w = world();
    w.waker.script(wake_after(100, WakeEnd::Succeed));
    let sessions = w.sessions.clone();
    tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    settle().await;

    let sessions = w.sessions.clone();
    let wait = tokio::spawn(async move { sessions.wait_connected(&phone_id(), Duration::from_secs(60)).await });
    tokio::time::sleep(Duration::from_secs(10)).await;
    w.sessions.disconnect(&phone_id()).await;
    assert_eq!(wait.await.unwrap(), WaitOutcome::Failed(ConnectionFailure::LocalStop));
}

#[tokio::test(start_paused = true)]
async fn wait_connected_times_out_when_no_attempt_ends_within_it() {
    let w = world();
    w.waker.script(WakeScript::now(WakeEnd::Fail(WakeStatus::DeviceUnreachable)));
    w.waker.script(wake_after(100, WakeEnd::Succeed));

    // The first attempt fails before the wait begins (at 0 s).
    assert_eq!(
        w.sessions.connect(&phone_id(), connect_within(StartPolicy::Default, Duration::ZERO)).await,
        ConnectOutcome::Pending { last_failure: None }
    );
    settle().await;

    let start = tokio::time::Instant::now();
    assert_eq!(w.sessions.wait_connected(&phone_id(), Duration::from_secs(20)).await, WaitOutcome::TimedOut);
    assert_eq!(start.elapsed(), Duration::from_secs(20));

    // The retry (woken at 2 s) is still waiting for the phone; a longer wait sees it connect at 102 s.
    assert_eq!(w.sessions.wait_connected(&phone_id(), Duration::from_secs(100)).await, WaitOutcome::Connected);
    assert_eq!(start.elapsed() + Duration::from_millis(5), Duration::from_secs(102));
}

// ------------------------------------------------------------------------------------------------
// The trace an open carries (the connection headers)
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn hub_opens_carry_the_trace_of_the_request_that_caused_them() {
    // A connect request's open carries its trace.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let request = connect(StartPolicy::Default);
    let trace = request.trace.clone();
    assert_eq!(w.sessions.connect(&phone_id(), request).await, ConnectOutcome::Connected);
    assert_eq!(w.hub.open_traces(), vec![trace.clone()]);

    // An automatic reconnect (the hub connection closed) opens under a fresh root context.
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    w.hub.lose_connection(Environment::Prod, HubState::Disconnected);
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(w.sessions.is_connected(&phone_id()));
    assert_eq!(w.hub.opens().last(), Some(&(phone_id(), ConnectReason::AutomaticReconnect)));

    let traces = w.hub.open_traces();
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[1].parent_id, None, "a root context");
    assert_ne!(traces[1].trace_id, trace.trace_id, "not the earlier request's trace");

    // A request that cuts a retry wait short: the attempt it causes opens with its trace.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.script_open(OpenStatus::InternetError);
    let first = connect(StartPolicy::Default);
    let first_trace = first.trace.clone();

    let sessions = w.sessions.clone();
    let first = tokio::spawn(async move { sessions.connect(&phone_id(), first).await });
    tokio::time::sleep(Duration::from_millis(500)).await;

    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let second = connect(StartPolicy::NewSession);
    let second_trace = second.trace.clone();
    assert_eq!(w.sessions.connect(&phone_id(), second).await, ConnectOutcome::Connected);

    assert_eq!(w.hub.open_traces(), vec![first_trace, second_trace]);
    assert_eq!(first.await.unwrap(), ConnectOutcome::Connected);

    // The send path's open carries the message's trace.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.waker.script(WakeScript::now(WakeEnd::Succeed));
    let message_trace = TraceContext::new_root(vec![("ypScenarioId".into(), "7".into())]);

    assert_eq!(w.sessions.connect_for_send(&phone_id(), false, WINDOW, &message_trace).await, Ok(()));
    assert_eq!(w.hub.open_traces(), vec![message_trace]);
}

#[tokio::test(start_paused = true)]
async fn a_hub_open_in_flight_is_never_abandoned_midway() {
    // An attempt stopped while its open runs: the open finishes in the background.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.set_open_delay(Duration::from_secs(10));

    let sessions = w.sessions.clone();
    let request = tokio::spawn(async move { sessions.connect(&phone_id(), connect(StartPolicy::Default)).await });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(w.hub.opens().len(), 1, "the open is in flight");

    end_session(&w, 1);
    assert_eq!(request.await.unwrap(), ConnectOutcome::Failed(ConnectionFailure::SessionEnded { reason: 1 }));

    tokio::time::sleep(Duration::from_secs(20)).await;
    assert_eq!(w.hub.opens_dropped(), 0, "an open dropped midway would leave the relay's connection half-started");
    assert!(w.waker.calls().is_empty(), "the stopped attempt goes no further");

    // Neither the send path's deadline nor a caller that stops waiting abandons the open.
    let w = World::with_hub(EngineConfig::default(), HubState::Disconnected);
    w.hub.set_open_delay(Duration::from_secs(30));
    let trace = TraceContext::new_root(Vec::new());
    assert_eq!(
        w.sessions.connect_for_send(&phone_id(), false, Duration::from_secs(5), &trace).await,
        Err(DcgSendStatus::TimedOut)
    );

    let gave_up =
        tokio::time::timeout(Duration::from_secs(1), w.sessions.connect_for_send(&phone_id(), false, WINDOW, &trace))
            .await;
    assert!(gave_up.is_err(), "the caller stopped waiting first");

    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(w.hub.opens().len(), 2);
    assert_eq!(w.hub.opens_dropped(), 0);
}
