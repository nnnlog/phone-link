use super::*;

fn item(reliable: bool) -> Outgoing {
    Outgoing { payload_type: 0x23, ssrc: 1, sequence: 7, body: vec![1, 2, 3], reliable }
}

#[test]
fn first_sn_and_retransmission_keep_the_descriptor() {
    let t0 = Instant::now();
    let mut s = Sender::new(t0);
    let mut r = Receiver::new(t0, 100);

    let p = s.send(t0, Some(item(true)), &mut r).unwrap();
    let (h, len) = RcHeader::decode(&p.payload).unwrap();
    assert_eq!(h.data_sn, Some(100));
    assert_eq!(h.log_window, 0xD);
    assert_eq!(&p.payload[len..], &[1, 2, 3]);

    let lost = s.on_timeout(t0 + Duration::from_millis(501)).unwrap();
    assert_eq!(lost.events.len(), 1);

    let again = s.send(t0 + Duration::from_millis(502), None, &mut r).unwrap();
    let (h2, _) = RcHeader::decode(&again.payload).unwrap();
    assert_eq!(h2.data_sn, Some(101), "a retransmission gets a new SN");
    assert_eq!(again.sequence, 7, "and keeps its descriptor sequence");
    assert_eq!(h2.ack_of_acks, Some(101), "SN 100 was given up");
    assert!(h.delay_ack_info.is_some(), "the first rate-controlled packet carries DELAYACKINFO");
    assert!(h2.delay_ack_info.is_none());
}

#[test]
fn acks_clear_outstanding_and_bad_acks_fail() {
    let t0 = Instant::now();
    let mut s = Sender::new(t0);
    let mut r = Receiver::new(t0, 100);

    for _ in 0..3 {
        s.send(t0, Some(item(false)), &mut r);
    }

    let ack = RcHeader {
        log_window: 0xC,
        ack: Some(Ack { sn: 101, received_ts: 10, ack_delay_ms: 0, shift: 0, gaps: vec![5] }),
        ..Default::default()
    };
    let out = s.on_header(t0 + Duration::from_millis(40), &ack).unwrap();
    assert_eq!(out.events.len(), 2);
    assert_eq!(s.outstanding.len(), 1);
    assert!(s.rtt().is_some());

    let bad = RcHeader {
        ack: Some(Ack { sn: 200, received_ts: 0, ack_delay_ms: 0, shift: 0, gaps: vec![] }),
        ..Default::default()
    };
    assert!(s.on_header(t0, &bad).is_err());

    let vector = RcHeader { ack_vector: Some(AckVector::encode(102, &[true], (0, 0))), ..Default::default() };
    assert!(s.on_header(t0, &vector).is_err(), "a vector must start with a missing SN");
}

#[test]
fn window_follows_the_peer() {
    let t0 = Instant::now();
    let mut s = Sender::new(t0);
    assert_eq!(s.window(), (1 << 12) - 2);
    s.on_header(t0, &RcHeader { log_window: 0xF, ..Default::default() }).unwrap();
    assert_eq!(s.window(), (1 << 14) - 2);
}
