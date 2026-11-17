use super::*;

#[test]
fn the_back_off_doubles_up_to_a_minute() {
    let mut backoff = Backoff::default();
    let delays: Vec<u64> = (0..8).map(|_| backoff.after_run(Duration::ZERO).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
}

#[test]
fn a_healthy_run_starts_the_back_off_over() {
    let mut backoff = Backoff::default();
    for _ in 0..4 {
        backoff.after_run(Duration::from_secs(3));
    }
    assert_eq!(backoff.after_run(Duration::from_secs(59)), Duration::from_secs(16));
    assert_eq!(backoff.after_run(Duration::from_secs(60)), Duration::from_secs(1));
    assert_eq!(backoff.after_run(Duration::ZERO), Duration::from_secs(2));
}
