use super::*;
use crate::transport::rc_header::{Ack, flag};

fn packet(payload_type: u8, payload: Vec<u8>) -> RtpPacket {
    RtpPacket { payload_type, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload }
}

/// A transport taken through the probe and rate-control handshakes by a scripted phone.
fn connected(now: Instant) -> Transport {
    let mut t = Transport::new(
        now,
        TransportConfig { lower_max_payload: 1402, lower_errors_close: false, warning_timeout: Duration::from_secs(5) },
    );

    let mut syn = vec![0u8; 1000];
    syn[0] = 1;
    t.handle_packet(now, packet(pt::PROBE, syn), 1030);
    t.handle_packet(now, packet(pt::PROBE, vec![2, 0, 0x7A, 0x05]), 34);

    let mut rc = 0u16.to_le_bytes().to_vec();
    rc.extend_from_slice(&3u16.to_le_bytes());
    rc.push(14);
    rc.extend_from_slice(&100u64.to_le_bytes());
    rc.extend_from_slice(&5u16.to_le_bytes());
    rc.extend_from_slice(&0u16.to_le_bytes());
    t.handle_packet(now, packet(pt::RC_HANDSHAKE, rc), 47);
    t.handle_packet(now, packet(pt::RC_HANDSHAKE, 2u16.to_le_bytes().to_vec()), 32);

    assert!(t.is_connected());
    while t.poll_transmit(now).is_some_and(|p| p.payload_type != pt::FILLER) {}
    t
}

#[test]
fn filler_probes_one_round_per_acknowledgement() {
    let now = Instant::now();
    let mut t = connected(now);

    let mut fillers = 1;
    while let Some(p) = t.poll_transmit(now) {
        assert_eq!((p.payload_type, p.ssrc), (pt::FILLER, 0));
        fillers += 1;
    }
    assert!((100..=125).contains(&fillers), "one start-up allowance of 1000-byte fillers: {fillers}");
    assert!(t.poll_transmit(now).is_none(), "no second round before the peer acknowledges the first");

    let highest = 100 + fillers - 1;
    let ack = RcHeader {
        log_window: 0xD,
        ack: Some(Ack { sn: highest as u16, received_ts: 0, ack_delay_ms: 0, shift: 0, gaps: vec![] }),
        ..Default::default()
    };
    let mut payload = Vec::new();
    ack.encode(&mut payload);
    t.handle_packet(now + Duration::from_millis(30), packet(pt::MUX_DATA, payload), 40);

    let next = t.poll_transmit(now + Duration::from_millis(30));
    assert!(next.is_some(), "the acknowledgement opens the next round");
}

#[test]
fn acknowledgement_only_packets() {
    let now = Instant::now();
    let mut t = connected(now);
    while t.poll_transmit(now).is_some() {}

    let data = RcHeader { log_window: 0xD, data_sn: Some(100), ..Default::default() };
    let mut payload = Vec::new();
    data.encode(&mut payload);
    payload.extend_from_slice(&[0; 8]);
    t.handle_packet(now, packet(pt::MUX_DATA, payload.clone()), 60);

    let second = RcHeader { log_window: 0xD, data_sn: Some(101), ..Default::default() };
    let mut p2 = Vec::new();
    second.encode(&mut p2);
    t.handle_packet(now, packet(pt::MUX_DATA, p2), 40);

    let ack = t.poll_transmit(now).expect("two in-order packets are acknowledged at once");
    assert_eq!((ack.payload_type, ack.ssrc, ack.sequence), (pt::MUX_DATA, 0, 0));
    let (h, len) = RcHeader::decode(&ack.payload).unwrap();
    assert_eq!(len, ack.payload.len(), "the header alone");
    assert_eq!(h.flags() & flag::DATA, 0);
    assert_eq!(h.ack.unwrap().sn, 101);
}
