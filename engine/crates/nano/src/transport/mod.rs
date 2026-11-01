//! The transport between SRTP and the Nano channels, client side: MTU probe → keepalive → rate-control
//! handshake → rate controller (URCP) → channel multiplexer with its sequencer.
//!
//! [`Transport`] is sans-IO: the connection feeds it decrypted RTP packets and timer ticks and
//! sends what it returns.
//!
//! Every multi-byte field below SRTP's RTP header is little-endian. Each layer recognises its own
//! packets by RTP payload type and passes the rest up.

pub mod keepalive;
pub mod mux;
pub mod probe;
pub mod rc_handshake;
pub mod rc_header;
pub mod receiver;
pub mod sender;
pub mod stats;
pub mod urcpv2;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use self::keepalive::{KeepAlive, KeepAliveEnd, KeepAliveEvent};
use self::mux::{Mux, MuxEvent, SEQUENCER_HEADER_LEN};
use self::probe::Prober;
use self::rc_handshake::{RATE_CONTROLLER_TYPE, RcHandshake};
use self::rc_header::{MAX_HEADER_LEN, RcHeader};
use self::receiver::{Receiver, Verdict};
use self::sender::Sender;
use super::srtp::RtpPacket;

/// RTP payload types of the transport.
pub mod pt {
    pub const PROBE: u8 = 0x66;
    pub const KEEPALIVE: u8 = 0x65;
    pub const RC_HANDSHAKE: u8 = 0x64;
    pub const MUX_CONTROL: u8 = 0x61;
    /// Channel payloads unless a channel sets its own.
    pub const MUX_DATA: u8 = 0x23;
    /// Bandwidth-probe filler: acknowledged, then discarded.
    pub const FILLER: u8 = 0x68;
}

/// How the transport carries one channel message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SendMode {
    /// Sent once; delivered on arrival, possibly out of order or missing.
    Unreliable = 1,
    /// Retransmitted until acknowledged; delivered exactly once, in order.
    Reliable = 2,
    /// As reliable, and later best-effort packets wait for it.
    ReliableFence = 3,
}

/// A malformed or out-of-state packet from the phone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProtocolError(pub String);

impl ProtocolError {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

/// Why the transport ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEnd {
    /// The PC closed it.
    Local,
    /// The phone's keepalive close packet.
    PeerClosed,
    KeepAliveTimeout,
    MtuTimeout,
    ProbeExhausted,
    ReliabilityThreshold,
    /// A protocol error below the mux on a connection whose lower errors close it.
    ProtocolError(String),
}

/// What the transport reports up.
#[derive(Debug, Clone, PartialEq)]
pub enum TransportEvent {
    /// The MTU probe completed: the path is open.
    PathOpen,
    /// The rate-control handshake completed: channels can be created.
    Connected,
    ChannelCreated {
        id: u16,
        class_name: Vec<u8>,
        init_info: Vec<u8>,
        priority: f64,
        fec_type: u32,
    },
    ChannelOpened(u16),
    ChannelData {
        id: u16,
        payload_type: u8,
        payload: Vec<u8>,
    },
    ChannelClosed(u16),
    MtuChanged(usize),
    /// Nothing arrived for the warning timeout.
    MissedPackets,
    /// Traffic resumed after [`TransportEvent::MissedPackets`].
    RecoveredPackets,
    Ended(TransportEnd),
}

/// Per-connection inputs.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// The largest RTP payload SRTP and the path below accept (`L`).
    pub lower_max_payload: usize,
    /// A protocol error below the mux ends the connection (`IpSrtp`) rather than dropping the packet
    /// (`IceSrtp`).
    pub lower_errors_close: bool,
    /// The answer's `keepalive.warningtimeout`.
    pub warning_timeout: Duration,
}

pub struct Transport {
    config: TransportConfig,
    prober: Prober,
    keepalive: Option<KeepAlive>,
    handshake: Option<RcHandshake>,
    flow: Option<(Receiver, Sender)>,
    mux: Mux,
    out: VecDeque<RtpPacket>,
    events: VecDeque<TransportEvent>,
    ended: bool,
    pace_wake: Option<Instant>,
    idle: bool,
    /// A rate-controlled packet went out in the current send cycle.
    cycle_sent: bool,
    /// The pacing budget of the current send cycle (bytes).
    cycle_budget: Option<f64>,
}

impl Transport {
    /// SRTP is open: the MTU probe starts.
    pub fn new(now: Instant, config: TransportConfig) -> Self {
        let mut prober = Prober::new(now, config.lower_max_payload);
        let out = prober.drain().into();

        Self {
            config,
            prober,
            keepalive: None,
            handshake: None,
            flow: None,
            mux: Mux::default(),
            out,
            events: VecDeque::new(),
            ended: false,
            pace_wake: None,
            idle: true,
            cycle_sent: false,
            cycle_budget: None,
        }
    }

    #[cfg(feature = "test-support")]
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    #[cfg(feature = "test-support")]
    pub fn is_connected(&self) -> bool {
        self.flow.is_some()
    }

    pub fn poll_event(&mut self) -> Option<TransportEvent> {
        self.events.pop_front()
    }

    /// The largest channel payload one packet carries now.
    pub fn max_channel_payload(&self) -> usize {
        let mtu =
            self.keepalive.as_ref().map_or(self.config.lower_max_payload.min(keepalive::WORKING_MTU_CAP), |k| k.mtu());
        mtu - MAX_HEADER_LEN - SEQUENCER_HEADER_LEN
    }

    /// The pacing rate in bit/s (0 before the rate controller exists).
    pub fn bandwidth_bps(&self) -> f64 {
        self.flow.as_ref().map_or(0.0, |(_, s)| s.bandwidth_bps())
    }

    /// The smoothed round trip in seconds, once known.
    pub fn rtt(&self) -> Option<f64> {
        self.flow.as_ref().and_then(|(_, s)| s.rtt())
    }

    /// Outbound packets declared lost: `(all, by timeout)`.
    pub fn dropped(&self) -> (u64, u64) {
        self.flow.as_ref().map_or((0, 0), |(_, s)| (s.dropped, s.dropped_by_timeout))
    }

    /// How full the send path is (0–1): packets in flight against the window, plus queued bytes.
    pub fn send_queue_fill(&self) -> f64 {
        let Some((_, sender)) = &self.flow else {
            return 1.0;
        };
        let queued = self.mux.queued_bytes() as f64 / (self.max_channel_payload() as f64 * 64.0);
        (sender.window_fill() + queued).min(1.0)
    }

    /// Ends the transport; the connection logs the end with the close reason it maps it to.
    fn end(&mut self, reason: TransportEnd) {
        if !self.ended {
            self.ended = true;
            self.events.push_back(TransportEvent::Ended(reason));
        }
    }

    fn lower_error(&mut self, e: ProtocolError) {
        if self.config.lower_errors_close {
            self.end(TransportEnd::ProtocolError(e.0));
        } else {
            debug!(error = %e, "protocol error below the mux; packet dropped");
        }
    }

    fn take_layer_output(&mut self) {
        if let Some(k) = self.keepalive.as_mut() {
            self.out.extend(k.drain());
            for e in k.events() {
                match e {
                    KeepAliveEvent::MtuChanged(m) => self.events.push_back(TransportEvent::MtuChanged(m)),
                    KeepAliveEvent::MissedPackets => self.events.push_back(TransportEvent::MissedPackets),
                    KeepAliveEvent::RecoveredPackets => self.events.push_back(TransportEvent::RecoveredPackets),
                    KeepAliveEvent::End(end) => self.end(match end {
                        KeepAliveEnd::PeerClosed => TransportEnd::PeerClosed,
                        KeepAliveEnd::Timeout => TransportEnd::KeepAliveTimeout,
                        KeepAliveEnd::MtuTimeout => TransportEnd::MtuTimeout,
                    }),
                }
            }
        }

        if let Some(h) = self.handshake.as_mut() {
            self.out.extend(h.drain());
        }
        self.out.extend(self.prober.drain());

        for e in self.mux.events() {
            self.events.push_back(match e {
                MuxEvent::Created { id, class_name, init_info, priority, fec_type } => {
                    TransportEvent::ChannelCreated { id, class_name, init_info, priority, fec_type }
                }
                MuxEvent::Opened(id) => TransportEvent::ChannelOpened(id),
                MuxEvent::Data { id, payload_type, payload } => {
                    TransportEvent::ChannelData { id, payload_type, payload }
                }
                MuxEvent::Closed(id) => TransportEvent::ChannelClosed(id),
            });
        }
    }

    /// One decrypted inbound packet; `raw_len` is the UDP payload size it arrived in.
    pub fn handle_packet(&mut self, now: Instant, packet: RtpPacket, raw_len: usize) {
        if self.ended {
            return;
        }

        if let Some(k) = self.keepalive.as_mut() {
            k.on_any_packet(now, packet.payload.len());
        }

        self.receive(now, packet, raw_len);
        self.take_layer_output();
    }

    fn receive(&mut self, now: Instant, packet: RtpPacket, raw_len: usize) {
        let Some(packet) = self.through_probe(now, packet) else {
            return;
        };

        if packet.payload_type == pt::KEEPALIVE {
            let k = self.keepalive.as_mut().expect("open after the probe");
            if let Err(e) = k.on_keepalive(&packet.payload) {
                self.lower_error(e);
            }
            return;
        }

        let handshake = self.handshake.as_mut().expect("open after the probe");
        let packet = match handshake.on_packet(now, packet) {
            Err(e) => return self.lower_error(e),
            Ok(rc_handshake::Inbound::Consumed) => return,
            Ok(rc_handshake::Inbound::Up(p)) => p,
            Ok(rc_handshake::Inbound::Connected(peer, up)) => {
                if peer.rate_controller_type != RATE_CONTROLLER_TYPE {
                    warn!(
                        rate_controller_type = peer.rate_controller_type,
                        "the phone names another rate controller; sending URCPV2"
                    );
                }

                debug!(initial_send_sn = peer.initial_send_sn, "Nano rate-control handshake complete");
                self.flow = Some((Receiver::new(now, peer.initial_send_sn), Sender::new(now)));
                self.events.push_back(TransportEvent::Connected);
                match up {
                    Some(p) => p,
                    None => return,
                }
            }
        };

        self.rate_controlled(now, packet, raw_len);
    }

    /// The MTU probe's stage: the packet it passes up, opening the keep-alive and the rate-control
    /// handshake when the probe completes; `None` when it consumes or drops the packet or fails.
    fn through_probe(&mut self, now: Instant, packet: RtpPacket) -> Option<RtpPacket> {
        match self.prober.on_packet(packet) {
            Err(e) => {
                self.lower_error(e);
                None
            }
            Ok(probe::Inbound::Consumed | probe::Inbound::Dropped) => None,
            Ok(probe::Inbound::Up(p)) => Some(p),
            Ok(probe::Inbound::Opened(result, up)) => {
                debug!(probe_result = result, "MTU probe open");
                let k = KeepAlive::new(now, result, self.config.warning_timeout);
                self.events.push_back(TransportEvent::PathOpen);
                self.events.push_back(TransportEvent::MtuChanged(k.mtu()));
                self.keepalive = Some(k);
                self.handshake = Some(RcHandshake::new(now));
                up
            }
        }
    }

    fn rate_controlled(&mut self, now: Instant, packet: RtpPacket, raw_len: usize) {
        let (receiver, sender) = self.flow.as_mut().expect("connected");
        let (header, len) = match RcHeader::decode(&packet.payload) {
            Ok(h) => h,
            Err(e) => return self.lower_error(e),
        };

        let overhead = raw_len.checked_sub(packet.payload.len() - len);
        if receiver.on_packet(now, &header, overhead) == Verdict::Drop {
            return;
        }

        if let Err(e) = sender.on_header(now, &header) {
            return self.lower_error(e);
        }

        let body = &packet.payload[len..];
        if body.is_empty() {
            return;
        }
        if let Err(e) = self.mux.on_packet(packet.payload_type, packet.ssrc, packet.sequence, body) {
            debug!(error = %e, channel = packet.ssrc as u16, "mux protocol error; packet dropped");
        }
    }

    pub fn poll_timeout(&self) -> Option<Instant> {
        if self.ended {
            return None;
        }

        [
            self.prober.timeout(),
            self.keepalive.as_ref().map(KeepAlive::timeout),
            self.handshake.as_ref().and_then(RcHandshake::timeout),
            self.flow.as_ref().and_then(|(r, _)| r.timeout()),
            self.flow.as_ref().and_then(|(_, s)| s.timeout()),
            self.pace_wake,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    pub fn handle_timeout(&mut self, now: Instant) {
        if self.ended {
            return;
        }

        // The probe's error says no more than `ProbeExhausted`.
        if self.prober.on_timeout(now).is_err() {
            self.end(TransportEnd::ProbeExhausted);
            return;
        }

        if let Some(k) = self.keepalive.as_mut() {
            k.on_timeout(now);
        }
        if let Some(h) = self.handshake.as_mut() {
            h.on_timeout(now);
        }
        if let Some((_, sender)) = self.flow.as_mut() {
            // The sender's error says no more than `ReliabilityThreshold`.
            if sender.on_timeout(now).is_err() {
                self.end(TransportEnd::ReliabilityThreshold);
            }
        }

        if self.pace_wake.is_some_and(|t| t <= now) {
            self.pace_wake = None;
        }

        self.take_layer_output();
    }

    /// The next packet to send now, if any.
    pub fn poll_transmit(&mut self, now: Instant) -> Option<RtpPacket> {
        if self.ended {
            return None;
        }
        if let Some(p) = self.out.pop_front() {
            return Some(p);
        }

        let packet = self.next_rate_controlled(now);
        if let Some((_, sender)) = self.flow.as_mut() {
            match packet {
                Some(_) => self.cycle_sent = true,
                None => {
                    sender.end_cycle(std::mem::take(&mut self.cycle_sent));
                    self.cycle_budget = None;
                }
            }
        }

        packet
    }

    /// The next rate-controlled packet: a retransmission or a mux packet within the window and the
    /// pacing; else filler while the rate controller probes; else an acknowledgement-only packet.
    fn next_rate_controlled(&mut self, now: Instant) -> Option<RtpPacket> {
        let (receiver, sender) = self.flow.as_mut()?;
        let pending = sender.has_retransmission() || self.mux.has_pending();
        let filler = !pending && sender.filler_wanted();

        // The pacing grants each send opportunity (cycle) one budget, spent by the packets it sends.
        if self.cycle_budget.is_none() && (pending || filler) {
            let want = (self.mux.queued_bytes() + MAX_HEADER_LEN).max(sender::FILLER_LEN + MAX_HEADER_LEN);
            self.cycle_budget = Some(sender.allowance(now, want));
        }

        let budget = self.cycle_budget.unwrap_or(0.0);
        let mut packet = None;
        if pending && sender.window_open() {
            let want = self.mux.peek_size().unwrap_or(0) + MAX_HEADER_LEN;
            if budget >= want as f64 {
                self.idle = false;
                packet = if sender.has_retransmission() {
                    sender.send(now, None, receiver)
                } else {
                    let item = self.mux.next_packet();
                    sender.send(now, item, receiver)
                };
            } else if self.pace_wake.is_none() {
                let rate = sender.rc.rate().max(1.0);
                self.pace_wake = Some(now + Duration::from_secs_f64((want as f64 - budget).max(0.0) / rate));
            }
        } else if filler && sender.window_open() && budget > sender::FILLER_LEN as f64 {
            let item = sender::Outgoing {
                payload_type: pt::FILLER,
                ssrc: 0,
                sequence: 0,
                body: vec![0; sender::FILLER_LEN],
                reliable: false,
            };
            packet = sender.send(now, Some(item), receiver);
        }

        if let Some(p) = &packet {
            self.cycle_budget = Some(budget - p.payload.len() as f64);
        }

        if packet.is_none() && !pending && !self.idle {
            self.idle = true;
            sender.on_idle(now);
        }

        if packet.is_none() && receiver.ack_due(now) && receiver.has_feedback() {
            packet = sender.ack_only(now, receiver);
        }

        if packet.is_some() {
            if let Some(k) = self.keepalive.as_mut() {
                k.on_sent();
            }
        }

        packet
    }

    /// The owner opened channel `id`.
    pub fn open_channel(&mut self, id: u16) -> Result<(), ProtocolError> {
        let r = self.mux.open(id);
        self.take_layer_output();
        r
    }

    /// The owner closed channel `id`.
    pub fn close_channel(&mut self, id: u16) {
        self.mux.close(id);
    }

    /// Queues one channel payload.
    pub fn send(&mut self, id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ProtocolError> {
        if body.len() > self.max_channel_payload() {
            return Err(ProtocolError::new(format!(
                "channel payload of {} bytes over the {}-byte limit",
                body.len(),
                self.max_channel_payload()
            )));
        }

        self.mux.send(id, mode, payload_type, body)
    }

    /// Local teardown: Close for every channel, then the keepalive close packet. Returns the packets
    /// to send; the transport is ended afterwards.
    pub fn close(&mut self, now: Instant) -> Vec<RtpPacket> {
        if self.ended {
            return Vec::new();
        }

        let mut out = Vec::new();
        if let Some((receiver, sender)) = self.flow.as_mut() {
            for id in self.mux.channel_ids() {
                self.mux.close(id);
            }
            while let Some(c) = self.mux.next_packet() {
                if c.payload_type != pt::MUX_CONTROL {
                    continue;
                }
                out.extend(sender.send(now, Some(c), receiver));
            }
        }

        if let Some(k) = self.keepalive.as_ref() {
            out.push(k.close_packet());
        }

        self.end(TransportEnd::Local);
        out
    }
}

#[cfg(test)]
mod tests;
