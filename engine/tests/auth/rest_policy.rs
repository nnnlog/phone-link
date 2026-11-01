//! The resilience policy of the DCG REST calls against the fake DCG server: retries of transient
//! attempts, resends of 503/504/429 with `Retry-Attempt`, `Retry-After`, the timeouts, the circuit
//! breaker and the concurrency limit, per service client.

use crate::support::auth_world;
use crate::support::common::retry_until;

use std::sync::Arc;
use std::time::{Duration, Instant};

use auth_world::fake_dcg::error_body;
use auth_world::harness::{Harness, fast_policy};
use phonelink_engine::auth::dcg_rest::{DcgRest, PolicyFailure, RestError, ServiceRequestStatus, TransportCause};
use phonelink_engine::auth::rest_policy::{CircuitState, ServiceClient};
use phonelink_engine::config::DcgRestConfig;
use phonelink_engine::tap::{Tap, TapLayer};
use phonelink_engine::types::{Environment, TraceContext};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;

fn rest_with(h: &Harness, base: Option<String>, policy: DcgRestConfig) -> DcgRest {
    let mut endpoints = h.config.endpoints.clone();
    if let Some(base) = base {
        endpoints.prod_base = base;
    }

    let ident =
        Arc::new(phonelink_engine::identification::ClientIdentification::load_or_create(&h.config, &*h.store).unwrap());
    let tap: Tap = h.tap.clone();
    DcgRest::new(DcgRest::http_client().unwrap(), endpoints, "FirstPartyAppsRepo", ident, h.reachability.clone(), tap)
        .with_policy(policy)
}

fn rest(h: &Harness) -> DcgRest {
    rest_with(h, None, fast_policy())
}

fn trace() -> TraceContext {
    TraceContext::new_root(Vec::new())
}

async fn account_cert(rest: &DcgRest) -> Result<Option<String>, RestError> {
    rest.account_cert(Environment::Prod, "msa", &trace()).await
}

/// The `Retry-Attempt` header of every `op` request so far.
fn retry_attempts(h: &Harness, op: &str) -> Vec<Option<String>> {
    h.dcg.of(op).iter().map(|r| r.header("Retry-Attempt").map(str::to_string)).collect()
}

/// A server that accepts connections and never answers; counts them.
async fn silent() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = accepted.clone();

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while socket.read(&mut buf).await.is_ok_and(|n| n > 0) {}
                std::future::pending::<()>().await;
            });
        }
    });

    (format!("http://{addr}/"), accepted)
}

#[tokio::test]
async fn a_transient_reply_is_retried_with_the_same_request() {
    let h = Harness::new().await;
    let rest = rest(&h);

    h.dcg.script("GetAccountCert", 500, "");
    h.dcg.script("GetAccountCert", 502, "");

    assert!(account_cert(&rest).await.unwrap().is_some(), "the third attempt succeeds");
    let sent = h.dcg.of("GetAccountCert");
    assert_eq!(sent.len(), 3);
    assert!(sent.iter().all(|r| r.headers == sent[0].headers && r.line() == sent[0].line()), "identical requests");
    assert_eq!(retry_attempts(&h, "GetAccountCert"), [None, None, None], "a retry is not a resend");

    // The tap shows one event for the call, with its sends.
    let events: Vec<_> = h.tap.take().into_iter().filter(|e| e.layer == TapLayer::Rest).collect();
    assert_eq!(events.len(), 1);
    let sends = events[0].summary["sends"].as_array().unwrap();
    let statuses: Vec<_> = sends.iter().map(|s| s["status"].as_u64()).collect();
    assert_eq!(statuses, [Some(500), Some(502), Some(200)]);
    assert_eq!(events[0].summary["status"], 200);
}

#[tokio::test]
async fn a_persistent_transient_reply_is_sent_four_times_and_mapped() {
    let h = Harness::new().await;
    let rest = rest(&h);
    for (http, expected) in [(500, ServiceRequestStatus::ServiceError), (408, ServiceRequestStatus::ClientError)] {
        h.dcg.take_requests();
        h.dcg.script_times("GetAccountCert", 4, http, &error_body(Some(3), "Busy"));
        let e = account_cert(&rest).await.unwrap_err();
        assert_eq!(e.status(), Some(expected), "HTTP {http}: {e}");
        assert_eq!(h.dcg.count("GetAccountCert"), 4, "HTTP {http}: 1 attempt + 3 retries");
    }
}

#[tokio::test]
async fn other_replies_are_not_retried() {
    let h = Harness::new().await;
    let rest = rest(&h);
    for http in [400, 401, 403, 404, 409] {
        h.dcg.take_requests();
        h.dcg.script("GetAccountCert", http, "");
        assert!(account_cert(&rest).await.is_err());
        assert_eq!(h.dcg.count("GetAccountCert"), 1, "HTTP {http}");
    }
}

#[tokio::test]
async fn retry_after_replaces_the_back_off() {
    let h = Harness::new().await;
    let rest = rest(&h);
    h.dcg.script_retry_after("GetAccountCert", 500, "1");
    let started = Instant::now();
    account_cert(&rest).await.unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1), "{:?}", started.elapsed());
    assert_eq!(h.dcg.count("GetAccountCert"), 2);
}

#[tokio::test]
async fn busy_replies_are_resent_inside_the_attempt_with_retry_attempt() {
    let h = Harness::new().await;
    let rest = rest(&h);

    h.dcg.script("GetAccountCert", 503, "");
    h.dcg.script("GetAccountCert", 504, "");
    h.dcg.script("GetAccountCert", 429, "");

    assert!(account_cert(&rest).await.unwrap().is_some());
    assert_eq!(retry_attempts(&h, "GetAccountCert"), [None, Some("1".into()), Some("2".into()), Some("3".into())]);
    let sends = h.tap.take().into_iter().find(|e| e.layer == TapLayer::Rest).unwrap().summary["sends"].clone();
    let attempts: Vec<_> = sends.as_array().unwrap().iter().map(|s| s["attempt"].as_u64().unwrap()).collect();
    assert_eq!(attempts, [0, 0, 0, 0], "all in the first attempt");

    // A resend answered otherwise than 503/504/429 ends the resends with that reply, which the retry
    // then handles as any reply.
    h.dcg.take_requests();
    h.dcg.script("GetAccountCert", 503, "");
    h.dcg.script("GetAccountCert", 500, "");
    assert!(account_cert(&rest).await.unwrap().is_some());
    assert_eq!(retry_attempts(&h, "GetAccountCert"), [None, Some("1".into()), None]);
}

#[tokio::test]
async fn busy_replies_after_every_resend_end_the_call_at_once() {
    let h = Harness::new().await;
    let rest = rest(&h);

    h.dcg.script_times("GetAccountCert", 4, 429, "");
    let e = account_cert(&rest).await.unwrap_err();
    assert_eq!(
        e,
        RestError::Policy {
            call: "GetAccountCert",
            failure: PolicyFailure::ResendsExhausted { status: 429, sends: 4 }
        }
    );
    assert_eq!(e.status(), None, "no status: an unexpected failure");
    assert_eq!(h.dcg.count("GetAccountCert"), 4, "not retried");
    assert_eq!(rest.policy().circuit_state(ServiceClient::DeviceAuthProxy), CircuitState::Closed);

    // A negative Retry-After ends it before any resend.
    h.dcg.take_requests();
    h.dcg.script_retry_after("GetAccountCert", 503, "-1");
    let e = account_cert(&rest).await.unwrap_err();
    assert!(
        matches!(&e, RestError::Policy { failure: PolicyFailure::NegativeRetryAfter { value }, .. } if value == "-1"),
        "{e:?}"
    );
    assert_eq!(h.dcg.count("GetAccountCert"), 1);

    // Retry-After: 0 resends at once.
    h.dcg.take_requests();
    h.dcg.script_retry_after("GetAccountCert", 503, "0");
    account_cert(&rest).await.unwrap();
    assert_eq!(retry_attempts(&h, "GetAccountCert"), [None, Some("1".into())]);
}

#[tokio::test]
async fn persistent_busy_replies_without_retry_after_end_as_a_timeout() {
    let h = Harness::new().await;
    // Resend waits 100, 200, 400 ms: sends at 0, 100 and 300 ms of each attempt, which the 350 ms
    // attempt timeout then cuts; the 1.2 s total timeout cuts the call during the fourth attempt.
    let policy = DcgRestConfig {
        resend_delay: Duration::from_millis(100),
        attempt_timeout: Some(Duration::from_millis(350)),
        total_timeout: Duration::from_millis(1200),
        retry_base_delay: Duration::from_millis(1),
        ..fast_policy()
    };

    let rest = rest_with(&h, None, policy);
    h.dcg.script_times("GetAccountCert", 50, 503, "");
    let started = Instant::now();
    let e = account_cert(&rest).await.unwrap_err();
    let took = started.elapsed();

    assert!(matches!(e, RestError::Transport { cause: TransportCause::TimedOut, .. }), "{e:?}");
    assert_eq!(e.status(), Some(ServiceRequestStatus::Cancelled));
    assert!(took >= Duration::from_millis(1200) && took < Duration::from_millis(2200), "{took:?}");

    let sent = retry_attempts(&h, "GetAccountCert");
    assert_eq!(&sent[..3], [None, Some("1".to_string()), Some("2".to_string())], "{sent:?}");
    assert!(!sent.contains(&Some("3".to_string())), "the third resend never goes out: {sent:?}");
    assert!(sent.len() >= 9, "three attempts of three sends at least: {sent:?}");
    let attempts = sent.iter().filter(|s| s.is_none()).count();
    assert!((3..=4).contains(&attempts), "at most four attempts: {sent:?}");
}

#[tokio::test]
async fn no_reply_is_retried_and_then_classified() {
    let h = Harness::new().await;
    let (base, accepted) = silent().await;
    let policy = DcgRestConfig {
        attempt_timeout: Some(Duration::from_millis(200)),
        total_timeout: Duration::from_secs(5),
        ..fast_policy()
    };

    let rest = rest_with(&h, Some(base), policy);
    let e = account_cert(&rest).await.unwrap_err();
    assert!(matches!(e, RestError::Transport { cause: TransportCause::TimedOut, .. }), "{e:?}");
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 4, "4 attempts, each timed out");

    // A refused connection: 4 attempts, then InternetError.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let rest = rest_with(&h, Some(format!("http://{closed}/")), fast_policy());
    h.tap.take();
    let e = account_cert(&rest).await.unwrap_err();
    assert_eq!(e.status(), Some(ServiceRequestStatus::InternetError), "{e:?}");
    let sends = h.tap.take().into_iter().find(|e| e.layer == TapLayer::Rest).unwrap().summary["sends"].clone();
    assert_eq!(sends.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn the_circuit_opens_per_service_client_and_half_opens_after_the_break() {
    let h = Harness::new().await;
    let policy = DcgRestConfig {
        retries: 0,
        breaker_minimum_throughput: 2,
        breaker_failure_ratio: 0.5,
        breaker_break: Duration::from_millis(300),
        ..fast_policy()
    };

    let rest = rest_with(&h, None, policy);
    h.dcg.script_times("GetAccountCert", 2, 500, "");
    for _ in 0..2 {
        assert_eq!(account_cert(&rest).await.unwrap_err().status(), Some(ServiceRequestStatus::ServiceError));
    }
    assert_eq!(rest.policy().circuit_state(ServiceClient::DeviceAuthProxy), CircuitState::Open);

    // Open: refused without a request.
    let e = account_cert(&rest).await.unwrap_err();
    assert!(matches!(e, RestError::Policy { failure: PolicyFailure::BrokenCircuit { .. }, .. }), "{e:?}");
    assert_eq!(e.status(), Some(ServiceRequestStatus::BrokenCircuit));
    assert_eq!(h.dcg.count("GetAccountCert"), 2);

    // Another service client is unaffected; clones share the circuit.
    rest.device_info_list(Environment::Prod, "msa", &trace()).await.unwrap();
    assert!(account_cert(&rest.clone()).await.is_err());

    // After the break one trial goes out; its success closes the circuit. A call while it is open
    // sends nothing.
    let trial = retry_until(|| async {
        match account_cert(&rest).await {
            Err(e) if e.status() == Some(ServiceRequestStatus::BrokenCircuit) => None,
            other => Some(other),
        }
    })
    .await;
    trial.expect("a trial goes out after the break").unwrap();
    assert_eq!(rest.policy().circuit_state(ServiceClient::DeviceAuthProxy), CircuitState::Closed);
    assert_eq!(h.dcg.count("GetAccountCert"), 3);
}

#[tokio::test]
async fn calls_over_the_concurrency_limit_are_refused() {
    let h = Harness::new().await;
    let (base, accepted) = silent().await;
    let policy = DcgRestConfig { concurrency_limit: 1, retries: 0, ..fast_policy() };
    let rest = rest_with(&h, Some(base), policy);

    let first = {
        let rest = rest.clone();
        tokio::spawn(async move { account_cert(&rest).await })
    };
    assert!(
        auth_world::harness::eventually(Duration::from_secs(2), || accepted.load(std::sync::atomic::Ordering::SeqCst)
            == 1)
        .await
    );

    let e = account_cert(&rest).await.unwrap_err();
    assert_eq!(e, RestError::Policy { call: "GetAccountCert", failure: PolicyFailure::ConcurrencyLimit { limit: 1 } });
    assert_eq!(e.status(), None);

    // Another service client has its own count.
    let e = rest.device_info_list(Environment::Prod, "msa", &trace()).await.unwrap_err();
    assert!(matches!(e, RestError::Transport { .. }), "sent, then timed out: {e:?}");

    // The first call ends with its attempt timeout and frees its place.
    assert!(matches!(first.await.unwrap(), Err(RestError::Transport { cause: TransportCause::TimedOut, .. })));
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn provisioning_goes_through_a_transient_failure() {
    let mut h = Harness::new().await;
    h.config.auth.rest = fast_policy();
    h.dcg.script("EnrollDevice", 500, "");
    h.dcg.script("GenerateNonce", 503, "");
    let (_service, _) = h.provisioned().await;
    assert_eq!(h.dcg.count("EnrollDevice"), 2);
    assert_eq!(retry_attempts(&h, "GenerateNonce")[..2], [None, Some("1".to_string())]);
}
