use super::*;
use crate::auth::dcg_rest::ServiceFailure;
use crate::auth::msa::MsaError;

fn cfg() -> WakeConfig {
    WakeConfig { breaker_minimum_throughput: 3, ..WakeConfig::default() }
}

fn transport(cause: TransportCause) -> RestError {
    RestError::Transport { call: "Wake", cause, detail: String::new() }
}

fn reply(http_status: u16, status: ServiceRequestStatus) -> RestError {
    RestError::Status(Box::new(ServiceFailure {
        call: "SignIn",
        http_status,
        status,
        error: None,
        body: String::new(),
    }))
}

#[test]
fn the_dispatcher_calls_transport_failures_are_internet_errors_or_cancelled() {
    assert_eq!(dispatcher_status(&transport(TransportCause::Internet)), WakeStatus::InternetError);
    assert_eq!(dispatcher_status(&transport(TransportCause::TimedOut)), WakeStatus::Cancelled);
    assert_eq!(dispatcher_status(&transport(TransportCause::Other)), WakeStatus::Failure);

    let open =
        RestError::Policy { call: "Wake", failure: PolicyFailure::BrokenCircuit { retry_after: Duration::ZERO } };
    assert_eq!(open.status(), Some(ServiceRequestStatus::BrokenCircuit));
    assert_eq!(dispatcher_status(&open), WakeStatus::Failure);
}

#[test]
fn services_token_failures_map_as_the_dispatcher_calls_auth_failures() {
    assert_eq!(
        token_status(&AuthError::UserInteractionRequired("x".into())),
        WakeStatus::MsaTokenUserInteractionRequired
    );
    assert_eq!(token_status(&AuthError::InternetUnavailable), WakeStatus::InternetError);
    assert_eq!(token_status(&AuthError::Msa(MsaError::Internet)), WakeStatus::InternetError);

    // The DCG auth layer's failures → `Unauthorized`.
    for e in [
        AuthError::Unauthorized("401".into()),
        AuthError::from_rest(reply(500, ServiceRequestStatus::ServiceError)),
        AuthError::from_rest(reply(429, ServiceRequestStatus::TooManyRequests)),
        AuthError::from_rest(transport(TransportCause::TimedOut)),
        AuthError::from_rest(RestError::Policy {
            call: "SignIn",
            failure: PolicyFailure::BrokenCircuit { retry_after: Duration::from_secs(1) },
        }),
        AuthError::InvalidIdentity,
        AuthError::UnlinkedDevice,
        AuthError::NotProvisioned("no identity".into()),
    ] {
        assert_eq!(token_status(&e), WakeStatus::Unauthorized, "{e:?}");
    }

    // A DCG REST failure with an internet cause is the auth layer's internet failure.
    assert_eq!(token_status(&AuthError::from_rest(transport(TransportCause::Internet))), WakeStatus::InternetError);

    // Everything else stops the attempts.
    for e in [
        AuthError::from_rest(transport(TransportCause::Other)),
        AuthError::from_rest(RestError::InvalidResponse { call: "SignIn", detail: "no accessToken".into() }),
        AuthError::from_rest(RestError::Policy {
            call: "SignIn",
            failure: PolicyFailure::ResendsExhausted { status: 503, sends: 4 },
        }),
        AuthError::Msa(MsaError::Endpoint("HTTP 500".into())),
        AuthError::Store("keychain".into()),
        AuthError::Protocol("deviceId echo".into()),
        AuthError::Config("bad base URL".into()),
    ] {
        assert_eq!(token_status(&e), WakeStatus::Failure, "{e:?}");
    }
}

#[test]
fn breaker_opens_at_full_failure_with_throughput_and_half_opens() {
    let cfg = cfg();
    let mut b = Breaker::new();
    let t0 = Instant::now();

    for i in 0..2 {
        assert!(b.allow(t0));
        b.record(t0 + Duration::from_secs(i), true, &cfg);
    }
    assert_eq!(b.state, BreakerState::Closed, "below the minimum throughput");
    b.record(t0 + Duration::from_secs(2), true, &cfg);
    assert!(matches!(b.state, BreakerState::Open { .. }));
    assert!(!b.allow(t0 + Duration::from_secs(3)));

    let after = t0 + Duration::from_secs(2) + cfg.breaker_break;
    assert!(b.allow(after), "half-open trial");
    assert!(!b.allow(after), "only one trial");

    b.record(after, false, &cfg);
    assert_eq!(b.state, BreakerState::Closed);
}

#[test]
fn breaker_needs_every_result_to_fail_within_the_window() {
    let cfg = cfg();
    let mut b = Breaker::new();
    let t0 = Instant::now();

    b.record(t0, false, &cfg);
    b.record(t0, true, &cfg);
    b.record(t0, true, &cfg);
    assert_eq!(b.state, BreakerState::Closed);

    let later = t0 + cfg.breaker_sampling_window + Duration::from_secs(1);
    b.record(later, true, &cfg);
    assert_eq!(b.state, BreakerState::Closed, "old results left the window");

    b.record(later, true, &cfg);
    b.record(later, true, &cfg);
    assert!(matches!(b.state, BreakerState::Open { .. }));
}
