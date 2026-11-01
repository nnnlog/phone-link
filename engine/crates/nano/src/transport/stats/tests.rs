use super::*;

#[test]
fn time_window_covers_the_last_window() {
    let mut s = Staggered::time_window(10.0, 2.0);
    for t in 0..=30 {
        s.add(t as f64, t as f64);
    }

    assert!(s.is_ready());

    // The current accumulator started 8–10 s ago.
    let n = s.count();
    assert!((8..=11).contains(&n), "{n}");
    assert!(s.mean() > 20.0);
}

#[test]
fn percentile_keeps_the_smallest() {
    let mut p = Staggered::percentile(0.3);
    for x in [5.0, 1.0, 3.0, 2.0, 4.0, 9.0] {
        p.add(0.0, x);
    }
    assert_eq!(p.percentile_value(), 1.0);

    for _ in 0..20 {
        p.add(0.0, 7.0);
    }
    assert_eq!(p.percentile_value(), 3.0, "index ⌊0.1 × 26⌋ = 2");
}

#[test]
fn count_window_rotates() {
    let mut c = CountWindow::new(32, 40);
    for i in 0..39 {
        c.add(i as f64);
    }
    assert!(!c.is_ready());

    c.add(39.0);
    c.add(40.0);
    assert!(c.is_ready());
    assert!((32..=40).contains(&c.count()));
}

#[test]
fn byte_rate() {
    let mut b = Staggered::byte_rate(2.0);
    b.add(0.0, 0.0);
    b.add(1.0, 1000.0);
    assert_eq!(b.rate(), 1000.0);
}
