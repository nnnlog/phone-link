use super::*;

#[test]
fn initial_values() {
    let mut c = Urcpv2::new(0.0);
    assert_eq!(c.max_rate(), 64_000.0);
    assert_eq!(c.max_delayed_acks(), 1, "d = 1368/64000 ≈ 21 ms ⇒ p = 3d");

    let first = c.allowance(0.0, 1000);
    assert_eq!(first, 100.0 * MTU - 13_600.0, "the start-up allowance");
}

#[test]
fn trains_set_a_capacity_and_start() {
    let mut c = Urcpv2::new(0.0);

    // 1 MB/s path: 1000-byte packets sent back to back arrive 1 ms apart.
    let mut events = Vec::new();
    for i in 0..400u64 {
        let send = i as f64 * 0.0002;
        let recv = 0.05 + i as f64 * 0.001;
        events.push(PacketEvent {
            sn: 100 + i,
            send_time: send,
            size: 1000,
            in_flight_at_send: 0,
            fate: Fate::Acked { peer_receive: Some(recv), rtt: Some(0.1), ack_time: recv + 0.05 },
        });
    }

    c.on_batch(3.0, &events);
    assert!(c.started);
    assert!(c.max_rate() >= 64_000.0);
    assert!(c.rate() > 0.0 && c.rate() <= c.max_rate());
}

#[test]
fn reference_timestamps() {
    let mut r = RefTimestamps::default();

    // Peer clock 100 s ahead; true one-way delay 10 ms.
    r.add(1.0, 0.03, 101.0 + 0.01);
    r.add(1.5, 0.02, 101.5 + 0.01);

    assert_eq!(r.min_delay(), Some(0.01));
    let owd = r.one_way_delay(2.0, 102.0 + 0.01).unwrap();
    assert!((owd - 0.01).abs() < 1e-9, "{owd}");
}

#[test]
fn loss_intervals() {
    let mut l = LossEstimator::new();
    l.current = 100.0;
    l.close_interval(0.1);
    assert_eq!(l.history, vec![100.0]);
    assert!(l.x > 0.0);

    l.current = 20.0;
    l.close_interval(0.1);
    assert!(l.history.is_empty(), "3n < last clears the history");

    l.record(10.0);
    l.current = 50.0;
    l.close_interval(0.1);
    assert_eq!(l.aside, Some(50.0), "a much longer interval waits for the next");
}
