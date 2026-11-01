use super::*;

fn create(name: &str) -> Vec<u8> {
    Control::Create { name: name.into(), fec_type: 0, init_info: Vec::new(), priority: 1.0 }.encode()
}

#[test]
fn control_codec() {
    let c = Control::decode(&create("Microsoft::Basix::Dct::Channel::Class::Messaging,3,4")).unwrap();
    assert!(matches!(c, Control::Create { fec_type: 0, priority: 1.0, .. }));

    // Without the optional tail.
    let mut short = 2u32.to_le_bytes().to_vec();
    short.extend_from_slice(&1u16.to_le_bytes());
    short.push(b'x');
    short.extend_from_slice(&0u32.to_le_bytes());
    assert_eq!(
        Control::decode(&short).unwrap(),
        Control::Create { name: b"x".to_vec(), fec_type: 0, init_info: vec![], priority: 1.0 }
    );

    assert!(Control::decode(&9u32.to_le_bytes()).is_err());
    assert_eq!(Control::Close.encode(), vec![4, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn lifecycle_and_parking() {
    let mut m = Mux::default();
    let open = Control::Open { init_info: vec![], priority: 1.0 }.encode();
    m.on_packet(pt::MUX_CONTROL, 5, 0, &open).unwrap();
    assert!(m.events().is_empty(), "parked");

    m.on_packet(pt::MUX_CONTROL, 5, 0, &create("c")).unwrap();
    assert!(matches!(m.events()[0], MuxEvent::Created { id: 5, .. }));

    m.open(5).unwrap();
    assert_eq!(m.events(), vec![MuxEvent::Opened(5)]);
    let out = m.next_packet().unwrap();
    assert_eq!((out.payload_type, out.ssrc, out.reliable), (pt::MUX_CONTROL, 5, true));

    m.on_packet(pt::MUX_DATA, 99, 0, &[0; 8]).unwrap();
    assert!(m.events().is_empty(), "unknown channel dropped");

    m.on_packet(pt::MUX_CONTROL, 5, 0, &Control::Close.encode()).unwrap();
    assert_eq!(m.events(), vec![MuxEvent::Closed(5)]);
}

#[test]
fn sequencer_orders_reliable_and_waits_for_fences() {
    let mut tx = SequencerTx::default();
    let mut rx = SequencerRx::default();
    let mut packets = Vec::new();
    for (mode, body) in
        [(SendMode::Reliable, 1u8), (SendMode::ReliableFence, 2), (SendMode::Unreliable, 3), (SendMode::Reliable, 4)]
    {
        let (sn, h) = tx.header(mode);
        let mut p = h.to_vec();
        p.push(body);
        packets.push((sn as u16, p));
    }

    assert_eq!(packets[2].1[0], 1, "best effort after a fence");
    assert_eq!(packets[3].1[..4], [3, 0, 1, 0], "reliable after the fence");

    // Arrive 3, 4, 2, 1.
    assert!(rx.on_packet(packets[2].0, &packets[2].1).unwrap().is_empty());
    assert!(rx.on_packet(packets[3].0, &packets[3].1).unwrap().is_empty());
    assert!(rx.on_packet(packets[1].0, &packets[1].1).unwrap().is_empty());
    let all = rx.on_packet(packets[0].0, &packets[0].1).unwrap();
    assert_eq!(all, vec![vec![1], vec![2], vec![3], vec![4]]);

    assert!(rx.on_packet(packets[0].0, &packets[0].1).unwrap().is_empty(), "duplicate");
    assert!(rx.on_packet(0, &[7, 0, 0, 0, 0, 0, 0, 0]).is_err());
    assert!(rx.on_packet(9, &[3, 0, 9, 0, 0, 0, 0, 0]).is_err(), "dependency anomaly");
}

#[test]
fn priority_weighting() {
    let mut m = Mux::default();
    for (id, priority) in [(1u16, 1.0), (2, 3.0)] {
        let c = Control::Create { name: b"c".to_vec(), fec_type: 0, init_info: vec![], priority }.encode();
        m.on_packet(pt::MUX_CONTROL, u32::from(id), 0, &c).unwrap();
        for _ in 0..8 {
            m.send(id, SendMode::Unreliable, pt::MUX_DATA, vec![0]).unwrap();
        }
    }

    let first: Vec<u32> = (0..8).map(|_| m.next_packet().unwrap().ssrc).collect();
    let twos = first.iter().filter(|s| **s == 2).count();
    assert!(twos >= 5, "{first:?}");
}
