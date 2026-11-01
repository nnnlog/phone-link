//! The connection's event loop: datagrams, timers, commands, the transport and its channels.

use super::{
    CloseReason, Command, Health, ICE_LINK_OVERHEAD, Link, NanoConnection, NanoSocket, Path, RECEIVE_BUFFER, Runner,
    SOCKET_MAX_V4, SOCKET_MAX_V6, Shared, sleep_until,
};
use crate::channel::{Announcement, RawChannel, RawInbound};
use crate::contract::{IceSrtpAnswer, IpSrtpAnswer};
use crate::ice::agent::{IceEvent, Transmit};
use crate::metrics::{Aggregators, MetricEvent};
use crate::srtp::SrtpSession;
use crate::transport::{Transport, TransportConfig, TransportEnd, TransportEvent};
use crate::{metrics, srtp};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// What the open connection shares with its handles.
pub(super) struct OpenParts {
    health: broadcast::Sender<Health>,
    closed: watch::Sender<Option<CloseReason>>,
}

/// The running attempt or connection.
pub(super) struct State {
    pub(super) runner: Runner,
    pub(super) cmd_rx: mpsc::UnboundedReceiver<Command>,
    pub(super) offer: Option<oneshot::Sender<Result<Map<String, Value>, CloseReason>>>,
    pub(super) opening: Option<oneshot::Sender<Result<NanoConnection, CloseReason>>>,
    pub(super) deadline: Option<Instant>,
    pub(super) datagrams: mpsc::UnboundedReceiver<(usize, SocketAddr, Vec<u8>)>,
    pub(super) datagram_tx: mpsc::UnboundedSender<(usize, SocketAddr, Vec<u8>)>,
    pub(super) readers: Vec<JoinHandle<()>>,
    pub(super) srtp: Option<SrtpSession>,
    pub(super) transport: Option<Transport>,
    pub(super) warning: Duration,
    pub(super) channels: HashMap<u16, mpsc::UnboundedSender<RawInbound>>,
    pub(super) stash: Vec<RawChannel>,
    pub(super) forward: Option<mpsc::UnboundedSender<RawChannel>>,
    pub(super) shared: Arc<Shared>,
    /// Set once the connection opened.
    pub(super) connection: Option<OpenParts>,
    pub(super) metrics: Option<Arc<Mutex<Aggregators>>>,
    pub(super) dropped_seen: (u64, u64),
    /// Why a [`Command::CloseLost`] closed the connection, for its close line.
    pub(super) close_trigger: Option<&'static str>,
    pub(super) done: bool,
}

impl State {
    pub(super) async fn main(&mut self) {
        self.start_readers();
        let now = Instant::now();
        self.pump(now);
        self.maybe_offer();

        while !self.done {
            let timer = self.next_timer();
            tokio::select! {
                d = self.datagrams.recv() => {
                    if let Some((base, from, data)) = d {
                        self.on_datagram(base, from, data);
                    }
                }
                c = self.cmd_rx.recv() => match c {
                    Some(c) => self.on_command(c),
                    None => self.finish(self.local_close_reason()),
                },
                _ = sleep_until(timer) => self.on_timer(),
            }
        }

        for r in &self.readers {
            r.abort();
        }
    }

    pub(super) fn start_readers(&mut self) {
        for (i, socket) in self.runner.sockets.iter().enumerate().skip(self.readers.len()) {
            let socket = socket.recv.clone();
            let tx = self.datagram_tx.clone();

            self.readers.push(tokio::spawn(async move {
                let mut buf = vec![0u8; RECEIVE_BUFFER];
                loop {
                    match socket.recv_from(&mut buf).await {
                        Ok((n, from)) => {
                            if tx.send((i, from, buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                            debug!(error = %e, "Nano socket receive reset; still receiving");
                        }
                        Err(e) => {
                            warn!(error = %e, "Nano socket receive failed; the socket stops receiving");
                            return;
                        }
                    }
                }
            }));
        }
    }

    pub(super) fn local_close_reason(&self) -> CloseReason {
        if self.connection.is_some() { CloseReason::UserEnded } else { CloseReason::UserCanceled }
    }

    pub(super) fn maybe_offer(&mut self) {
        let ready = match &self.runner.path {
            Path::Ice(agent) => agent.gathering_complete(),
            Path::Direct(_) => true,
        };

        if ready {
            if let Some(tx) = self.offer.take() {
                let _ = tx.send(Ok(self.runner.offer()));
            }
        }
    }

    pub(super) fn next_timer(&self) -> Option<Instant> {
        let mut t = self.deadline;
        let mut consider = |x: Option<std::time::Instant>| {
            if let Some(x) = x {
                let x = Instant::from_std(x);
                t = Some(t.map_or(x, |c| c.min(x)));
            }
        };

        if let Path::Ice(agent) = &self.runner.path {
            consider(agent.poll_timeout());
        }
        if let Some(tr) = &self.transport {
            consider(tr.poll_timeout());
        }

        t
    }

    pub(super) fn send_raw(&self, base: usize, to: SocketAddr, data: &[u8]) {
        let Some(socket) = self.runner.sockets.get(base) else {
            return;
        };

        match socket.send.send_to(data, to) {
            Ok(_) => metrics::record(MetricEvent::DatagramSent(data.len())),
            Err(e) => debug!(error = %e, %to, "Nano datagram not sent"),
        }
    }

    pub(super) fn on_datagram(&mut self, base: usize, from: SocketAddr, data: Vec<u8>) {
        metrics::record(MetricEvent::DatagramReceived(data.len()));
        let now = Instant::now();
        let raw_len = data.len();

        let app = match &mut self.runner.path {
            Path::Ice(agent) => agent.handle_datagram(now.into_std(), base, from, &data),
            Path::Direct(Some(remote)) if *remote == from => Some(data),
            Path::Direct(_) => None,
        };

        if let (Some(app), Some(srtp), Some(transport)) = (app, self.srtp.as_mut(), self.transport.as_mut()) {
            match srtp.unprotect(&app) {
                Ok(packet) => transport.handle_packet(now.into_std(), packet, raw_len),
                Err(e) => debug!(error = %e, "SRTP packet dropped"),
            }
        }

        self.pump(now);
    }

    pub(super) fn on_timer(&mut self) {
        let now = Instant::now();
        if self.deadline.is_some_and(|d| d <= now) {
            self.deadline = None;
            if self.connection.is_none() {
                return self.finish(CloseReason::TimeoutExceeded);
            }
        }

        if let Path::Ice(agent) = &mut self.runner.path {
            agent.handle_timeout(now.into_std());
        }
        if let Some(t) = self.transport.as_mut() {
            t.handle_timeout(now.into_std());
        }

        self.pump(now);
    }

    pub(super) fn on_command(&mut self, c: Command) {
        let now = Instant::now();
        match c {
            Command::Open(answer, reply) => {
                if !self.on_open_command(now, &answer, reply) {
                    return;
                }
            }
            Command::ChannelOpen(id) => self.open_transport_channel(id),
            Command::ChannelSend { id, mode, payload_type, body } => {
                if let Some(t) = self.transport.as_mut() {
                    if let Err(e) = t.send(id, mode, payload_type, body) {
                        warn!(channel = id, error = %e, "channel payload not sent");
                    }
                }
            }
            Command::ChannelClose(id) => {
                self.channels.remove(&id);
                if let Some(t) = self.transport.as_mut() {
                    t.close_channel(id);
                }
            }
            Command::Forward(tx) => self.forward_channels(tx),
            Command::Close => return self.close_with(now, self.local_close_reason()),
            Command::CloseLost { trigger } => {
                self.close_trigger = Some(trigger);
                return self.close_with(now, CloseReason::KeepAliveTimeout);
            }
        }

        self.pump(now);
    }

    /// `Open`: applies the answer and arms the open deadline; `false` when the command ends here
    /// (out of order, or the answer refused and the connection finished).
    fn on_open_command(
        &mut self,
        now: Instant,
        answer: &Value,
        reply: oneshot::Sender<Result<NanoConnection, CloseReason>>,
    ) -> bool {
        if self.opening.is_some() || self.offer.is_some() {
            let _ = reply.send(Err(CloseReason::Unknown("open out of order".into())));
            return false;
        }

        self.opening = Some(reply);
        if let Err(e) = self.open(now, answer) {
            self.finish(e);
            return false;
        }

        self.deadline = Some(now + self.runner.open_timeout);
        true
    }

    /// Opens channel `id` on the transport, when there is one.
    fn open_transport_channel(&mut self, id: u16) {
        if let Some(t) = self.transport.as_mut() {
            if let Err(e) = t.open_channel(id) {
                warn!(channel = id, error = %e, "channel open failed");
            }
        }
    }

    /// Hands the kept channels to their owner and forwards every later one to it.
    fn forward_channels(&mut self, tx: mpsc::UnboundedSender<RawChannel>) {
        for c in self.stash.drain(..) {
            debug!(channel = c.id, "kept channel handed to its owner");
            let _ = tx.send(c);
        }
        self.forward = Some(tx);
    }

    /// Closes the transport, sending its close packets, then finishes with `reason`.
    fn close_with(&mut self, now: Instant, reason: CloseReason) {
        if let Some(t) = self.transport.as_mut() {
            for packet in t.close(now.into_std()) {
                self.send_packet(packet);
            }
        }
        self.finish(reason);
    }

    pub(super) fn open(&mut self, now: Instant, answer: &Value) -> Result<(), CloseReason> {
        match &mut self.runner.path {
            Path::Ice(agent) => {
                let answer = IceSrtpAnswer::parse(answer).map_err(|e| CloseReason::Unknown(e.to_string()))?;
                self.warning = answer.warning_timeout;
                agent.set_remote(now.into_std(), answer.description);
            }
            Path::Direct(remote) => {
                let answer = IpSrtpAnswer::parse(answer).map_err(|e| CloseReason::Unknown(e.to_string()))?;
                let to = answer.select_address().map_err(|e| CloseReason::Unknown(e.to_string()))?;
                self.warning = answer.warning_timeout;
                self.runner.key = answer.key.clone();
                *remote = Some(to);

                let bind: SocketAddr = match to.ip() {
                    IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
                    IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
                };
                let socket = NanoSocket::bind(bind).map_err(|e| CloseReason::Unknown(e.to_string()))?;
                self.runner.sockets.push(socket);
                self.start_readers();

                self.start_transport(now, to.is_ipv4(), false);
            }
        }
        Ok(())
    }

    pub(super) fn start_transport(&mut self, now: Instant, ipv4: bool, ice: bool) {
        let socket_max = if ipv4 { SOCKET_MAX_V4 } else { SOCKET_MAX_V6 };
        let lower = socket_max - if ice { ICE_LINK_OVERHEAD } else { 0 } - srtp::OVERHEAD;

        self.srtp = Some(SrtpSession::new(&self.runner.key));
        let transport = Transport::new(
            now.into_std(),
            TransportConfig { lower_max_payload: lower, lower_errors_close: !ice, warning_timeout: self.warning },
        );

        self.shared.max_payload.store(transport.max_channel_payload(), Ordering::Relaxed);
        self.transport = Some(transport);
        debug!(lower, "Nano transport started");
    }

    pub(super) fn send_packet(&mut self, packet: srtp::RtpPacket) {
        let Some(srtp) = self.srtp.as_mut() else {
            return;
        };
        let bytes = srtp.protect(&packet);

        match &self.runner.path {
            Path::Ice(agent) => {
                if let Some(Transmit { base, to, data }) = agent.send_data(bytes) {
                    self.send_raw(base, to, &data);
                }
            }
            Path::Direct(Some(remote)) => {
                let base = self.runner.sockets.len() - 1;
                self.send_raw(base, *remote, &bytes);
            }
            Path::Direct(None) => {}
        }
    }

    /// Moves everything the layers produced: ICE events and datagrams, transport events and
    /// packets.
    pub(super) fn pump(&mut self, now: Instant) {
        if let Path::Ice(_) = self.runner.path {
            loop {
                let Path::Ice(agent) = &mut self.runner.path else { unreachable!("the path was checked to be ICE") };
                let Some(event) = agent.poll_event() else {
                    break;
                };
                match event {
                    IceEvent::GatheringComplete => {}
                    IceEvent::Selected { local, .. } => self.start_transport(now, local.is_ipv4(), true),
                    IceEvent::Failed { pair_count, candidate_count } => {
                        let reason = if self.runner.resolution_failed {
                            CloseReason::HostnameResolutionError
                        } else {
                            CloseReason::Unknown(format!(
                                "every ICE pair failed ({pair_count} pairs, {candidate_count} local candidates)"
                            ))
                        };
                        return self.finish(reason);
                    }
                    IceEvent::ServerTimedOut(_) => {}
                }
            }

            loop {
                let Path::Ice(agent) = &mut self.runner.path else { unreachable!("the path was checked to be ICE") };
                let Some(t) = agent.poll_transmit() else {
                    break;
                };
                self.send_raw(t.base, t.to, &t.data);
            }

            self.maybe_offer();
        }

        self.pump_transport(now);
    }

    pub(super) fn pump_transport(&mut self, now: Instant) {
        let Some(transport) = self.transport.as_mut() else {
            return;
        };

        let mut events = Vec::new();
        while let Some(e) = transport.poll_event() {
            events.push(e);
        }

        let mut packets = Vec::new();
        while let Some(p) = transport.poll_transmit(now.into_std()) {
            packets.push(p);
        }

        let sampled = transport.rtt().map(|rtt| (rtt, transport.bandwidth_bps()));
        let dropped = transport.dropped();
        self.shared.max_payload.store(transport.max_channel_payload(), Ordering::Relaxed);
        self.shared.fill.store(transport.send_queue_fill().to_bits(), Ordering::Relaxed);
        self.shared.bandwidth.store(transport.bandwidth_bps().to_bits(), Ordering::Relaxed);

        let sent_data = !packets.is_empty();
        for p in packets {
            self.send_packet(p);
        }

        if sent_data {
            if let Some((rtt, bandwidth)) = sampled {
                metrics::record(MetricEvent::RateSample { rtt, bandwidth });
            }
        }

        for _ in self.dropped_seen.0..dropped.0 {
            metrics::record(MetricEvent::PacketLost { timeout: false });
        }
        for _ in self.dropped_seen.1..dropped.1 {
            metrics::record(MetricEvent::PacketLost { timeout: true });
        }
        self.dropped_seen = dropped;

        for e in events {
            self.on_transport_event(e);
            if self.done {
                return;
            }
        }
    }

    pub(super) fn on_transport_event(&mut self, event: TransportEvent) {
        match event {
            TransportEvent::PathOpen => self.on_opened(),
            TransportEvent::Connected => debug!("Nano transport connected"),
            TransportEvent::ChannelCreated { id, class_name, init_info, priority, fec_type } => {
                let key = if init_info.is_empty() { class_name } else { init_info };
                let key = String::from_utf8_lossy(&key).into_owned();
                debug!(channel = id, priority, fec_type, %key, "Nano channel created");
                self.on_channel_created(id, &key);
            }
            TransportEvent::ChannelOpened(id) => self.deliver_to_channel(id, RawInbound::Opened),
            TransportEvent::ChannelData { id, payload_type, payload } => {
                self.deliver_to_channel(id, RawInbound::Data { payload_type, payload })
            }
            TransportEvent::ChannelClosed(id) => {
                self.deliver_to_channel(id, RawInbound::Closed);
                self.channels.remove(&id);
            }
            TransportEvent::MtuChanged(_) => {}
            TransportEvent::MissedPackets => self.report_health(Health::MissedPackets),
            TransportEvent::RecoveredPackets => self.report_health(Health::RecoveredPackets),
            TransportEvent::Ended(end) => {
                let opened = self.connection.is_some();
                self.finish(transport_close_reason(end, opened));
            }
        }
    }

    /// A channel the peer created: refused (and closed) when its announcement does not parse, else
    /// handed to its owner or kept until the owner asks.
    fn on_channel_created(&mut self, id: u16, key: &str) {
        let announcement = match Announcement::parse(key) {
            Ok(a) => a,
            Err(e) => {
                warn!(channel = id, error = %e, "channel announcement rejected");
                if let Some(t) = self.transport.as_mut() {
                    t.close_channel(id);
                }
                return;
            }
        };

        let (tx, rx) = mpsc::unbounded_channel();
        self.channels.insert(id, tx);
        let link = Arc::new(Link { cmd: self.runner.cmd.clone(), shared: self.shared.clone() });
        let channel = RawChannel::new(id, announcement, link, rx);
        self.hand_over_channel(id, channel);
    }

    /// Hands a new channel to its owner, or keeps it until the owner asks (also when the owner is
    /// gone).
    fn hand_over_channel(&mut self, id: u16, channel: RawChannel) {
        match &self.forward {
            Some(f) => {
                debug!(channel = id, "channel handed to its owner");
                if let Err(mpsc::error::SendError(c)) = f.send(channel) {
                    self.forward = None;
                    self.stash.push(c);
                }
            }
            None => {
                debug!(channel = id, "channel kept until its owner asks");
                self.stash.push(channel);
            }
        }
    }

    /// Tells the open connection's watchers about its packet health.
    fn report_health(&self, health: Health) {
        if let Some(c) = &self.connection {
            let _ = c.health.send(health);
        }
    }

    pub(super) fn deliver_to_channel(&mut self, id: u16, item: RawInbound) {
        if let Some(tx) = self.channels.get(&id) {
            if tx.send(item).is_err() {
                self.channels.remove(&id);
            }
        }
    }

    pub(super) fn on_opened(&mut self) {
        let Some(reply) = self.opening.take() else {
            return;
        };

        self.deadline = None;
        let (health, _) = broadcast::channel(16);
        let (closed_tx, closed) = watch::channel(None);
        let metrics = metrics::register();
        self.metrics = Some(metrics.clone());

        let connection = NanoConnection {
            kind: self.runner.kind,
            cmd: self.runner.cmd.clone(),
            health: health.clone(),
            closed,
            metrics,
        };

        info!(kind = ?self.runner.kind, "Nano connection open");
        self.connection = Some(OpenParts { health, closed: closed_tx });
        if reply.send(Ok(connection)).is_err() {
            self.finish(CloseReason::UserCanceled);
        }
    }

    /// Ends the attempt or the connection with `reason`. An attempt that never opened hands
    /// `reason` to whoever waits for its offer or its opening, which logs it; a connection that
    /// opened logs its close here.
    pub(super) fn finish(&mut self, reason: CloseReason) {
        if self.done {
            return;
        }
        self.done = true;

        if self.connection.is_some() {
            log_closed(self.runner.kind, &reason, self.close_trigger);
        }

        if let Some(tx) = self.offer.take() {
            let _ = tx.send(Err(reason.clone()));
        }
        if let Some(tx) = self.opening.take() {
            let _ = tx.send(Err(reason.clone()));
        }
        for (_, tx) in self.channels.drain() {
            let _ = tx.send(RawInbound::Closed);
        }
        if let Some(c) = &self.connection {
            let _ = c.closed.send(Some(reason));
        }
    }
}

/// The one line of an open connection's close: `warn` when it was lost or failed, `info` otherwise.
fn log_closed(kind: super::ConnectionType, reason: &CloseReason, trigger: Option<&'static str>) {
    match reason {
        CloseReason::KeepAliveTimeout | CloseReason::ClientUnknown(_) => {
            warn!(?kind, %reason, trigger, "Nano connection lost");
        }
        CloseReason::UserCanceled
        | CloseReason::TimeoutExceeded
        | CloseReason::HostnameResolutionError
        | CloseReason::None
        | CloseReason::Unknown(_)
        | CloseReason::UserEnded
        | CloseReason::PeerEnded => info!(?kind, %reason, "Nano connection closed"),
    }
}

/// The close reason of a transport end; `opened` says whether the connection had opened.
fn transport_close_reason(end: TransportEnd, opened: bool) -> CloseReason {
    match end {
        TransportEnd::Local if opened => CloseReason::UserEnded,
        TransportEnd::Local => CloseReason::UserCanceled,
        TransportEnd::PeerClosed => CloseReason::PeerEnded,
        TransportEnd::KeepAliveTimeout | TransportEnd::MtuTimeout => CloseReason::KeepAliveTimeout,
        TransportEnd::ProbeExhausted => CloseReason::TimeoutExceeded,
        TransportEnd::ReliabilityThreshold => {
            CloseReason::ClientUnknown("a reliable packet went unacknowledged".into())
        }
        TransportEnd::ProtocolError(e) if opened => CloseReason::ClientUnknown(e),
        TransportEnd::ProtocolError(e) => CloseReason::Unknown(e),
    }
}
