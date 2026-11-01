use super::*;

fn data(sn: u64) -> RcHeader {
    RcHeader { data_sn: Some(sn as u16), ..Default::default() }
}

#[test]
fn in_order_packets_are_acked_with_gaps() {
    let t0 = Instant::now();
    let mut r = Receiver::new(t0, 100);

    assert_eq!(r.on_packet(t0, &data(100), Some(30)), Verdict::Deliver);
    assert!(!r.ack_due(t0), "one packet waits for the 10 ms floor");
    assert_eq!(r.timeout(), Some(t0 + DELAYED_ACK_FLOOR));

    r.on_packet(t0 + Duration::from_micros(400), &data(101), None);
    assert!(r.ack_due(t0), "two in-order packets before any DELAYACKINFO");

    let mut h = RcHeader::default();
    r.fill(t0 + Duration::from_millis(1), &mut h);
    let ack = h.ack.unwrap();
    assert_eq!(ack.sn, 101);
    assert_eq!(ack.gaps, vec![200], "400 µs >> 1");
    assert_eq!(ack.shift, 1);
    assert_eq!(ack.received_ts, 100, "400 µs in 4 µs units");
    assert_eq!(h.overhead, Some(30));
    assert!(!r.has_feedback());
}

#[test]
fn holes_send_a_vector_and_duplicates_drop() {
    let t0 = Instant::now();
    let mut r = Receiver::new(t0, 10);

    r.on_packet(t0, &data(10), None);
    r.on_packet(t0, &data(12), None);
    assert!(r.ack_due(t0), "a hole is acknowledged at once");

    let mut h = RcHeader::default();
    r.fill(t0, &mut h);
    assert_eq!(h.ack.as_ref().unwrap().sn, 10);

    let mut h = RcHeader::default();
    r.fill(t0, &mut h);
    let v = h.ack_vector.unwrap();
    assert_eq!((v.base_sn, v.states()), (11, vec![false, true]));

    assert_eq!(r.on_packet(t0, &data(12), None), Verdict::Drop);
    r.on_packet(t0, &data(11), None);
    assert_eq!(r.next, 13);

    // A duplicate below next is delivered and re-acknowledged.
    assert_eq!(r.on_packet(t0, &data(10), None), Verdict::Deliver);
    let mut h = RcHeader::default();
    r.fill(t0, &mut h);
    assert_eq!(h.ack.unwrap().sn, 12);
    let mut h = RcHeader::default();
    r.fill(t0, &mut h);
    assert_eq!(h.ack.unwrap().sn, 10, "re-ACK of the duplicate");
}

#[test]
fn ack_of_acks_skips_given_up_packets() {
    let t0 = Instant::now();
    let mut r = Receiver::new(t0, 50);
    r.on_packet(t0, &data(53), None);

    let aoa = RcHeader { ack_of_acks: Some(53), ..Default::default() };
    r.on_packet(t0, &aoa, None);
    assert_eq!(r.next, 54, "50–52 are given up; 53 was received");
    assert!(r.ack_due(t0));
}

#[test]
fn delayed_ack_policy() {
    let t0 = Instant::now();
    let mut r = Receiver::new(t0, 0);
    let info = RcHeader { delay_ack_info: Some((6, 20)), data_sn: Some(0), ..Default::default() };
    r.on_packet(t0, &info, None);
    assert_eq!(r.timeout(), Some(t0 + Duration::from_millis(20)));

    for sn in 1..=6 {
        r.on_packet(t0, &data(sn), None);
    }
    assert!(!r.ack_due(t0), "7 pending is not more than 6 + 1");

    r.on_packet(t0, &data(7), None);
    assert!(r.ack_due(t0));
}
