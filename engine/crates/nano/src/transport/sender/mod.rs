//! Outbound flow control: data SNs, the send window, acknowledgement processing, loss detection,
//! retransmission of reliable packets with new SNs, `AOA`, `DELAYACKINFO`, and the URCPV2 pacing.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use super::ProtocolError;
use super::rc_header::{Ack, AckVector, PC_LOG_WINDOW, RcHeader, reconstruct};
use super::receiver::Receiver;
use super::urcpv2::{DELAYED_ACK_TIMEOUT_MS, Fate, PacketEvent, Urcpv2};
use crate::srtp::RtpPacket;

/// The PC's first data SN (its `initialSendSN`).
pub const INITIAL_SEND_SN: u64 = 100;
/// The peer's receive window until its first header arrives.
const INITIAL_PEER_LOG_WINDOW: u8 = 12;
const MAX_LOG_WINDOW: u8 = 14;
/// A reliable packet sent more often than this closes the connection.
pub const RETRANSMISSION_LIMIT: u32 = 50;
/// `DELAYACKINFO` goes out on packets 0, 16, 32, … of every rate-controlled packet sent.
const DELAY_ACK_INFO_EVERY: u64 = 16;
/// Loss deadlines are checked this much early.
const LOSS_SLACK: Duration = Duration::from_millis(2);
/// The windowed maximum of RTT samples behind the loss deadline: 5 accumulators over 120 s, 24 s apart.
const RTT_WINDOW: f64 = 120.0;
const RTT_STAGGER: f64 = 24.0;
/// Filler (bandwidth-probe) packets: payload size, and the budget that must remain to send one.
pub const FILLER_LEN: usize = 1000;
/// 24-bit timestamps in 4 µs units wrap every 2^26 µs.
const TS_WRAP_US: i64 = 1 << 26;

/// One packet the layers above hand the rate controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub payload_type: u8,
    pub ssrc: u32,
    /// The descriptor sequence number (mux data: the channel SN's low 16 bits).
    pub sequence: u16,
    /// Bytes after the rate-control header.
    pub body: Vec<u8>,
    /// Send mode 2 or 3: retransmitted until acknowledged.
    pub reliable: bool,
}

struct InFlight {
    item: Outgoing,
    sent_at: Instant,
    size: usize,
    in_flight_at_send: usize,
    attempts: u32,
    deadline: Instant,
}

/// What an incoming header did to the send side.
#[derive(Debug, Default)]
pub struct AckOutcome {
    /// Packets acknowledged or declared lost.
    pub events: Vec<PacketEvent>,
}

/// The send side of one connection.
pub struct Sender {
    t0: Instant,
    next_sn: u64,
    outstanding: BTreeMap<u64, InFlight>,
    retransmit: VecDeque<(Outgoing, u32)>,
    peer_log_window: u8,
    /// RTT samples (send to acknowledgement, seconds); the loss deadline uses their windowed maximum.
    rtt_max: super::stats::Staggered,
    /// `max(last cumulative ACK SN + 1, last ack-vector base)`: what the peer has acknowledged in order.
    peer_floor: u64,
    /// Rate-controlled packets sent, data and acknowledgement-only alike.
    packets: u64,
    /// The highest SN sent when the last send cycle ended: filler waits until the peer acknowledged it.
    fill_marker: Option<u64>,
    /// The last unwrapped peer receive time (µs).
    peer_clock: Option<i64>,
    pub rc: Urcpv2,
    /// Outbound packets declared lost, in total and by timeout.
    pub dropped: u64,
    pub dropped_by_timeout: u64,
}

impl Sender {
    pub fn new(now: Instant) -> Self {
        Self {
            t0: now,
            next_sn: INITIAL_SEND_SN,
            outstanding: BTreeMap::new(),
            retransmit: VecDeque::new(),
            peer_log_window: INITIAL_PEER_LOG_WINDOW,
            rtt_max: super::stats::Staggered::new(5, RTT_WINDOW, RTT_STAGGER),
            peer_floor: INITIAL_SEND_SN,
            packets: 0,
            fill_marker: None,
            peer_clock: None,
            rc: Urcpv2::new(0.0),
            dropped: 0,
            dropped_by_timeout: 0,
        }
    }

    fn secs(&self, t: Instant) -> f64 {
        t.saturating_duration_since(self.t0).as_secs_f64()
    }

    fn window(&self) -> usize {
        (1usize << self.peer_log_window.min(MAX_LOG_WINDOW)) - 2
    }

    /// Room for another data packet in the send window.
    pub fn window_open(&self) -> bool {
        self.outstanding.len() < self.window()
    }

    /// Packets in flight relative to the window (0–1).
    pub fn window_fill(&self) -> f64 {
        self.outstanding.len() as f64 / self.window() as f64
    }

    /// A reliable packet waits for retransmission.
    pub fn has_retransmission(&self) -> bool {
        !self.retransmit.is_empty()
    }

    /// Bytes the pacing allows for a packet of `want` bytes now.
    pub fn allowance(&mut self, now: Instant, want: usize) -> f64 {
        let t = self.secs(now);
        self.rc.allowance(t, want)
    }

    pub fn on_idle(&mut self, now: Instant) {
        let t = self.secs(now);
        self.rc.on_idle(t);
    }

    fn highest_sent(&self) -> u64 {
        self.next_sn - 1
    }

    /// The pacing rate in bit/s.
    pub fn bandwidth_bps(&self) -> f64 {
        self.rc.rate() * 8.0
    }

    /// The rate controller's round trip, seconds, once it has a sample.
    pub fn rtt(&self) -> Option<f64> {
        self.rtt_max.max_value().map(|_| self.rc.rtt())
    }

    /// `500·attempts` ms without an RTT sample in the window, else the windowed maximum RTT ×
    /// `attempts + 1`.
    fn packet_timeout(&self, attempts: u32) -> Duration {
        match self.rtt_max.max_value() {
            None => Duration::from_millis(500 * u64::from(attempts)),
            Some(rtt) => Duration::from_secs_f64(rtt.max(0.0) * f64::from(attempts + 1)),
        }
    }

    /// The header fields every rate-controlled packet carries: the log window, the receive side's
    /// feedback, `AOA` while the lowest outstanding SN is past what the peer acknowledged in order,
    /// and `DELAYACKINFO` on packets 0, 16, 32, ….
    fn common_header(&mut self, now: Instant, receiver: &mut Receiver) -> RcHeader {
        let mut h = RcHeader { log_window: PC_LOG_WINDOW, ..Default::default() };
        receiver.fill(now, &mut h);

        let lowest = self.outstanding.keys().next().copied().unwrap_or(self.next_sn);
        if lowest > self.peer_floor {
            h.ack_of_acks = Some(lowest as u16);
        }
        h
    }

    /// Counts one rate-controlled packet whose header has flags; adds `DELAYACKINFO` when due.
    fn count_packet(&mut self, h: &mut RcHeader) {
        if self.packets % DELAY_ACK_INFO_EVERY == 0 {
            h.delay_ack_info = Some((self.rc.max_delayed_acks(), DELAYED_ACK_TIMEOUT_MS));
        }
        self.packets += 1;
    }

    /// Filler may go out: the rate controller has no path-capacity estimate yet, and the peer has
    /// acknowledged everything sent before the last send cycle ended (one round per RTT).
    pub fn filler_wanted(&self) -> bool {
        !self.rc.is_started() && self.fill_marker.is_none_or(|m| self.peer_floor > m)
    }

    /// The end of a send cycle that sent `sent` packets.
    pub fn end_cycle(&mut self, sent: bool) {
        if sent {
            self.fill_marker = Some(self.highest_sent());
        }
    }

    /// Builds the next data packet from `item` (or the retransmission queue when `item` is `None`).
    pub fn send(&mut self, now: Instant, item: Option<Outgoing>, receiver: &mut Receiver) -> Option<RtpPacket> {
        let (item, attempts) = match item {
            Some(i) => (i, 1),
            None => {
                let (i, a) = self.retransmit.pop_front()?;
                (i, a + 1)
            }
        };

        let sn = self.next_sn;
        let mut h = self.common_header(now, receiver);
        self.next_sn += 1;
        h.data_sn = Some(sn as u16);
        self.count_packet(&mut h);

        let mut payload = Vec::with_capacity(item.body.len() + 16);
        h.encode(&mut payload);
        payload.extend_from_slice(&item.body);

        let size = payload.len();
        let in_flight_at_send = self.outstanding.values().map(|f| f.size).sum();
        self.rc.on_sent(size);
        let packet = RtpPacket {
            payload_type: item.payload_type,
            marker: false,
            timestamp: 0,
            ssrc: item.ssrc,
            sequence: item.sequence,
            payload,
        };

        // Deadlines never decrease along the SNs in flight.
        let previous = self.outstanding.values().next_back().map(|f| f.deadline);
        let deadline = (now + self.packet_timeout(attempts)).max(previous.unwrap_or(now));
        self.outstanding.insert(sn, InFlight { item, sent_at: now, size, in_flight_at_send, attempts, deadline });
        Some(packet)
    }

    /// An acknowledgement-only packet: payload type `0x23`, SSRC 0, the rate-control header without
    /// `DATA` and nothing after it; sent once.
    pub fn ack_only(&mut self, now: Instant, receiver: &mut Receiver) -> Option<RtpPacket> {
        let mut h = self.common_header(now, receiver);
        if h.flags() == 0 {
            return None;
        }

        self.count_packet(&mut h);
        let mut payload = Vec::with_capacity(32);
        h.encode(&mut payload);
        Some(RtpPacket {
            payload_type: super::pt::MUX_DATA,
            marker: false,
            timestamp: 0,
            ssrc: 0,
            sequence: 0,
            payload,
        })
    }

    /// When the earliest loss deadline is due (checked 2 ms early).
    pub fn timeout(&self) -> Option<Instant> {
        self.outstanding.values().map(|f| f.deadline).min().map(|d| d.checked_sub(LOSS_SLACK).unwrap_or(d))
    }

    /// Declares overdue packets lost: reliable ones go back to the queue (a packet past the
    /// retransmission limit fails the connection), unreliable ones are given up.
    pub fn on_timeout(&mut self, now: Instant) -> Result<AckOutcome, ProtocolError> {
        let overdue: Vec<u64> =
            self.outstanding.iter().filter(|(_, f)| f.deadline < now + LOSS_SLACK).map(|(sn, _)| *sn).collect();
        let mut out = AckOutcome::default();

        for sn in overdue {
            let f = self.outstanding.remove(&sn).expect("listed");
            self.dropped += 1;
            self.dropped_by_timeout += 1;
            out.events.push(PacketEvent {
                sn,
                send_time: self.secs(f.sent_at),
                size: f.size,
                in_flight_at_send: f.in_flight_at_send,
                fate: Fate::Lost,
            });

            if f.item.reliable {
                if f.attempts >= RETRANSMISSION_LIMIT {
                    return Err(ProtocolError::new("a reliable packet was sent 50 times without acknowledgement"));
                }
                self.retransmit.push_back((f.item, f.attempts));
            }
        }

        if !out.events.is_empty() {
            let t = self.secs(now);
            self.rc.on_batch(t, &out.events);
        }
        Ok(out)
    }

    fn unwrap_peer_time(&mut self, ts: u32) -> f64 {
        let v = i64::from(ts) << 2;
        let t = match self.peer_clock {
            None => v,
            Some(last) => {
                let mut c = (last & !(TS_WRAP_US - 1)) | v;
                if c - last > TS_WRAP_US / 2 {
                    c -= TS_WRAP_US;
                } else if last - c > TS_WRAP_US / 2 {
                    c += TS_WRAP_US;
                }
                c
            }
        };

        self.peer_clock = Some(self.peer_clock.map_or(t, |l| l.max(t)));
        t as f64 / 1e6
    }

    /// Moves `sn` to acknowledged; its RTT sample is the time from sending to now.
    fn acknowledge(&mut self, now: Instant, sn: u64, peer_receive: Option<f64>) -> Option<PacketEvent> {
        let f = self.outstanding.remove(&sn)?;
        let rtt = now.saturating_duration_since(f.sent_at).as_secs_f64();
        let t = self.secs(now);
        self.rtt_max.add(t, rtt);

        Some(PacketEvent {
            sn,
            send_time: self.secs(f.sent_at),
            size: f.size,
            in_flight_at_send: f.in_flight_at_send,
            fate: Fate::Acked { peer_receive, rtt: Some(rtt), ack_time: t },
        })
    }

    /// Applies the peer's log window, `ACK` and `ACKVEC` of an incoming header.
    pub fn on_header(&mut self, now: Instant, header: &RcHeader) -> Result<AckOutcome, ProtocolError> {
        self.peer_log_window = header.log_window;
        let mut out = AckOutcome::default();

        if let Some(ack) = &header.ack {
            self.on_ack(now, ack, &mut out)?;
        }
        if let Some(vector) = &header.ack_vector {
            self.on_ack_vector(now, vector, &mut out)?;
        }

        if !out.events.is_empty() {
            let t = self.secs(now);
            self.rc.on_batch(t, &out.events);
        }
        Ok(out)
    }

    fn on_ack(&mut self, now: Instant, ack: &Ack, out: &mut AckOutcome) -> Result<(), ProtocolError> {
        let highest = self.highest_sent();
        let sn = reconstruct(ack.sn, highest);
        if sn > highest {
            return Err(ProtocolError::new("ACK for an SN never sent"));
        }

        let peer_time = self.unwrap_peer_time(ack.received_ts);
        // Receive times of sn, sn − 1, … from the gaps.
        let mut times = vec![peer_time];
        for gap in &ack.gaps {
            let previous = times.last().copied().expect("non-empty") - f64::from(u32::from(*gap) << ack.shift) / 1e6;
            times.push(previous);
        }

        let acked: Vec<u64> = self.outstanding.range(..=sn).map(|(s, _)| *s).collect();
        for s in acked {
            let receive = (sn - s).try_into().ok().and_then(|i: usize| times.get(i).copied());
            if let Some(e) = self.acknowledge(now, s, receive) {
                out.events.push(e);
            }
        }

        self.peer_floor = self.peer_floor.max(sn + 1);
        Ok(())
    }

    fn on_ack_vector(&mut self, now: Instant, vector: &AckVector, out: &mut AckOutcome) -> Result<(), ProtocolError> {
        let highest = self.highest_sent();
        let base = reconstruct(vector.base_sn, highest);
        let states = vector.states();

        if states.first() == Some(&true) {
            return Err(ProtocolError::new("ack vector starts with a received SN"));
        }
        if base + states.len() as u64 > highest + 1 {
            return Err(ProtocolError::new("ack vector beyond the highest SN sent"));
        }

        let newest = vector.timestamp.map(|(ts, _)| self.unwrap_peer_time(ts));
        let last_received = states.iter().rposition(|s| *s).map(|i| base + i as u64);
        for (i, received) in states.iter().enumerate() {
            let s = base + i as u64;
            if *received {
                let receive = if Some(s) == last_received { newest } else { None };
                if let Some(e) = self.acknowledge(now, s, receive) {
                    out.events.push(e);
                }
            }
        }

        self.peer_floor = self.peer_floor.max(base);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
