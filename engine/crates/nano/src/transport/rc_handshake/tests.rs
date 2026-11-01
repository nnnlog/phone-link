use super::*;

fn server_syn(retry: u16) -> RtpPacket {
    let mut p = SYN.to_le_bytes().to_vec();
    p.extend_from_slice(&3u16.to_le_bytes());
    p.push(14);
    p.extend_from_slice(&100u64.to_le_bytes());
    p.extend_from_slice(&5u16.to_le_bytes());
    p.extend_from_slice(&retry.to_le_bytes());
    packet(p)
}

#[test]
fn client_sequence() {
    let t0 = Instant::now();
    let mut h = RcHandshake::new(t0);
    let syn = h.drain();
    assert_eq!(syn[0].payload, server_syn(0).payload, "the PC's SYN has the same fields");

    h.on_timeout(t0 + RETRY);
    assert_eq!(h.drain()[0].payload[15..], [1, 0], "retry index 1");

    assert!(h.on_packet(t0, server_syn(5)).is_err(), "never sent");
    h.on_packet(t0 + Duration::from_millis(900), server_syn(1)).unwrap();
    let synack = h.drain();
    assert_eq!(synack[0].payload, [vec![1, 0], 100_000u32.to_le_bytes().to_vec(), vec![1, 0]].concat());

    let done = h.on_packet(t0, packet(ACKOFACK.to_le_bytes().to_vec())).unwrap();
    assert!(matches!(done, Inbound::Connected(PeerSyn { initial_send_sn: 100, .. }, None)));
    assert!(h.is_connected());
}

#[test]
fn data_completes_or_fails() {
    let t0 = Instant::now();
    let data = RtpPacket { payload_type: 0x23, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload: vec![] };
    let mut h = RcHandshake::new(t0);
    assert!(h.on_packet(t0, data.clone()).is_err());

    h.on_packet(t0, server_syn(0)).unwrap();
    assert!(matches!(h.on_packet(t0, data).unwrap(), Inbound::Connected(_, Some(_))));
}
