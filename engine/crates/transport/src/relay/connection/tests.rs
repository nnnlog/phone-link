use super::*;
use phonelink_core::net;
use phonelink_core::seams::TokenError;
use phonelink_core::types::OpenStatus;
use std::io;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[test]
fn heartbeat_header_is_total_seconds() {
    assert_eq!(heartbeat_frequency_header(Duration::from_secs(15)), "15");
    assert_eq!(heartbeat_frequency_header(Duration::from_secs(0)), "0");
    assert_eq!(heartbeat_frequency_header(Duration::from_millis(1500)), "1.5");
}

#[test]
fn effective_timeouts_require_timeout_above_keepalive() {
    let mut cfg = RelayConfig::default();
    assert_eq!(effective_timeouts(&cfg), (Duration::from_secs(15), Duration::from_secs(30)));

    cfg.keep_alive = Duration::from_millis(100);
    cfg.server_timeout = Duration::from_millis(400);
    assert_eq!(effective_timeouts(&cfg), (Duration::from_millis(100), Duration::from_millis(400)));

    cfg.server_timeout = Duration::from_millis(100);
    assert_eq!(effective_timeouts(&cfg), (LIBRARY_KEEP_ALIVE, LIBRARY_SERVER_TIMEOUT));
}

#[test]
fn breaker_delay_is_base_plus_variance() {
    let mut cfg = RelayConfig::default();
    for _ in 0..100 {
        let d = draw_breaker_delay(&cfg);
        assert!(d >= Duration::from_secs(15) && d < Duration::from_secs(45), "{d:?}");
        assert_eq!(d.subsec_nanos(), 0, "whole seconds of variance: {d:?}");
    }

    cfg.too_many_requests_variance = Duration::ZERO;
    assert_eq!(draw_breaker_delay(&cfg), Duration::from_secs(15));
}

#[test]
fn the_reconnect_delay_after_a_429_is_drawn_anew_for_every_attempt() {
    let cfg = RelayConfig::default();
    let draws: Vec<Duration> = (0..20).map(|_| reconnect_delay(&cfg, 0, true).unwrap()).collect();
    for d in &draws {
        assert!(*d >= Duration::from_secs(15) && *d < Duration::from_secs(45), "{d:?}");
    }

    let distinct: std::collections::BTreeSet<Duration> = draws.iter().copied().collect();
    assert!(distinct.len() > 1, "20 draws with a 30 s variance were all {:?}", draws[0]);

    // Whatever the index, also past the end of the schedule.
    assert!(reconnect_delay(&cfg, cfg.reconnect_delays.len() + 5, true).is_some());

    // Otherwise the schedule by index, then the end.
    assert_eq!(reconnect_delay(&cfg, 0, false), Some(Duration::ZERO));
    assert_eq!(reconnect_delay(&cfg, 1, false), Some(Duration::from_secs(2)));
    assert_eq!(reconnect_delay(&cfg, cfg.reconnect_delays.len(), false), None);
}

#[test]
fn transport_failures_with_an_internet_cause_are_network_failures() {
    let reset = io::Error::from(io::ErrorKind::ConnectionReset);
    assert_eq!(StartError::transport("x".into(), &reset), StartError::Network("x".into()));

    let refused = io::Error::from(io::ErrorKind::ConnectionRefused);
    assert_eq!(StartError::transport("x".into(), &refused), StartError::Transport("x".into()));

    let dns = net::NameResolutionFailed { host: "h.invalid".into(), source: io::Error::other("not known") };
    assert_eq!(StartError::transport("x".into(), &dns), StartError::Network("x".into()));

    let ws = tungstenite::Error::Io(io::Error::from(io::ErrorKind::NetworkUnreachable));
    assert_eq!(StartError::transport("x".into(), &ws), StartError::Network("x".into()));
}

#[test]
fn open_status_mapping() {
    use StartError as E;
    assert_eq!(E::Cancelled.open_status(true), OpenStatus::Cancelled);
    assert_eq!(E::BrokenCircuit.open_status(true), OpenStatus::BrokenCircuitError);
    assert_eq!(E::Http { stage: "negotiate", status: 401 }.open_status(true), OpenStatus::AuthTransientFailure);
    assert_eq!(E::Http { stage: "negotiate", status: 500 }.open_status(true), OpenStatus::Failure);
    assert_eq!(E::Transport("x".into()).open_status(true), OpenStatus::Failure);
    assert_eq!(E::Transport("x".into()).open_status(false), OpenStatus::InternetError);
    assert_eq!(E::Network("x".into()).open_status(true), OpenStatus::InternetError, "the cause chain says so");
    assert!(E::Network("x".into()).is_retryable());
    assert_eq!(E::Token(TokenError::Crypto("x".into())).open_status(true), OpenStatus::CryptoManagerFailure);
    assert_eq!(
        E::Token(TokenError::UserInteractionRequired).open_status(true),
        OpenStatus::MsaTokenUserInteractionRequired
    );
    assert_eq!(E::EmptyToken.open_status(true), OpenStatus::NonTransientAuthFailure);
    assert_eq!(E::Token(TokenError::Msa("x".into())).open_status(true), OpenStatus::NonTransientAuthFailure);

    // An auth failure without internet: the root cause is the missing connectivity.
    assert_eq!(E::Token(TokenError::Msa("x".into())).open_status(false), OpenStatus::InternetError);
    assert_eq!(E::Token(TokenError::UserInteractionRequired).open_status(false), OpenStatus::InternetError);
    assert_eq!(E::Token(TokenError::Crypto("x".into())).open_status(false), OpenStatus::InternetError);
    assert_eq!(E::Token(TokenError::Internet).open_status(true), OpenStatus::InternetError);

    assert!(!E::Token(TokenError::UserInteractionRequired).is_retryable());
    assert!(!E::Token(TokenError::Msa("x".into())).is_retryable());
    assert!(!E::Cancelled.is_retryable());
    assert!(E::Token(TokenError::Crypto("x".into())).is_retryable());
    assert!(E::Token(TokenError::Internet).is_retryable());
    assert!(E::EmptyToken.is_retryable());
    assert!(E::BrokenCircuit.is_retryable());
}

#[tokio::test]
async fn a_wss_upgrade_over_the_own_tcp_stream_speaks_tls() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut first = [0u8; 2];
        socket.read_exact(&mut first).await.unwrap();
        first
    });

    let request = format!("wss://127.0.0.1:{port}/relayhub/?id=t").into_client_request().unwrap();
    let tcp = connect_tcp(&request).await.unwrap();
    assert!(tcp.nodelay().unwrap(), "Nagle is disabled as connect_async(…, disable_nagle = true) did");

    let stream = ActivityStream { inner: tcp, received: ReceiveClock::new() };
    let upgrade = tokio::spawn(tokio_tungstenite::client_async_tls_with_config(request, stream, None, None));

    assert_eq!(server.await.unwrap(), [0x16, 0x03], "the first record is a TLS handshake (ClientHello)");
    assert!(upgrade.await.unwrap().is_err(), "the peer is no TLS server");
}

#[tokio::test]
async fn the_receive_clock_follows_bytes_read_not_bytes_written() {
    let (near, mut far) = tokio::io::duplex(64);
    let clock = ReceiveClock::new();
    let created = clock.last();
    let mut stream = ActivityStream { inner: near, received: clock.clone() };

    tokio::time::sleep(Duration::from_millis(20)).await;
    tokio::io::AsyncWriteExt::write_all(&mut stream, b"out").await.unwrap();
    assert_eq!(clock.last(), created, "writing is not receiving");

    tokio::io::AsyncWriteExt::write_all(&mut far, b"in").await.unwrap();
    let mut buf = [0u8; 8];
    let n = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"in");
    assert!(clock.last() >= created + Duration::from_millis(20), "the read moved the clock");

    let moved = clock.last();
    drop(far);
    assert_eq!(tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await.unwrap(), 0);
    assert_eq!(clock.last(), moved, "end of stream is no activity");
}

#[test]
fn session_ends_that_reconnect() {
    assert!(SessionEnd::Lost("x".into()).reconnects());
    assert!(SessionEnd::ServerClose { error: None, allow_reconnect: Some(true) }.reconnects());
    assert!(!SessionEnd::ServerClose { error: None, allow_reconnect: Some(false) }.reconnects());
    assert!(!SessionEnd::ServerClose { error: None, allow_reconnect: None }.reconnects());
    assert!(!SessionEnd::Stopped.reconnects());
}
