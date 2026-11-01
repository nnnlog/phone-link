//! The MTU-probe handshake (client side), RTP payload type `0x66`: SYN bursts, the peer's SYNACK
//! naming the largest SYN it received, and that size as the cap for every layer above.

use std::time::{Duration, Instant};

use super::{ProtocolError, pt};
use crate::srtp::RtpPacket;

const SYN: u16 = 1;
const SYNACK: u16 = 2;
/// Retry period of bursts and SYNACKs.
pub const RETRY: Duration = Duration::from_millis(800);
/// Firings without reaching `Open` before the connection closes.
const MAX_FIRINGS: u32 = 10;
/// The first burst starts at most here.
const START_CAP: usize = 1600;
/// The smallest SYN.
const MIN_SYN: usize = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    ClientStart,
    ClientSynReceived,
    Open,
}

/// What the prober did with an inbound packet.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// A probe packet, handled.
    Consumed,
    /// Not a probe packet; dropped before `Open`.
    Dropped,
    /// Not a probe packet, `Open`: goes up.
    Up(RtpPacket),
    /// This packet opened the prober (with the probe result); a non-probe packet goes up too.
    Opened(usize, Option<RtpPacket>),
}

pub struct Prober {
    state: State,
    /// The largest payload the layer below accepts.
    lower_max: usize,
    next_burst: usize,
    peer_mtu: usize,
    timer: Instant,
    firings: u32,
    out: Vec<RtpPacket>,
}

fn probe_packet(payload: Vec<u8>) -> RtpPacket {
    RtpPacket { payload_type: pt::PROBE, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload }
}

fn syn(size: usize, last: bool) -> RtpPacket {
    let mut p = vec![0u8; size.max(3)];
    p[..2].copy_from_slice(&SYN.to_le_bytes());
    p[2] = last as u8;
    probe_packet(p)
}

impl Prober {
    /// Starts in `ClientStart` and queues the first burst.
    pub fn new(now: Instant, lower_max: usize) -> Self {
        let mut p = Self {
            state: State::ClientStart,
            lower_max,
            next_burst: lower_max.min(START_CAP),
            peer_mtu: 0,
            timer: now + RETRY,
            firings: 0,
            out: Vec::new(),
        };

        p.burst();
        p
    }

    #[cfg(feature = "test-support")]
    pub fn state(&self) -> State {
        self.state
    }

    fn burst(&mut self) {
        let s = self.next_burst;
        if s >= MIN_SYN {
            for i in 0..5 {
                self.out.push(syn(s - 16 * i, false));
            }
            self.out.push(syn(s - 80, true));
            self.next_burst = s - 100;
        } else {
            self.out.push(syn(MIN_SYN, true));
        }
    }

    fn synack(&mut self) {
        let mut p = SYNACK.to_le_bytes().to_vec();
        p.extend_from_slice(&(self.peer_mtu.min(usize::from(u16::MAX)) as u16).to_le_bytes());
        self.out.push(probe_packet(p));
    }

    /// Packets to send.
    pub fn drain(&mut self) -> Vec<RtpPacket> {
        std::mem::take(&mut self.out)
    }

    pub fn timeout(&self) -> Option<Instant> {
        (self.state != State::Open).then_some(self.timer)
    }

    /// The retry timer; `Err` once the prober gives up.
    pub fn on_timeout(&mut self, now: Instant) -> Result<(), ProtocolError> {
        if self.state == State::Open || now < self.timer {
            return Ok(());
        }

        self.firings += 1;
        if self.firings > MAX_FIRINGS {
            return Err(ProtocolError::new("MTU probe did not open"));
        }

        match self.state {
            State::ClientStart => self.burst(),
            State::ClientSynReceived => self.synack(),
            State::Open => {}
        }
        self.timer = now + RETRY;
        Ok(())
    }

    pub fn on_packet(&mut self, packet: RtpPacket) -> Result<Inbound, ProtocolError> {
        if packet.payload_type != pt::PROBE {
            return Ok(match self.state {
                State::Open => Inbound::Up(packet),
                State::ClientSynReceived => {
                    self.state = State::Open;
                    Inbound::Opened(self.peer_mtu.min(self.lower_max), Some(packet))
                }
                State::ClientStart => Inbound::Dropped,
            });
        }

        let p = &packet.payload;
        if p.len() < 2 {
            return Err(ProtocolError::new("short MTU-probe packet"));
        }

        match u16::from_le_bytes([p[0], p[1]]) {
            SYN => {
                self.peer_mtu = self.peer_mtu.max(p.len());
                if self.state == State::ClientStart {
                    self.state = State::ClientSynReceived;
                }
                self.synack();
                Ok(Inbound::Consumed)
            }
            SYNACK => {
                let mtu = p.get(2..4).ok_or_else(|| ProtocolError::new("short SYNACK"))?;
                let mtu = usize::from(u16::from_le_bytes([mtu[0], mtu[1]]));
                match self.state {
                    State::ClientStart => Err(ProtocolError::new("SYNACK before any SYN")),
                    State::ClientSynReceived => {
                        self.state = State::Open;
                        Ok(Inbound::Opened(mtu.min(self.lower_max), None))
                    }
                    State::Open => Ok(Inbound::Consumed),
                }
            }
            other => Err(ProtocolError::new(format!("unknown MTU-probe flags {other}"))),
        }
    }
}

#[cfg(test)]
mod tests;
