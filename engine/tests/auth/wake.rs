//! The wake sequence and `/Dispatcher/Wake` over a fake `WakeHub` and the fake DCG server; the wake
//! results of the Dispatcher call's transport failures against local sockets.

use crate::assert_matches;
use crate::support::auth_world;
use crate::support::common::{poll_once, retry_until};

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use auth_world::fake_dcg::error_body;
use auth_world::harness::{Harness, Phone, eventually};
use auth_world::jwt;
use auth_world::wake_hub::{FakeWakeHub, Partner};
use phonelink_engine::auth::dcg_rest::{DcgRest, RestError, TransportCause, WakeBody};
use phonelink_engine::auth::wake::dispatcher_status;
use phonelink_engine::auth::{AuthError, AuthService};
use phonelink_engine::codec::json::OrderedStringMap;
use phonelink_engine::config::EngineConfig;
use phonelink_engine::edge::{Clock, Reachability};
use phonelink_engine::identification::ClientIdentification;
use phonelink_engine::net;
use phonelink_engine::seams::{HubRegionTimeout, Waker};
use phonelink_engine::tap::Tap;
use phonelink_engine::types::{DcgClient, Environment, HubSendResult, TraceContext, WakeOptions, WakeStatus};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// The default and the long wake TTL.
const TTL: Duration = Duration::from_secs(20);
const LONG_TTL: Duration = Duration::from_secs(45 * 60);

struct Ready {
    h: Harness,
    service: Arc<AuthService>,
    phone: Phone,
    target: DcgClient,
}

async fn ready(configure: impl FnOnce(&mut EngineConfig)) -> Ready {
    let mut h = Harness::new().await;
    configure(&mut h.config);
    let (service, _, phone, _) = h.with_phone().await;
    h.dcg.take_requests();
    let target = DcgClient::new(phone.id.clone(), Environment::Prod);
    Ready { h, service, phone, target }
}

fn waker(r: &Ready, hub: &Arc<FakeWakeHub>) -> Arc<dyn Waker> {
    r.service.waker(hub.clone())
}

async fn wake(w: &Arc<dyn Waker>, target: &DcgClient) -> WakeStatus {
    w.wake(target, WakeOptions::default(), TTL, CancellationToken::new()).await.status
}

fn key_order(body: &str, keys: &[&str]) -> bool {
    let positions: Vec<usize> = keys.iter().map(|k| body.find(&format!("\"{k}\":")).expect(k)).collect();
    positions.windows(2).all(|w| w[0] < w[1])
}

#[tokio::test]
async fn golden_wake_body_and_crypto_wake_jwt() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(*hub.flushes.lock().unwrap(), std::slice::from_ref(&r.target), "SendConnectedAsync(target) first");
    assert_eq!(*hub.region_waits.lock().unwrap(), [(Environment::Prod, Duration::from_secs(10))]);
    assert_eq!(hub.waits(), [Duration::from_secs(20)], "max(ttl 20 s, 10 s)");

    let wakes = r.h.dcg.of("Wake");
    assert_eq!(wakes.len(), 1);
    let req = &wakes[0];
    assert_eq!(req.path_and_query, "/Dispatcher/Wake?api-version=1.5.0");
    assert_eq!(req.header("Authorization"), Some("Bearer svc-token-1"), "the services token");
    assert_eq!(req.header("Authorization-Type"), None);
    assert_eq!(req.header("AuthorizationPortal"), Some("MSM"));
    for (name, value) in r.h.ident().headers() {
        assert_eq!(req.header(name), Some(value));
    }

    let token = req.json()["data"]["DCG-CryptoWakeJwt"].as_str().unwrap().to_string();
    let own = r.service.own_client().unwrap();
    assert_eq!(
        req.body,
        format!(
            concat!(
                r#"{{"deviceId":"{}","ttl":20,"priority":"normal","collapseKey":"YPPWake","data":{{"#,
                r#""DCG-Environment":"Prod","IgnoreDeviceDisabledStatus":"False","DCG-RequestNewSession":"False","#,
                r#""DCG-HubRegion":"westus2","DCG-CryptoWakeJwt":"{}"}}}}"#
            ),
            r.phone.id, token
        )
    );

    let j = jwt::decode(&token);
    assert_eq!(j.header, r#"{"alg":"ES384","typ":"JWT"}"#);
    assert_eq!(j.claim_order(), ["iss", "Data", "SourceId", "Scope", "nbf", "exp"]);
    assert_eq!(j.str("iss"), format!("trust_{}", own.id), "the relationship's SelfClientId");
    assert_eq!(j.str("SourceId"), own.id);
    assert_eq!(j.str("Scope"), "wake");
    assert_eq!(
        j.str("Data"),
        r#"{"DCG-Environment":"Prod","IgnoreDeviceDisabledStatus":"False","DCG-RequestNewSession":"False","DCG-HubRegion":"westus2"}"#
    );
    let now = r.h.clock.now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    assert_eq!((j.i64("nbf"), j.i64("exp")), (now - 43_200, now + 43_200));

    let trust_cert = r.h.trust_cert(Environment::Prod);
    assert!(jwt::verifies(&j, &trust_cert), "signed with the trust key");
    let published = r
        .h
        .dcg
        .with(|s| s.enrolled.values().next().unwrap().certificates.clone().unwrap().self_signed.unwrap()[0].clone());
    assert_eq!(jwt::unb64(&published), trust_cert, "the certificate the phone sees in the device list");
}

#[tokio::test]
async fn wake_options_and_region() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    let options = WakeOptions { request_new_session: true, ignore_device_disabled: true };
    assert_eq!(w.wake(&r.target, options, TTL, CancellationToken::new()).await.status, WakeStatus::Success);

    *hub.region.lock().unwrap() = Ok(None);
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);

    let wakes = r.h.dcg.of("Wake");
    let d0 = &wakes[0].json()["data"];
    assert_eq!(
        (d0["IgnoreDeviceDisabledStatus"].as_str(), d0["DCG-RequestNewSession"].as_str()),
        (Some("True"), Some("True"))
    );
    let d1 = &wakes[1].json()["data"];
    assert!(d1.get("DCG-HubRegion").is_none(), "no region known → no key");
    assert_eq!(d1.as_object().unwrap().len(), 4);

    let r = ready(|c| c.relay.keep_session_on_peer_disconnect = true).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(
        r.h.dcg.of("Wake")[0].json()["data"]["DCG-RequestNewSession"],
        "True",
        "YPPShouldDropWakeNewSessionFlag"
    );
}

#[tokio::test]
async fn unreachable_retries_normal_then_high_with_the_callers_ttl() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::Never);
    let w = waker(&r, &hub);

    let summary = |from: usize| -> Vec<(String, u64)> {
        r.h.dcg.of("Wake")[from..]
            .iter()
            .map(|req| (req.json()["priority"].as_str().unwrap().to_string(), req.json()["ttl"].as_u64().unwrap()))
            .collect()
    };

    assert_eq!(wake(&w, &r.target).await, WakeStatus::DeviceUnreachable);
    assert_eq!(summary(0), [("normal".into(), 20), ("high".into(), 20), ("high".into(), 20)], "2 retries");
    assert_eq!(hub.waits(), [Duration::from_secs(20); 3]);

    // The TTL is the caller's: the connection's 45-min one …
    hub.set_partner(Partner::Never);
    let status = w.wake(&r.target, WakeOptions::default(), LONG_TTL, CancellationToken::new()).await.status;
    assert_eq!(status, WakeStatus::DeviceUnreachable);
    assert_eq!(summary(3), [("normal".into(), 2700), ("high".into(), 2700), ("high".into(), 2700)]);
    assert_eq!(hub.waits(), [Duration::from_secs(2700); 3]);

    // … and after any number of unreachable cycles a wake with the default TTL — a send's — sends
    // 20 s: the waker alternates nothing.
    hub.set_partner(Partner::OnWait(2));
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(summary(6), [("normal".into(), 20), ("high".into(), 20)]);
}

#[tokio::test]
async fn concurrent_wakes_of_one_target_are_deduplicated() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::Block);
    let w = waker(&r, &hub);

    let (w1, w2, w3) = (w.clone(), w.clone(), w.clone());
    let (t1, t2, t3) = (r.target.clone(), r.target.clone(), r.target.clone());
    let first =
        tokio::spawn(async move { w1.wake(&t1, WakeOptions::default(), TTL, CancellationToken::new()).await.status });
    assert!(eventually(Duration::from_secs(5), || hub.waits().len() == 1).await);

    // A joining wake gets the result of the wake in flight, whatever TTL it asked for. A wake joins
    // when it is first polled.
    let mut second = w2.wake(&t2, WakeOptions::default(), LONG_TTL, CancellationToken::new());
    assert!(poll_once(&mut second).await.is_pending(), "the second wake waits for the one in flight");

    // A waiter that gives up returns Cancelled without cancelling the wake in flight.
    let impatient = CancellationToken::new();
    let mut third = w3.wake(&t3, WakeOptions::default(), TTL, impatient.clone());
    assert!(poll_once(&mut third).await.is_pending(), "so does the third");
    impatient.cancel();
    assert_eq!(third.await.status, WakeStatus::Cancelled);

    hub.release.notify_waiters();
    assert_eq!(first.await.unwrap(), WakeStatus::Success);
    assert_eq!(second.await.status, WakeStatus::Success);
    assert_eq!(hub.flush_count(), 1);
    assert_eq!(r.h.dcg.count("Wake"), 1, "one Dispatcher request, no cancel wake");
    assert_eq!(r.h.dcg.of("Wake")[0].json()["ttl"], 20, "the TTL of the wake in flight");

    // Once finished, a new wake runs again.
    hub.set_partner(Partner::OnWait(1));
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(r.h.dcg.count("Wake"), 2);
}

#[tokio::test]
async fn cancellation_during_the_partner_wait_sends_the_cancel_wake() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::Block);
    let w = waker(&r, &hub);

    let token = CancellationToken::new();
    let (w1, t1, tok) = (w.clone(), r.target.clone(), token.clone());
    let task = tokio::spawn(async move { w1.wake(&t1, WakeOptions::default(), TTL, tok).await.status });
    assert!(eventually(Duration::from_secs(5), || hub.waits().len() == 1).await);

    token.cancel();
    assert_eq!(task.await.unwrap(), WakeStatus::Cancelled);

    let wakes = r.h.dcg.of("Wake");
    assert_eq!(wakes.len(), 2);
    let cancel = &wakes[1];
    let body = cancel.json();
    assert_eq!(body["ttl"], 1);
    assert_eq!(body["priority"], "high");
    assert_eq!(body["deviceId"], r.phone.id.as_str());
    assert_eq!(body["data"]["DCG-NoOp"], "True");
    assert!(key_order(
        &cancel.body,
        &[
            "DCG-Environment",
            "IgnoreDeviceDisabledStatus",
            "DCG-RequestNewSession",
            "DCG-HubRegion",
            "DCG-NoOp",
            "DCG-CryptoWakeJwt"
        ]
    ));

    let j = jwt::decode(body["data"]["DCG-CryptoWakeJwt"].as_str().unwrap());
    assert!(j.str("Data").ends_with(r#","DCG-NoOp":"True"}"#), "{}", j.str("Data"));

    // Cancelled before anything was sent: nothing reaches the Dispatcher.
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(w.wake(&r.target, WakeOptions::default(), TTL, token).await.status, WakeStatus::Cancelled);
    assert_eq!(r.h.dcg.count("Wake"), 2);
}

#[tokio::test]
async fn circuit_breaker_opens_on_unreachable_results_and_half_opens() {
    let r = ready(|c| c.auth.wake.breaker_break = Duration::from_millis(300)).await;
    let hub = FakeWakeHub::new(Partner::Never);
    let w = waker(&r, &hub);

    for _ in 0..5 {
        assert_eq!(wake(&w, &r.target).await, WakeStatus::DeviceUnreachable);
    }

    assert_eq!(r.h.dcg.count("Wake"), 15, "15 unreachable results at 100 %");
    assert_eq!(wake(&w, &r.target).await, WakeStatus::WakeBrokenCircuitError);
    assert_eq!(r.h.dcg.count("Wake"), 15, "an open circuit sends nothing");

    // After the break one trial is let through; it fails, so the circuit opens again. A wake while it
    // is open sends nothing, so the first wake that reaches the Dispatcher is the trial.
    let (waker, target, dcg) = (&w, &r.target, &r.h.dcg);
    let trial = |sent: usize| {
        retry_until(move || async move {
            let status = wake(waker, target).await;
            (dcg.count("Wake") > sent).then_some(status)
        })
    };

    assert_eq!(trial(15).await, Some(WakeStatus::WakeBrokenCircuitError));
    assert_eq!(r.h.dcg.count("Wake"), 16);

    // A successful trial closes it.
    hub.set_partner(Partner::OnWait(1));
    assert_eq!(trial(16).await, Some(WakeStatus::Success));
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(r.h.dcg.count("Wake"), 18);
}

/// Session unlock and power resume reset every wake circuit breaker: an open circuit lets the next
/// wake through at once.
#[tokio::test]
async fn resetting_the_breakers_closes_an_open_circuit() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::Never);
    let w = waker(&r, &hub);

    for _ in 0..5 {
        assert_eq!(wake(&w, &r.target).await, WakeStatus::DeviceUnreachable);
    }
    assert_eq!(wake(&w, &r.target).await, WakeStatus::WakeBrokenCircuitError);

    w.reset_breakers();
    hub.set_partner(Partner::OnWait(1));
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success, "no break to wait out");
    assert_eq!(r.h.dcg.count("Wake"), 16);
}

#[tokio::test]
async fn unreachable_results_count_only_while_online() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::Never);
    let w = waker(&r, &hub);
    r.h.reachability.set(false);
    for _ in 0..6 {
        assert_eq!(wake(&w, &r.target).await, WakeStatus::DeviceUnreachable);
    }
    assert_eq!(r.h.dcg.count("Wake"), 18, "the breaker stayed closed");
}

#[tokio::test]
async fn flush_and_region_failures() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    for (flush, online, expected) in [
        (HubSendResult::InternetError, true, WakeStatus::TransportInternetError),
        (HubSendResult::Failure, true, WakeStatus::TransportNetworkError),
        (HubSendResult::NotConnected, true, WakeStatus::TransportNetworkError),
        (HubSendResult::BrokenCircuitError, true, WakeStatus::TransportNetworkError),
        (HubSendResult::NotConnected, false, WakeStatus::TransportInternetError),
        (HubSendResult::Failure, false, WakeStatus::TransportInternetError),
    ] {
        *hub.flush_result.lock().unwrap() = flush;
        r.h.reachability.set(online);
        assert_eq!(wake(&w, &r.target).await, expected, "{flush:?} online={online}");
    }

    r.h.reachability.set(true);
    *hub.flush_result.lock().unwrap() = HubSendResult::Success;
    *hub.region.lock().unwrap() = Err(HubRegionTimeout);
    assert_eq!(wake(&w, &r.target).await, WakeStatus::TransportNetworkError, "no OnConnected within 10 s");
    assert_eq!(r.h.dcg.count("Wake"), 0);
    assert!(hub.waits().is_empty());
}

#[tokio::test]
async fn dispatcher_errors_map_to_wake_statuses_without_retry() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    let rows = [
        (404, String::new(), WakeStatus::DeviceNotFoundError),
        (400, error_body(Some(10), "UnsupportedMarket"), WakeStatus::IncompatibleRegionError),
        (403, error_body(Some(7), "LtwDisabled"), WakeStatus::LtwDisabled),
        (401, error_body(Some(1), "InvalidAuthHeader"), WakeStatus::Unauthorized),
        (401, String::new(), WakeStatus::Unauthorized),
        (400, error_body(Some(2), "InvalidRequest"), WakeStatus::Failure),
        (403, error_body(Some(13), "Unlinked"), WakeStatus::Failure),
        (429, String::new(), WakeStatus::Failure),
        (500, String::new(), WakeStatus::Failure),
    ];

    for (i, (http, body, expected)) in rows.into_iter().enumerate() {
        r.h.dcg.script("Wake", http, &body);
        assert_eq!(wake(&w, &r.target).await, expected, "HTTP {http} {body}");
        assert_eq!(r.h.dcg.count("Wake"), i + 1, "no retry on {expected:?}");
    }
    assert!(hub.waits().is_empty(), "no partner wait after a failed request");

    // Token failures: the MSA account needs the user once the services token expired.
    r.h.advance(Duration::from_secs(25 * 3600));
    r.service.msa().sign_out().unwrap();

    // With the session gone and no usable web view, the token needs the user.
    r.h.web.queue(Err(phonelink_engine::edge::WebSignInError::Failed { reason: "no view".into() }));
    assert_eq!(wake(&w, &r.target).await, WakeStatus::MsaTokenUserInteractionRequired);
}

#[tokio::test]
async fn untrusted_targets_are_never_woken() {
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    let stranger = DcgClient::new("5c8e51c2-0000-4000-8000-00000000000a", Environment::Prod);
    assert_eq!(wake(&w, &stranger).await, WakeStatus::Failure);

    let wrong_env = DcgClient::new(r.phone.id.clone(), Environment::Beta);
    assert_eq!(wake(&w, &wrong_env).await, WakeStatus::Failure);

    assert_eq!(hub.flush_count(), 0);
    assert_eq!(r.h.dcg.count("Wake"), 0);
}

#[tokio::test]
async fn an_msa_token_failure_without_connectivity_is_an_internet_error() {
    let h = Harness::new().await;
    let (service, _, phone, _) = h.with_phone().await;
    drop(service);

    // A restart without network: no token is cached and the MSA token exchange cannot connect.
    h.reachability.set(false);
    let service = h.service();

    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = service.waker(hub.clone());
    let target = DcgClient::new(phone.id.clone(), Environment::Prod);

    h.dcg.take_requests();
    assert_eq!(wake(&w, &target).await, WakeStatus::InternetError);
    assert!(h.dcg.requests().is_empty(), "no SignIn and no Dispatcher/Wake");
}

// ------------------------------------------------------------------------------------------------
// Transport failures of the Dispatcher call and their status mapping, against local sockets
// ------------------------------------------------------------------------------------------------

/// What a one-connection server does once it accepted.
#[derive(Clone, Copy)]
enum Misbehaving {
    /// Reads the request, then closes with RST.
    Reset,
    /// Answers bytes that are no TLS record.
    NotTls,
    /// Reads the request and never answers.
    Silent,
}

async fn misbehaving(what: Misbehaving) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];

        match what {
            Misbehaving::Reset => {
                let _ = socket.read(&mut buf).await;
                socket.set_zero_linger().unwrap();
            }
            Misbehaving::NotTls => {
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            }
            Misbehaving::Silent => {
                let _ = socket.read(&mut buf).await;
                std::future::pending::<()>().await;
            }
        }
    });
    addr
}

/// The DCG REST client with `base` as the Prod base and `timeout` per request; the path monitor
/// reports connectivity.
fn rest_at(h: &Harness, base: String, timeout: Duration) -> DcgRest {
    let mut endpoints = h.config.endpoints.clone();
    endpoints.prod_base = base;
    let ident = Arc::new(ClientIdentification::load_or_create(&h.config, &*h.store).unwrap());
    let tap: Tap = h.tap.clone();
    let http = net::http_client(timeout).unwrap();
    DcgRest::new(http, endpoints, "FirstPartyAppsRepo", ident, h.reachability.clone(), tap)
        .with_policy(h.config.auth.rest.clone())
}

async fn dispatcher_failure(rest: &DcgRest) -> RestError {
    let mut data = OrderedStringMap::default();
    data.push("DCG-Environment", "Prod");

    let body = WakeBody {
        device_id: "phone".into(),
        ttl: 20,
        priority: "normal".into(),
        collapse_key: "YPPWake".into(),
        data,
    };

    rest.wake(Environment::Prod, "svc-token", &body, &TraceContext::new_root(Vec::new())).await.unwrap_err()
}

#[tokio::test]
async fn transport_failures_of_the_dispatcher_call_are_internet_errors_or_cancelled() {
    let h = Harness::new().await;
    assert!(h.reachability.is_internet_available(), "the path is reported satisfied throughout");

    let long = Duration::from_secs(10);
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
    let cases = [
        ("a host name that does not resolve", "http://dcg.invalid/".to_string(), long),
        ("a refused connection", format!("http://{closed}/"), long),
        ("a reset after the request", format!("http://{}/", misbehaving(Misbehaving::Reset).await), long),
        ("a failed TLS handshake", format!("https://{}/", misbehaving(Misbehaving::NotTls).await), long),
    ];

    for (what, base, timeout) in cases {
        let e = dispatcher_failure(&rest_at(&h, base, timeout)).await;
        assert!(matches!(e, RestError::Transport { cause: TransportCause::Internet, .. }), "{what}: {e}");
        assert_eq!(dispatcher_status(&e), WakeStatus::InternetError, "{what}: retryable, not Failure");

        // The other DCG REST callers' view: the auth layer's internet failure (a services-token
        // refresh opens the hub as `InternetError`).
        assert_matches!(AuthError::from_rest(e), AuthError::InternetUnavailable, "{what}");
    }

    // No answer within the request timeout: `Cancelled`, a cancelled attempt.
    let base = format!("http://{}/", misbehaving(Misbehaving::Silent).await);
    let e = dispatcher_failure(&rest_at(&h, base, Duration::from_millis(300))).await;
    assert!(matches!(e, RestError::Transport { cause: TransportCause::TimedOut, .. }), "{e}");
    assert_eq!(dispatcher_status(&e), WakeStatus::Cancelled);
}

#[tokio::test]
async fn a_services_token_the_auth_manager_cannot_produce_is_unauthorized() {
    // The services token is part of the Dispatcher call. DCG answering the token request with an
    // error is a failure of the auth manager — `Unauthorized`, which the
    // attempts retry — not `Failure`, which would stop them.
    let r = ready(|_| {}).await;
    let hub = FakeWakeHub::new(Partner::OnWait(1));
    let w = waker(&r, &hub);

    r.h.advance(Duration::from_secs(25 * 3600));
    r.h.dcg.script("SignIn", 500, "");
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Unauthorized);
    assert_eq!(r.h.dcg.count("Wake"), 0, "no Dispatcher request without a token");

    // The next wake gets its token and goes out.
    assert_eq!(wake(&w, &r.target).await, WakeStatus::Success);
    assert_eq!(r.h.dcg.count("Wake"), 1);
}
