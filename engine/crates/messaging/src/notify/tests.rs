use super::*;

#[test]
fn recency_window() {
    let five_min = 5 * 60 * 1000;
    assert!(recent(1_000_000, 1_000_000 - five_min, five_min));
    assert!(!recent(1_000_000, 1_000_000 - five_min - 1, five_min));
    assert!(recent(1_000_000, 2_000_000, five_min), "a future timestamp is recent");
}
