use super::*;
use reqwest::header::HeaderValue;

fn headers(retry_after: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    if let Some(v) = retry_after {
        h.insert(RETRY_AFTER, HeaderValue::from_str(v).unwrap());
    }
    h
}

fn at(text: &str) -> SystemTime {
    parse_rfc1123(text).unwrap()
}

#[test]
fn backoff_follows_the_decorrelated_jitter_curve() {
    let base = Duration::from_secs(2);
    let f = |t: f64| 2f64.powf(t) * (4.0 * t).sqrt().tanh();
    let c = 2925.0 / 4095.0 * 2.0;

    // r = 0 on every retry: f(0) = 0, then f(1), f(2) differences.
    let mut prev = 0.0;
    assert_eq!(backoff_delay(0, base, &mut prev, 0.0), Duration::ZERO);
    let d1 = backoff_delay(1, base, &mut prev, 0.0);
    assert!((d1.as_secs_f64() - f(1.0) * c).abs() < 1e-6, "{d1:?}");
    let d2 = backoff_delay(2, base, &mut prev, 0.0);
    assert!((d2.as_secs_f64() - (f(2.0) - f(1.0)) * c).abs() < 1e-6, "{d2:?}");

    // The cumulative wait is f(n + r) · c whatever the earlier draws.
    let mut prev = 0.0;
    let total: Duration =
        [(0, 0.3), (1, 0.9), (2, 0.5)].iter().map(|&(n, r)| backoff_delay(n, base, &mut prev, r)).sum();
    assert!((total.as_secs_f64() - f(2.5) * c).abs() < 1e-6, "{total:?}");

    // The ranges before retries 1, 2, 3.
    for (n, lo, hi) in [(0u32, 0.0, 2.755), (1, 2.754, 5.676), (2, 5.675, 11.41)] {
        for r in [0.0, 0.5, 0.999_999] {
            let mut prev = if n == 0 { 0.0 } else { f(f64::from(n) - 1.0 + 0.999_999) };
            let mut cumulative = prev * c;
            cumulative += backoff_delay(n, base, &mut prev, r).as_secs_f64();
            assert!(cumulative >= lo - 1e-3 && cumulative < hi, "retry {} r {r}: {cumulative}", n + 1);
        }
    }

    assert_eq!(backoff_delay(1, Duration::ZERO, &mut 0.0, 0.5), Duration::ZERO);
}

#[test]
fn rfc1123_dates() {
    let t = at("Sun, 06 Nov 1994 08:49:37 GMT");
    assert_eq!(t.duration_since(UNIX_EPOCH).unwrap().as_secs(), 784_111_777);
    assert_eq!(at("sun, 06 nov 1994 08:49:37 GMT"), t, "names in any case");

    assert!(parse_rfc1123("Mon, 06 Nov 1994 08:49:37 GMT").is_none(), "wrong day name");
    assert!(parse_rfc1123("Sun, 6 Nov 1994 08:49:37 GMT").is_none());
    assert!(parse_rfc1123("Sun, 06 Nov 1994 08:49:37 UTC").is_none());
    assert!(parse_rfc1123("Sunday, 06-Nov-94 08:49:37 GMT").is_none(), "RFC 850");
    assert!(parse_rfc1123("Sun Nov  6 08:49:37 1994").is_none(), "asctime");
    assert!(parse_rfc1123("Thu, 31 Feb 2026 00:00:00 GMT").is_none());
    assert!(parse_rfc1123("Sat, 29 Feb 2025 00:00:00 GMT").is_none());
    assert!(parse_rfc1123("Thu, 29 Feb 2024 23:59:59 GMT").is_some());
    assert!(parse_rfc1123("Thu, 29 Feb 2024 24:00:00 GMT").is_none());
}

#[test]
fn retry_after_of_the_retry() {
    let now = at("Wed, 28 Oct 2026 12:00:00 GMT");

    assert_eq!(retry_after(&headers(None), now), None);
    assert_eq!(retry_after(&headers(Some("7")), now), Some(Duration::from_secs(7)));
    assert_eq!(retry_after(&headers(Some(" 120 ")), now), Some(Duration::from_secs(120)));
    assert_eq!(retry_after(&headers(Some("-1")), now), None);
    assert_eq!(retry_after(&headers(Some("1.5")), now), None);
    assert_eq!(retry_after(&headers(Some("99999999999")), now), None);
    assert_eq!(retry_after(&headers(Some("Wed, 28 Oct 2026 12:00:42 GMT")), now), Some(Duration::from_secs(42)));
    assert_eq!(retry_after(&headers(Some("Wed, 28 Oct 2026 11:00:00 GMT")), now), Some(Duration::ZERO));
    assert_eq!(retry_after(&headers(Some("soon")), now), None);

    let mut two = headers(Some("1"));
    two.append(RETRY_AFTER, HeaderValue::from_static("2"));
    assert_eq!(retry_after(&two, now), None);
}

#[test]
fn resend_waits() {
    let now = at("Wed, 28 Oct 2026 12:00:00 GMT");
    let d = Duration::from_secs(3);
    let max = Duration::from_secs(180);
    let wait = |h: Option<&str>, k| resend_wait(&headers(h), k, d, max, now);

    // Without Retry-After: 3, 6, 12 s.
    assert_eq!(wait(None, 0), Ok(Duration::from_secs(3)));
    assert_eq!(wait(None, 1), Ok(Duration::from_secs(6)));
    assert_eq!(wait(None, 2), Ok(Duration::from_secs(12)));

    // An integer, capped at 180 s; a sign and white space are allowed; negative is an error.
    assert_eq!(wait(Some("0"), 2), Ok(Duration::ZERO));
    assert_eq!(wait(Some("+5"), 0), Ok(Duration::from_secs(5)));
    assert_eq!(wait(Some("600"), 0), Ok(max));
    assert_eq!(wait(Some("-2"), 0), Err("-2".to_string()));
    assert_eq!(wait(Some("-0"), 1), Ok(Duration::ZERO));

    // A date: its seconds component when positive, else the exponential wait.
    assert_eq!(wait(Some("Wed, 28 Oct 2026 12:00:20 GMT"), 1), Ok(Duration::from_secs(20)));
    assert_eq!(wait(Some("Wed, 28 Oct 2026 12:01:30 GMT"), 0), Ok(Duration::from_secs(30)));
    assert_eq!(wait(Some("Wed, 28 Oct 2026 12:01:00 GMT"), 1), Ok(Duration::from_secs(6)), "a whole minute");
    assert_eq!(wait(Some("Wed, 28 Oct 2026 11:59:50 GMT"), 2), Ok(Duration::from_secs(12)), "past");

    // Any other form: the base delay.
    assert_eq!(wait(Some("1.5"), 2), Ok(d));
    assert_eq!(wait(Some("Wed, 28-Oct-26 12:00:20 GMT"), 2), Ok(d));

    // Truncated seconds of a fractional difference.
    let later = now + Duration::from_millis(400);
    assert_eq!(
        resend_wait(&headers(Some("Wed, 28 Oct 2026 12:00:05 GMT")), 0, d, max, later),
        Ok(Duration::from_secs(4))
    );
}

fn cfg() -> DcgRestConfig {
    DcgRestConfig {
        breaker_minimum_throughput: 4,
        breaker_failure_ratio: 0.5,
        breaker_sampling: Duration::from_secs(30),
        breaker_break: Duration::from_secs(5),
        ..DcgRestConfig::default()
    }
}

#[tokio::test(start_paused = true)]
async fn breaker_opens_on_the_failure_ratio_over_the_minimum_throughput() {
    let mut b = Breaker::new(&cfg(), Instant::now());
    let t0 = Instant::now();

    b.on_outcome(false, t0);
    b.on_outcome(true, t0);
    b.on_outcome(true, t0);
    assert_eq!(b.state, CircuitState::Closed, "3 samples, under the minimum");
    b.on_outcome(false, t0);
    assert_eq!(b.state, CircuitState::Closed, "a success never opens");
    b.on_outcome(true, t0);
    assert_eq!(b.state, CircuitState::Open, "5 samples, 3 failed");
    assert_eq!(b.before_attempt(t0 + Duration::from_secs(2)), Err(Duration::from_secs(3)));

    // The break is over: one trial; the others wait for it.
    let t1 = t0 + Duration::from_secs(5);
    assert_eq!(b.before_attempt(t1), Ok(()));
    assert_eq!(b.state, CircuitState::HalfOpen);
    assert_eq!(b.before_attempt(t1), Err(Duration::from_secs(5)));
    assert_eq!(b.before_attempt(t1 + Duration::from_secs(9)), Err(Duration::ZERO), "still waiting for the trial");

    // A failed trial re-opens; a successful one closes and clears the samples.
    b.on_outcome(true, t1);
    assert_eq!(b.state, CircuitState::Open);
    let t2 = t1 + Duration::from_secs(5);
    assert_eq!(b.before_attempt(t2), Ok(()));
    b.on_outcome(false, t2);
    assert_eq!(b.state, CircuitState::Closed);

    for _ in 0..3 {
        b.on_outcome(true, t2);
    }
    assert_eq!(b.state, CircuitState::Closed, "the samples were cleared: 3 under the minimum");
    b.on_outcome(true, t2);
    assert_eq!(b.state, CircuitState::Open);
}

#[tokio::test(start_paused = true)]
async fn breaker_samples_age_out_by_bucket() {
    let mut b = Breaker::new(&cfg(), Instant::now());
    let t0 = Instant::now();

    for _ in 0..3 {
        b.on_outcome(true, t0);
    }

    // 30 s later the first bucket is gone: one failure in the window, under the minimum.
    let t1 = t0 + Duration::from_secs(30);
    b.on_outcome(true, t1);
    assert_eq!(b.state, CircuitState::Closed);

    // Within 30 s they add up.
    let t2 = t1 + Duration::from_secs(29);
    for _ in 0..2 {
        b.on_outcome(true, t2);
    }
    assert_eq!(b.state, CircuitState::Closed, "3 failures in the window");
    b.on_outcome(true, t2);
    assert_eq!(b.state, CircuitState::Open, "4 failures in buckets started 29 s and 0 s ago");
}

#[test]
#[should_panic(expected = "attempt timeout exceeds the total timeout")]
fn a_policy_that_cannot_run_fails_loudly() {
    RestPolicy::new(DcgRestConfig { attempt_timeout: Some(Duration::from_secs(40)), ..DcgRestConfig::default() });
}
