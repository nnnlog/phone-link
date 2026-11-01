use super::*;

#[test]
fn the_retry_delay_follows_decorrelated_jitter() {
    let base = Duration::from_secs(1);
    let mut prev = 0.0;

    // r = 0 for the first retry: t = 0, f = 0, no wait.
    assert_eq!(retry_delay(0, base, &mut prev, 0.0), Duration::ZERO);

    // t = 1: f = 2 · tanh(2) = 1.928055…; (1.928055 − 0) / 1.4 s.
    let d = retry_delay(1, base, &mut prev, 0.0);
    assert!((d.as_secs_f64() - 2.0 * 2f64.tanh() / 1.4).abs() < 1e-6, "{d:?}");
    assert_eq!(d.as_nanos() % 100, 0);

    // t = 2.5 after t = 1: the difference of the two.
    let f1 = 2.0 * 2f64.tanh();
    let f2 = 2f64.powf(2.5) * 10f64.sqrt().tanh();
    let d = retry_delay(2, base, &mut prev, 0.5);
    assert!((d.as_secs_f64() - (f2 - f1) / 1.4).abs() < 1e-6, "{d:?}");

    // The tenth retry stays finite and grows: about 2^9.x / 1.4 s.
    let mut prev = 0.0;
    let mut last = Duration::ZERO;
    for n in 0..10 {
        last = retry_delay(n, base, &mut prev, 0.99);
    }
    assert!(last > Duration::from_secs(200) && last < Duration::from_secs(800), "{last:?}");
}
