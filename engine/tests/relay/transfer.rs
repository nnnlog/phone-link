//! Large transfers on a live hub connection (the ASP.NET Core `SignalR` client closes the connection
//! when nothing is received for `ServerTimeout`; the heartbeat window): a long client write must
//! neither stop the client from reading nor trip the server timeout, and a long inbound message
//! keeps the connection as long as its bytes keep arriving. Real time with short configured timers;
//! the inbound tests put a throttling TCP proxy between the client and the fake hub.

use crate::support::fake_hub;
use crate::support::relay_world as fixtures;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fake_hub::{FakeHub, on_partner_connected, on_partner_heartbeat, on_receive_message};
use fixtures::{Harness, PHONE, RecordingPacketSink, drain_events, next_event, phone};
use phonelink_engine::codec::dcg_packet::Fragment;
use phonelink_engine::codec::hub::{HubMessage, MultiplexPacket, method};
use phonelink_engine::config::EngineConfig;
use phonelink_engine::seams::{HubControl, HubLink, RelayEvent};
use phonelink_engine::tap::{Direction, TapLayer};
use phonelink_engine::types::{Environment, HubSendResult, HubState, TraceContext};
use rmpv::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const SECOND: Duration = Duration::from_secs(1);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Keepalive 100 ms, server timeout 500 ms; a lost connection would wait 30 s before reconnecting,
/// so a spurious loss stays visible as `Reconnecting`.
fn short_timeouts(c: &mut EngineConfig) {
    c.relay.keep_alive = ms(100);
    c.relay.server_timeout = ms(500);
    c.relay.reconnect_delays = vec![Duration::from_secs(30)];
}

fn hub_states(events: &[RelayEvent]) -> Vec<HubState> {
    events
        .iter()
        .filter_map(|e| match e {
            RelayEvent::HubState { state, .. } => Some(*state),
            _ => None,
        })
        .collect()
}

fn partner_losses(events: &[RelayEvent]) -> Vec<&RelayEvent> {
    events.iter().filter(|e| matches!(e, RelayEvent::PartnerDisconnected { .. })).collect()
}

/// A single-fragment `ms-dcg` packet carrying `raw_len` payload bytes.
fn large_packet(raw_len: usize) -> MultiplexPacket {
    Fragment {
        version: 1.0,
        session_id: "6f9619ff-8b86-d011-b42d-00c04fc964ff".into(),
        sequence_number: 1,
        message_id: 1,
        message_type_raw: Some(1),
        fragment_id: 1,
        fragment_count: 1,
        raw: vec![0x5A; raw_len],
    }
    .to_packet()
}

/// Runs `step` every `every` until the returned token is cancelled.
fn every(every: Duration, step: impl Fn() + Send + 'static) -> CancellationToken {
    let stop = CancellationToken::new();
    let token = stop.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = token.cancelled() => return,
                _ = tokio::time::sleep(every) => step(),
            }
        }
    });

    stop
}

// ------------------------------------------------------------------------------------------------
// A throttling TCP proxy
// ------------------------------------------------------------------------------------------------

/// How the proxy forwards the hub → client direction (client → hub always passes freely).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
    Free,
    /// At most `bytes` per `every`.
    Trickle {
        bytes: usize,
        every: Duration,
    },
    /// Nothing at all.
    Stalled,
}

/// Forwards every connection to the fake hub on `127.0.0.1`, pacing what the hub sends.
struct ThrottleProxy {
    addr: SocketAddr,
    pace: Arc<watch::Sender<Pace>>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for ThrottleProxy {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl ThrottleProxy {
    async fn start(hub_port: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind 127.0.0.1:0");
        let addr = listener.local_addr().expect("local addr");
        let pace = Arc::new(watch::channel(Pace::Free).0);

        let server = tokio::spawn({
            let pace = pace.clone();
            async move {
                loop {
                    let Ok((client, _)) = listener.accept().await else { return };
                    let pace = pace.subscribe();
                    tokio::spawn(async move {
                        let Ok(hub) = TcpStream::connect(("127.0.0.1", hub_port)).await else { return };
                        let _ = client.set_nodelay(true);
                        let _ = hub.set_nodelay(true);
                        let (mut client_read, mut client_write) = client.into_split();
                        let (hub_read, mut hub_write) = hub.into_split();

                        // Either direction ending closes both sockets.
                        tokio::select! {
                            _ = tokio::io::copy(&mut client_read, &mut hub_write) => {}
                            _ = paced_copy(hub_read, &mut client_write, pace) => {}
                        }
                    });
                }
            }
        });

        Self { addr, pace, server }
    }

    /// `http://127.0.0.1:<port>/` — an environment base.
    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.addr.port())
    }

    fn set_pace(&self, pace: Pace) {
        self.pace.send_replace(pace);
    }
}

async fn paced_copy(mut from: OwnedReadHalf, to: &mut OwnedWriteHalf, mut pace: watch::Receiver<Pace>) {
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let current = *pace.borrow_and_update();
        let limit = match current {
            Pace::Stalled => {
                if pace.changed().await.is_err() {
                    return;
                }
                continue;
            }
            Pace::Free => buf.len(),
            Pace::Trickle { bytes, .. } => bytes.min(buf.len()),
        };

        let n = tokio::select! {
            read = from.read(&mut buf[..limit]) => match read {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            },
            changed = pace.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
        };

        if to.write_all(&buf[..n]).await.is_err() {
            return;
        }

        if let Pace::Trickle { every, .. } = current {
            tokio::select! {
                _ = tokio::time::sleep(every) => {}
                changed = pace.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------------
// A long client write
// ------------------------------------------------------------------------------------------------

/// 24 MiB in one WebSocket frame: far more than the loopback socket buffers hold, so the write
/// stalls while the hub does not read.
const LARGE_WRITE: usize = 24 << 20;

/// Waits until the client queued its `SendMessageAsync` invocation (its tap event is emitted just
/// before the frame is handed to the writer). Encoding a large frame takes a while in a debug build.
async fn wait_invocation_queued(h: &Harness) {
    let deadline = Instant::now() + 30 * SECOND;

    loop {
        let queued = h.tap.take().into_iter().any(|e| {
            e.layer == TapLayer::HubMessage
                && e.direction == Direction::Out
                && e.summary["target"] == serde_json::json!(method::SEND_MESSAGE)
        });

        if queued {
            return;
        }

        assert!(Instant::now() < deadline, "the client never queued the invocation");
        tokio::time::sleep(ms(20)).await;
    }
}

/// Waits until the client has read `count` more `OnPartnerHeartbeat` invocations. Read while a write
/// is stuck, they say that it keeps reading; sent one every 100 ms, they span `count` × 100 ms.
async fn wait_heartbeats_read(h: &Harness, count: usize) {
    let deadline = Instant::now() + 30 * SECOND;
    let mut read = 0;

    loop {
        read += h
            .tap
            .take()
            .into_iter()
            .filter(|e| {
                e.layer == TapLayer::HubMessage
                    && e.direction == Direction::In
                    // An invocation frame as read: `[1, headers, invocationId, target, arguments]`.
                    && e.summary[3] == serde_json::json!(method::ON_PARTNER_HEARTBEAT)
            })
            .count();

        if read >= count {
            return;
        }

        assert!(Instant::now() < deadline, "the client read {read} heartbeats, not {count}");
        tokio::time::sleep(ms(20)).await;
    }
}

// Several workers: encoding the frame is CPU work that must not stall the hub or the timers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_write_keeps_reading_and_does_not_trip_the_server_timeout() {
    // Heartbeat windows of 150 ms: a partner whose heartbeats went unread for 450 ms would be removed.
    let mut h = Harness::new(|c| {
        short_timeouts(c);
        c.relay.heartbeat_frequency = ms(100);
        c.relay.heartbeat_grace = ms(50);
    })
    .await;

    h.hub.set_ping_interval(Some(ms(100)));
    let session = h.open_session().await;
    session.send(on_partner_connected(PHONE, Value::Nil));
    next_event(&mut h.events, SECOND, |e| matches!(e, RelayEvent::PartnerConnected(p) if *p == phone())).await;

    let heartbeats = every(ms(100), {
        let session = session.clone();
        move || session.send(on_partner_heartbeat(PHONE, "conn-1"))
    });

    session.pause_reading();
    let relay = h.relay.clone();
    let send = tokio::spawn(async move {
        relay.send_packet(&phone(), &TraceContext::new_root(Vec::new()), large_packet(LARGE_WRITE), None).await
    });

    wait_invocation_queued(&h).await;
    // More than three server timeouts (500 ms) and three heartbeat windows while the write is stuck.
    wait_heartbeats_read(&h, 16).await;

    assert!(!send.is_finished(), "the hub has not read the invocation, so it cannot have completed");
    let events = drain_events(&mut h.events);
    assert_eq!(hub_states(&events), vec![], "no Lost / Reconnecting while bytes keep arriving");
    assert_eq!(partner_losses(&events), Vec::<&RelayEvent>::new(), "heartbeats were processed during the write");
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);
    assert!(h.relay.is_partner_connected(&phone()));

    session.resume_reading();
    let result = tokio::time::timeout(20 * SECOND, send).await.expect("the send completes once the hub reads").unwrap();
    assert_eq!(result, HubSendResult::Success);

    heartbeats.cancel();
    let events = drain_events(&mut h.events);
    assert_eq!(hub_states(&events), vec![]);
    assert_eq!(partner_losses(&events), Vec::<&RelayEvent>::new());
    assert!(h.relay.is_partner_connected(&phone()));

    let invocation = session.wait_invocation(SECOND, |i| i.target == method::SEND_MESSAGE).await;
    assert_eq!(invocation.packet().and_then(|p| p.raw).map(|raw| raw.len()), Some(LARGE_WRITE));
    assert_eq!(h.hub.sessions().len(), 1, "no reconnect happened");
}

// ------------------------------------------------------------------------------------------------
// A long inbound message
// ------------------------------------------------------------------------------------------------

/// The fake hub behind a [`ThrottleProxy`]; the client connects to the proxy.
async fn proxied() -> (Harness, ThrottleProxy) {
    let hub = FakeHub::start().await;
    let proxy = ThrottleProxy::start(hub.port()).await;
    let base = proxy.base_url();

    let h = Harness::with_hub(hub, move |c| {
        short_timeouts(c);
        c.endpoints.prod_base = base;
    });
    (h, proxy)
}

#[tokio::test]
async fn a_long_inbound_message_keeps_the_connection_while_its_bytes_flow() {
    let (mut h, proxy) = proxied().await;
    let sink = Arc::new(RecordingPacketSink::default());
    h.relay.set_packet_sink(sink.clone());
    let session = h.open_session().await;
    drain_events(&mut h.events);

    // 16 KiB every 100 ms: the 256 KiB message takes ≈ 1.6 s — over three server timeouts — while
    // no gap between its bytes comes near one.
    proxy.set_pace(Pace::Trickle { bytes: 16 * 1024, every: ms(100) });
    let started = Instant::now();
    session.send(on_receive_message(PHONE, &large_packet(256 * 1024)));
    let packets = sink.wait(1, 10 * SECOND).await;
    let took = started.elapsed();

    assert!(took >= ms(1000), "the message arrived after {took:?}; it was not trickled");
    assert_eq!(packets[0].packet.raw.as_ref().map(Vec::len), Some(256 * 1024));
    assert_eq!(hub_states(&drain_events(&mut h.events)), vec![], "no Lost / Reconnecting during the message");
    assert_eq!(h.relay.hub_state(Environment::Prod), HubState::Connected);

    // The hub keeps pinging, but the proxy forwards nothing: the server timeout fires one timeout
    // after the last byte (the end of the message, just before `stalled`).
    proxy.set_pace(Pace::Stalled);
    let stalled = Instant::now();
    let pings = every(ms(100), {
        let session = session.clone();
        move || session.send(HubMessage::Ping)
    });

    next_event(&mut h.events, 3 * SECOND, |e| matches!(e, RelayEvent::HubState { state: HubState::Reconnecting, .. }))
        .await;
    let waited = stalled.elapsed();
    pings.cancel();

    assert!(waited >= ms(400) && waited < ms(1500), "Reconnecting {waited:?} after the bytes stopped");
    assert_eq!(h.hub.sessions().len(), 1);
}
