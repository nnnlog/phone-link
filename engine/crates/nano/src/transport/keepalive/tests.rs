use super::*;

fn handshake(flags: u32, interval: u64, timeout: u64) -> Vec<u8> {
    let mut p = 0u32.to_le_bytes().to_vec();
    p.extend_from_slice(&flags.to_le_bytes());
    p.extend_from_slice(&interval.to_le_bytes());
    p.extend_from_slice(&timeout.to_le_bytes());

    if flags & flag::MTU_PARAMETERS != 0 {
        p.extend_from_slice(&10u32.to_le_bytes());
        p.extend_from_slice(&600u32.to_le_bytes());
        p.extend_from_slice(&5000u64.to_le_bytes());
    }
    p
}

#[test]
fn handshake_reply_and_small_mode() {
    let t0 = Instant::now();
    let mut k = KeepAlive::new(t0, 1402, Duration::from_secs(5));
    assert_eq!(k.mtu(), 1368);

    let p = handshake(0x9, 400, 3000);
    k.on_any_packet(t0, p.len());
    k.on_keepalive(&p).unwrap();
    let reply = k.drain();
    assert_eq!(reply[0].payload, [40u32.to_le_bytes(), 0x12u32.to_le_bytes()].concat());

    // Ticks 1–9 send an 8-byte keepalive only when idle; tick 10 an MTU-sized one.
    k.on_sent();
    k.on_timeout(t0 + Duration::from_millis(400));
    assert!(k.drain().is_empty(), "size word unchanged and data was sent");
    k.on_timeout(t0 + Duration::from_millis(800));
    assert_eq!(k.drain()[0].payload.len(), 8);

    let mut t = t0 + Duration::from_millis(800);
    for _ in 3..=10 {
        t += Duration::from_millis(400);
        k.on_any_packet(t, 40);
        k.on_timeout(t);
    }
    assert!(k.drain().iter().any(|p| p.payload.len() == 1368));
}

#[test]
fn normal_mode_probes_and_echo_raises_mtu() {
    let t0 = Instant::now();
    let mut k = KeepAlive::new(t0, 1402, Duration::from_secs(5));

    k.on_timeout(t0 + Duration::from_millis(100));
    let sizes: Vec<usize> = k.drain().iter().map(|p| p.payload.len()).collect();
    assert_eq!(sizes, vec![1402, 1368], "probe min(1368 × 1.1, 1402), then the working MTU");

    let mut echo = 1402u32.to_le_bytes().to_vec();
    echo.extend_from_slice(&0u32.to_le_bytes());
    k.on_keepalive(&echo).unwrap();
    assert_eq!(k.mtu(), 1402);
    assert_eq!(k.events(), vec![KeepAliveEvent::MtuChanged(1402)]);
}

#[test]
fn timeouts_close_and_peer_close() {
    let t0 = Instant::now();
    let mut k = KeepAlive::new(t0, 1402, Duration::from_millis(500));
    k.on_timeout(t0 + Duration::from_millis(600));
    assert_eq!(k.events(), vec![KeepAliveEvent::MissedPackets]);

    k.on_any_packet(t0 + Duration::from_millis(700), 8);
    assert_eq!(k.events(), vec![KeepAliveEvent::RecoveredPackets]);

    k.on_timeout(t0 + Duration::from_millis(1800));
    assert!(k.events().contains(&KeepAliveEvent::End(KeepAliveEnd::Timeout)));

    let close = k.close_packet();
    k.on_keepalive(&close.payload).unwrap();
    assert_eq!(k.events(), vec![KeepAliveEvent::End(KeepAliveEnd::PeerClosed)]);
}
