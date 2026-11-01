//! One Nano connection attempt and, once open, the connection: the UDP sockets, ICE or the direct
//! `IpSrtp` path, SRTP and the transport, run by one task.
//!
//! [`ConnectObject`] is the attempt (initialize → offer, open with the answer); [`NanoConnection`]
//! is what an attempt that opened yields.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::debug;

use super::channel::{ChannelError, ChannelLink, RawChannel};
use super::contract::{IceServers, IceSrtpOffer, MasterKey, ip_srtp_offer};
use super::ice::agent::{Base, IceAgent, IceConfig, TurnServer};
use super::metrics::Aggregators;
use super::netif::{HostAddress, host_addresses};
use super::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod state;
use state::State;

/// The largest UDP payload the socket layer sends, IPv4 / IPv6.
const SOCKET_MAX_V4: usize = 1472;
const SOCKET_MAX_V6: usize = 1452;
/// What the ICE link takes from it.
const ICE_LINK_OVERHEAD: usize = 40;
const RECEIVE_BUFFER: usize = 2048;

/// The native connection types the PC opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionType {
    /// The phone listens on a UDP port and advertises its addresses; no ICE.
    IpSrtp,
    /// ICE.
    IceSrtp,
}

impl ConnectionType {
    /// `open.timeout`: from `Open` to opened.
    pub fn open_timeout(self) -> Duration {
        match self {
            ConnectionType::IpSrtp => Duration::from_millis(7000),
            ConnectionType::IceSrtp => Duration::from_millis(10_000),
        }
    }
}

/// Why an attempt or a connection closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloseReason {
    // Connection setup.
    #[error("connection setup canceled")]
    UserCanceled,
    #[error("connection setup timed out")]
    TimeoutExceeded,
    #[error("a STUN or TURN host did not resolve")]
    HostnameResolutionError,
    #[error("closed without error")]
    None,
    #[error("connection setup failed: {0}")]
    Unknown(String),
    // After opening.
    #[error("closed locally")]
    UserEnded,
    #[error("keepalive timeout")]
    KeepAliveTimeout,
    #[error("the phone ended the connection")]
    PeerEnded,
    #[error("connection failed: {0}")]
    ClientUnknown(String),
}

/// `missed packets` / `recovered packets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    MissedPackets,
    RecoveredPackets,
}

/// The inputs of one attempt.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub kind: ConnectionType,
    /// `IceSrtp`: the STUN/TURN strings written into the offer and gathered with.
    pub servers: IceServers,
    /// The host addresses to gather on; `None` = the machine's interfaces.
    pub host_addresses: Option<Vec<HostAddress>>,
    /// Overrides `open.timeout`.
    pub open_timeout: Option<Duration>,
    /// Diagnostics: offer and pair only relayed candidates (`IceSrtp`).
    pub relay_only: bool,
}

impl ConnectOptions {
    pub fn new(kind: ConnectionType) -> Self {
        Self { kind, servers: IceServers::default(), host_addresses: None, open_timeout: None, relay_only: false }
    }
}

enum Command {
    Open(Value, oneshot::Sender<Result<NanoConnection, CloseReason>>),
    ChannelOpen(u16),
    ChannelSend {
        id: u16,
        mode: SendMode,
        payload_type: u8,
        body: Vec<u8>,
    },
    ChannelClose(u16),
    Forward(mpsc::UnboundedSender<RawChannel>),
    Close,
    /// Close as a lost connection; `trigger` says why, for the close's log line.
    CloseLost {
        trigger: &'static str,
    },
}

#[derive(Default)]
struct Shared {
    max_payload: AtomicUsize,
    fill: AtomicU64,
    bandwidth: AtomicU64,
}

struct Link {
    cmd: mpsc::UnboundedSender<Command>,
    shared: Arc<Shared>,
}

impl ChannelLink for Link {
    fn open(&self, id: u16) -> Result<(), ChannelError> {
        self.cmd.send(Command::ChannelOpen(id)).map_err(|_| ChannelError::ConnectionGone)
    }

    fn send(&self, id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ChannelError> {
        let max = self.max_payload();
        if body.len() > max {
            return Err(ChannelError::Transport(format!("payload of {} bytes over the {max}-byte limit", body.len())));
        }
        self.cmd.send(Command::ChannelSend { id, mode, payload_type, body }).map_err(|_| ChannelError::ConnectionGone)
    }

    fn close(&self, id: u16) {
        let _ = self.cmd.send(Command::ChannelClose(id));
    }

    fn max_payload(&self) -> usize {
        self.shared.max_payload.load(Ordering::Relaxed)
    }

    fn send_queue_fill(&self) -> f64 {
        f64::from_bits(self.shared.fill.load(Ordering::Relaxed))
    }

    fn bandwidth_bps(&self) -> f64 {
        f64::from_bits(self.shared.bandwidth.load(Ordering::Relaxed))
    }
}

/// An attempt between `Initialize` and `Open`.
pub struct ConnectObject {
    pub kind: ConnectionType,
    cmd: mpsc::UnboundedSender<Command>,
    /// Dropping the attempt cancels it until `open` hands it the answer.
    armed: bool,
}

impl ConnectObject {
    /// Creates the attempt and waits for its offer (`IceSrtp`: once gathering finished).
    pub async fn initialize(options: ConnectOptions) -> Result<(Self, Map<String, Value>), CloseReason> {
        let (cmd, cmd_rx) = mpsc::unbounded_channel();
        let (offer_tx, offer_rx) = oneshot::channel();
        let kind = options.kind;
        let runner = Runner::new(options, cmd.clone()).await?;
        tokio::spawn(runner.run(cmd_rx, offer_tx));

        let me = Self { kind, cmd, armed: true };
        match offer_rx.await {
            Ok(Ok(offer)) => Ok((me, offer)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(CloseReason::Unknown("the attempt ended before its offer".into())),
        }
    }

    /// Opens the connection with the phone's answer (the whole object; unknown keys ignored).
    /// A caller that stops waiting leaves the attempt to its `open.timeout`; a connection that opens
    /// after that closes at once.
    pub async fn open(mut self, answer: Value) -> Result<NanoConnection, CloseReason> {
        let (tx, rx) = oneshot::channel();
        self.cmd.send(Command::Open(answer, tx)).map_err(|_| CloseReason::Unknown("the attempt is gone".into()))?;
        self.armed = false;
        rx.await.unwrap_or(Err(CloseReason::UserCanceled))
    }

    pub fn close(&self) {
        let _ = self.cmd.send(Command::Close);
    }
}

impl Drop for ConnectObject {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.cmd.send(Command::Close);
        }
    }
}

/// An open Nano connection.
pub struct NanoConnection {
    pub kind: ConnectionType,
    cmd: mpsc::UnboundedSender<Command>,
    health: broadcast::Sender<Health>,
    closed: watch::Receiver<Option<CloseReason>>,
    metrics: Arc<Mutex<Aggregators>>,
}

impl std::fmt::Debug for NanoConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NanoConnection").field("kind", &self.kind).finish()
    }
}

impl NanoConnection {
    /// The channels the phone creates: the ones created so far, then each as it comes. Only one
    /// receiver exists; a second call replaces the first.
    pub fn channels(&self) -> mpsc::UnboundedReceiver<RawChannel> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.cmd.send(Command::Forward(tx));
        rx
    }

    pub fn health(&self) -> broadcast::Receiver<Health> {
        self.health.subscribe()
    }

    /// `Some(reason)` once the connection closed.
    pub fn closed(&self) -> watch::Receiver<Option<CloseReason>> {
        self.closed.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.borrow().is_some()
    }

    /// Closes the connection: Close for every channel, then the keepalive close packet.
    pub fn close(&self) {
        let _ = self.cmd.send(Command::Close);
    }

    /// Closes the connection as lost (reported `KeepAliveTimeout`).
    pub fn close_lost(&self) {
        let _ = self.cmd.send(Command::CloseLost { trigger: "closed as lost by its owner" });
    }

    /// Closes the connection as lost when it reported missed packets and did not recover within
    /// `after` (the screen-mirroring path's rule).
    pub fn close_when_unrecovered(&self, after: Duration) {
        let mut health = self.health();
        let mut closed = self.closed();
        let cmd = self.cmd.clone();

        tokio::spawn(async move {
            let mut deadline: Option<Instant> = None;
            loop {
                tokio::select! {
                    h = health.recv() => match h {
                        Ok(Health::MissedPackets) => deadline = Some(Instant::now() + after),
                        Ok(Health::RecoveredPackets) => deadline = None,
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return,
                    },
                    _ = closed.wait_for(Option::is_some) => return,
                    _ = sleep_until(deadline) => {
                        let _ = cmd.send(Command::CloseLost { trigger: "missed packets not recovered" });
                        return;
                    }
                }
            }
        });
    }

    /// The real-time metrics document.
    pub fn realtime_metrics(&self) -> String {
        self.metrics.lock_or_panic().realtime()
    }

    /// The cumulative metrics document (since the connection opened).
    pub fn cumulative_metrics(&self) -> String {
        self.metrics.lock_or_panic().cumulative()
    }
}

impl Drop for NanoConnection {
    fn drop(&mut self) {
        let _ = self.cmd.send(Command::Close);
    }
}

enum Path {
    Ice(Box<IceAgent>),
    Direct(Option<SocketAddr>),
}

struct Runner {
    kind: ConnectionType,
    cmd: mpsc::UnboundedSender<Command>,
    sockets: Vec<NanoSocket>,
    path: Path,
    servers: IceServers,
    key: MasterKey,
    resolution_failed: bool,
    open_timeout: Duration,
}

/// One local UDP socket: the tokio half its reader task receives on, and a non-blocking clone that
/// sends without waiting for readiness (a fresh tokio socket's `try_send_to` reports `WouldBlock` until
/// its first writable event).
struct NanoSocket {
    recv: Arc<UdpSocket>,
    send: std::net::UdpSocket,
}

impl NanoSocket {
    fn bind(address: SocketAddr) -> std::io::Result<Self> {
        let socket = std::net::UdpSocket::bind(address)?;
        socket.set_nonblocking(true)?;
        let send = socket.try_clone()?;
        Ok(Self { recv: Arc::new(UdpSocket::from_std(socket)?), send })
    }

    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.send.local_addr()
    }
}

async fn resolve(host: &str, port: u16) -> Option<SocketAddr> {
    match tokio::net::lookup_host((host, port)).await {
        Ok(mut addrs) => addrs.next(),
        Err(e) => {
            debug!(host, error = %e, "STUN/TURN host did not resolve");
            None
        }
    }
}

impl Runner {
    async fn new(options: ConnectOptions, cmd: mpsc::UnboundedSender<Command>) -> Result<Self, CloseReason> {
        let open_timeout = options.open_timeout.unwrap_or(options.kind.open_timeout());
        let mut runner = Self {
            kind: options.kind,
            cmd,
            sockets: Vec::new(),
            path: Path::Direct(None),
            servers: options.servers.clone(),
            key: MasterKey::random(),
            resolution_failed: false,
            open_timeout,
        };

        if options.kind == ConnectionType::IpSrtp {
            return Ok(runner);
        }

        let hosts = match options.host_addresses {
            Some(h) => h,
            None => host_addresses().map_err(|e| CloseReason::Unknown(format!("listing interfaces: {e}")))?,
        };

        let mut bases = Vec::new();
        for h in hosts {
            match NanoSocket::bind(h.address) {
                Ok(s) => {
                    let address = s.local_addr().map_err(|e| CloseReason::Unknown(e.to_string()))?;
                    bases.push(Base { address, interface_index: h.interface_index });
                    runner.sockets.push(s);
                }
                Err(e) => debug!(address = %h.address, error = %e, "host address not bound"),
            }
        }

        if bases.is_empty() {
            return Err(CloseReason::Unknown("no usable host address".into()));
        }

        let mut stun = Vec::new();
        for entry in options.servers.stun_servers() {
            match resolve(&entry.host, entry.port).await {
                Some(a) => stun.push(a),
                None => runner.resolution_failed = true,
            }
        }

        let mut turn = None;
        if let Some(entry) = options.servers.turn_server() {
            match resolve(&entry.host, entry.port).await {
                Some(address) => {
                    turn = Some(TurnServer {
                        address,
                        username: options.servers.turn_username.clone(),
                        password: options.servers.turn_password.clone(),
                    })
                }
                None => runner.resolution_failed = true,
            }
        }

        let now = Instant::now().into_std();
        runner.path =
            Path::Ice(Box::new(IceAgent::new(now, bases, IceConfig { stun, turn, relay_only: options.relay_only })));
        Ok(runner)
    }

    fn offer(&self) -> Map<String, Value> {
        match &self.path {
            Path::Ice(agent) => IceSrtpOffer {
                key: self.key.clone(),
                description: agent.local_description(),
                servers: self.servers.clone(),
            }
            .to_json(),
            Path::Direct(_) => ip_srtp_offer(),
        }
    }

    async fn run(
        self,
        cmd_rx: mpsc::UnboundedReceiver<Command>,
        offer: oneshot::Sender<Result<Map<String, Value>, CloseReason>>,
    ) {
        let (datagram_tx, datagrams) = mpsc::unbounded_channel();
        let mut state = State {
            runner: self,
            cmd_rx,
            offer: Some(offer),
            opening: None,
            deadline: None,
            datagrams,
            datagram_tx,
            readers: Vec::new(),
            srtp: None,
            transport: None,
            warning: Duration::ZERO,
            channels: HashMap::new(),
            stash: Vec::new(),
            forward: None,
            shared: Arc::new(Shared::default()),
            connection: None,
            metrics: None,
            dropped_seen: (0, 0),
            close_trigger: None,
            done: false,
        };

        state.main().await;
    }
}

async fn sleep_until(t: Option<Instant>) {
    match t {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}
