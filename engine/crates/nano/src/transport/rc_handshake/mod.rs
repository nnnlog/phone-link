//! The rate-control handshake (client side), RTP payload type `0x64`.

use std::time::{Duration, Instant};

use super::sender::INITIAL_SEND_SN;
use super::{ProtocolError, pt};
use crate::srtp::RtpPacket;

const SYN: u16 = 0;
const SYNACK: u16 = 1;
const ACKOFACK: u16 = 2;
pub const PROTOCOL_VERSION: u16 = 3;
/// The PC's receive window in the SYN (log2, packets).
const LOG_RECEIVE_WINDOW: u8 = 14;
/// URCPV2.
pub const RATE_CONTROLLER_TYPE: u16 = 5;
pub const RETRY: Duration = Duration::from_millis(800);

/// What the phone's SYN said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerSyn {
    pub log_receive_window: u8,
    /// The phone's first data SN.
    pub initial_send_sn: u64,
    pub rate_controller_type: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Start,
    SynReceived { peer: PeerSyn, echoed: u16 },
    Connected,
}

/// What the handshake did with an inbound packet.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    Consumed,
    /// Connected with the phone's SYN; a non-handshake packet that completed it goes up.
    Connected(PeerSyn, Option<RtpPacket>),
    Up(RtpPacket),
}

pub struct RcHandshake {
    state: State,
    syn_times: Vec<Instant>,
    timer: Instant,
    out: Vec<RtpPacket>,
}

fn packet(payload: Vec<u8>) -> RtpPacket {
    RtpPacket { payload_type: pt::RC_HANDSHAKE, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload }
}

impl RcHandshake {
    /// Starts once the keepalive layer is open: sends the first SYN.
    pub fn new(now: Instant) -> Self {
        let mut h = Self { state: State::Start, syn_times: Vec::new(), timer: now + RETRY, out: Vec::new() };
        h.syn(now);
        h
    }

    fn syn(&mut self, now: Instant) {
        let mut p = SYN.to_le_bytes().to_vec();
        p.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        p.push(LOG_RECEIVE_WINDOW);
        p.extend_from_slice(&INITIAL_SEND_SN.to_le_bytes());
        p.extend_from_slice(&RATE_CONTROLLER_TYPE.to_le_bytes());
        p.extend_from_slice(&(self.syn_times.len() as u16).to_le_bytes());

        self.syn_times.push(now);
        self.out.push(packet(p));
    }

    fn synack(&mut self, now: Instant, echoed: u16) {
        let sent = self.syn_times[usize::from(echoed)];
        let us = now.saturating_duration_since(sent).as_micros().min(u128::from(u32::MAX)) as u32;

        let mut p = SYNACK.to_le_bytes().to_vec();
        p.extend_from_slice(&us.to_le_bytes());
        p.extend_from_slice(&echoed.to_le_bytes());
        self.out.push(packet(p));
    }

    #[cfg(feature = "test-support")]
    pub fn is_connected(&self) -> bool {
        self.state == State::Connected
    }

    pub fn drain(&mut self) -> Vec<RtpPacket> {
        std::mem::take(&mut self.out)
    }

    pub fn timeout(&self) -> Option<Instant> {
        (self.state != State::Connected).then_some(self.timer)
    }

    pub fn on_timeout(&mut self, now: Instant) {
        if now < self.timer {
            return;
        }

        match self.state {
            State::Start => self.syn(now),
            State::SynReceived { echoed, .. } => self.synack(now, echoed),
            State::Connected => return,
        }
        self.timer = now + RETRY;
    }

    pub fn on_packet(&mut self, now: Instant, packet: RtpPacket) -> Result<Inbound, ProtocolError> {
        if packet.payload_type != pt::RC_HANDSHAKE {
            return match self.state {
                State::Connected => Ok(Inbound::Up(packet)),
                State::SynReceived { peer, .. } => {
                    self.state = State::Connected;
                    Ok(Inbound::Connected(peer, Some(packet)))
                }
                State::Start => Err(ProtocolError::new("data before the rate-control handshake")),
            };
        }

        let p = &packet.payload;
        let flags =
            u16::from_le_bytes(p.get(..2).ok_or_else(|| ProtocolError::new("short RC packet"))?.try_into().expect("2"));

        match (flags, self.state) {
            (SYN, State::Start) => {
                if p.len() < 17 {
                    return Err(ProtocolError::new("short RC SYN"));
                }
                let version = u16::from_le_bytes([p[2], p[3]]);
                if version != PROTOCOL_VERSION {
                    return Err(ProtocolError::new(format!("RC protocol version {version}")));
                }
                let echoed = u16::from_le_bytes([p[15], p[16]]);
                if usize::from(echoed) >= self.syn_times.len() {
                    return Err(ProtocolError::new("retry index not previously sent"));
                }

                let peer = PeerSyn {
                    log_receive_window: p[4],
                    initial_send_sn: u64::from_le_bytes(p[5..13].try_into().expect("8")),
                    rate_controller_type: u16::from_le_bytes([p[13], p[14]]),
                };

                self.state = State::SynReceived { peer, echoed };
                self.synack(now, echoed);
                self.timer = now + RETRY;
                Ok(Inbound::Consumed)
            }
            (ACKOFACK, State::SynReceived { peer, .. }) => {
                self.state = State::Connected;
                Ok(Inbound::Connected(peer, None))
            }
            (SYN | ACKOFACK, State::Connected) => Ok(Inbound::Consumed),
            (SYN, State::SynReceived { .. }) => Ok(Inbound::Consumed),
            (SYNACK, _) => Err(ProtocolError::new("SYNACK at the client")),
            (ACKOFACK, State::Start) => Err(ProtocolError::new("ACKOFACK before SYN")),
            (f, _) => Err(ProtocolError::new(format!("RC handshake flags {f}"))),
        }
    }
}

#[cfg(test)]
mod tests;
