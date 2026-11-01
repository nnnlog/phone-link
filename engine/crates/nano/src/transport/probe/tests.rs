use super::*;

#[test]
fn handshake() {
    let t0 = Instant::now();
    let mut p = Prober::new(t0, 1402);
    let burst = p.drain();
    let sizes: Vec<usize> = burst.iter().map(|b| b.payload.len()).collect();
    assert_eq!(sizes, vec![1402, 1386, 1370, 1354, 1338, 1322]);
    assert_eq!(burst[5].payload[2], 1, "last flag on the final SYN");

    assert!(p.on_packet(probe_packet(vec![2, 0, 0x50, 0x05])).is_err(), "SYNACK in ClientStart");

    let mut peer_syn = vec![0u8; 1300];
    peer_syn[0] = 1;
    assert_eq!(p.on_packet(probe_packet(peer_syn)).unwrap(), Inbound::Consumed);
    let synack = p.drain();
    assert_eq!(synack[0].payload, vec![2, 0, 0x14, 0x05], "SYNACK names 1300");

    assert_eq!(p.on_packet(probe_packet(vec![2, 0, 0x7A, 0x05])).unwrap(), Inbound::Opened(1402, None));
    assert_eq!(p.state(), State::Open);
}

#[test]
fn later_bursts_shrink_and_exhaustion_fails() {
    let t0 = Instant::now();
    let mut p = Prober::new(t0, 1402);
    p.drain();

    p.on_timeout(t0 + RETRY).unwrap();
    assert_eq!(p.drain()[0].payload.len(), 1302);

    let mut t = t0 + RETRY;
    let mut result = Ok(());
    for _ in 0..10 {
        t += RETRY;
        result = p.on_timeout(t);
    }
    assert!(result.is_err());
}
