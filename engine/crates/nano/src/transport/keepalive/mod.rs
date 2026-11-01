//! Keepalive (client side), RTP payload type `0x65`.
//!
//! It covers the phone-driven parameter handshake, the per-tick keepalives that also grow the
//! working MTU, liveness and MTU timeouts, the close signal, and the missed/recovered-packets
//! warning.

use std::time::{Duration, Instant};

use super::{ProtocolError, pt};
use crate::srtp::RtpPacket;

pub mod flag {
    pub const PARAMETERS: u32 = 0x1;
    pub const HANDSHAKE_REPLY: u32 = 0x2;
    pub const CLOSE: u32 = 0x4;
    pub const MTU_PARAMETERS: u32 = 0x8;
    pub const SMALL_KEEPALIVE: u32 = 0x10;
}

/// The size word of a closing keepalive.
const CLOSING: u32 = 0xFFFF_FFFF;
/// The working MTU starts at most here.
pub const WORKING_MTU_CAP: usize = 0x558;
const BASE_LEN: usize = 8;

/// Why the keepalive layer ended the transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveEnd {
    PeerClosed,
    Timeout,
    MtuTimeout,
}

/// What the keepalive layer reports up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepAliveEvent {
    MtuChanged(usize),
    MissedPackets,
    RecoveredPackets,
    End(KeepAliveEnd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Before or after a reply `0x2`.
    Normal,
    /// After a reply `0x12`.
    Small,
}

#[derive(Debug, Clone, Copy)]
struct Parameters {
    interval: Duration,
    timeout: Duration,
    mtu_frequency: u32,
    mtu_probe_frequency: u32,
    mtu_timeout: Duration,
}

impl Parameters {
    fn before_handshake() -> Self {
        let timeout = Duration::from_millis(1000);
        Self {
            interval: Duration::from_millis(100),
            timeout,
            mtu_frequency: 10,
            mtu_probe_frequency: 600,
            mtu_timeout: timeout.max(Duration::from_millis(5000)),
        }
    }
}

pub struct KeepAlive {
    params: Parameters,
    mode: Mode,
    /// The probe result `P`.
    max: usize,
    /// The working MTU.
    mtu: usize,
    largest_received: u32,
    last_size_word: Option<u32>,
    last_receive: Instant,
    last_large_receive: Instant,
    next_tick: Instant,
    ticks: u64,
    sent_in_interval: bool,
    warning: Duration,
    warned: bool,
    out: Vec<RtpPacket>,
    events: Vec<KeepAliveEvent>,
}

impl KeepAlive {
    /// Starts once the MTU probe opened with result `probe_result`; `warning` is the answer's
    /// `keepalive.warningtimeout`.
    pub fn new(now: Instant, probe_result: usize, warning: Duration) -> Self {
        let params = Parameters::before_handshake();
        Self {
            params,
            mode: Mode::Normal,
            max: probe_result,
            mtu: probe_result.min(WORKING_MTU_CAP),
            largest_received: 0,
            last_size_word: None,
            last_receive: now,
            last_large_receive: now,
            next_tick: now + params.interval,
            ticks: 0,
            sent_in_interval: false,
            warning,
            warned: false,
            out: Vec::new(),
            events: Vec::new(),
        }
    }

    /// The working MTU: the largest RTP payload the layers above may send.
    pub fn mtu(&self) -> usize {
        self.mtu
    }

    pub fn drain(&mut self) -> Vec<RtpPacket> {
        std::mem::take(&mut self.out)
    }

    pub fn events(&mut self) -> Vec<KeepAliveEvent> {
        std::mem::take(&mut self.events)
    }

    /// Anything (data included) went out.
    pub fn on_sent(&mut self) {
        self.sent_in_interval = true;
    }

    fn keepalive(&mut self, flags: u32, padded_to: usize) {
        let mut p = Vec::with_capacity(padded_to.max(BASE_LEN));
        p.extend_from_slice(&self.largest_received.to_le_bytes());
        p.extend_from_slice(&flags.to_le_bytes());
        p.resize(padded_to.max(BASE_LEN), 0);

        self.last_size_word = Some(self.largest_received);
        self.out.push(RtpPacket {
            payload_type: pt::KEEPALIVE,
            marker: false,
            timestamp: 0,
            ssrc: 0,
            sequence: 0,
            payload: p,
        });
    }

    /// The close packet sent on a local close.
    pub fn close_packet(&self) -> RtpPacket {
        let mut p = CLOSING.to_le_bytes().to_vec();
        p.extend_from_slice(&flag::CLOSE.to_le_bytes());
        RtpPacket { payload_type: pt::KEEPALIVE, marker: false, timestamp: 0, ssrc: 0, sequence: 0, payload: p }
    }

    fn grown(&self) -> usize {
        ((self.mtu as f64 * 1.1) as usize).min(self.max)
    }

    /// Any packet arrived (`size` = its RTP payload length).
    pub fn on_any_packet(&mut self, now: Instant, size: usize) {
        self.last_receive = now;
        let size = size as u32;
        if size >= self.largest_received {
            self.largest_received = size;
            self.last_large_receive = now;
        }

        if self.warned {
            self.warned = false;
            self.events.push(KeepAliveEvent::RecoveredPackets);
        }
    }

    /// A keepalive packet arrived (after [`Self::on_any_packet`]).
    pub fn on_keepalive(&mut self, payload: &[u8]) -> Result<(), ProtocolError> {
        if payload.len() < BASE_LEN {
            return Err(ProtocolError::new("short keepalive"));
        }

        let word = u32::from_le_bytes(payload[..4].try_into().expect("4 bytes"));
        let flags = u32::from_le_bytes(payload[4..8].try_into().expect("4 bytes"));

        if word == CLOSING || flags & flag::CLOSE != 0 {
            self.events.push(KeepAliveEvent::End(KeepAliveEnd::PeerClosed));
            return Ok(());
        }

        if word as usize > self.mtu {
            self.mtu = (word as usize).min(self.max);
            self.events.push(KeepAliveEvent::MtuChanged(self.mtu));
        }

        if flags & flag::PARAMETERS != 0 {
            let u64_at = |at: usize| payload.get(at..at + 8).map(|b| u64::from_le_bytes(b.try_into().expect("8")));
            let u32_at = |at: usize| payload.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4")));
            let (Some(interval), Some(timeout)) = (u64_at(8), u64_at(16)) else {
                return Err(ProtocolError::new("keepalive parameters truncated"));
            };

            self.params.interval = Duration::from_millis(interval.max(1));
            self.params.timeout = Duration::from_millis(timeout);
            let reply = if flags & flag::MTU_PARAMETERS != 0 {
                let (Some(freq), Some(probe), Some(mtu_timeout)) = (u32_at(24), u32_at(28), u64_at(32)) else {
                    return Err(ProtocolError::new("keepalive MTU parameters truncated"));
                };
                self.params.mtu_frequency = freq.max(1);
                self.params.mtu_probe_frequency = probe.max(1);
                self.params.mtu_timeout = Duration::from_millis(mtu_timeout);
                self.mode = Mode::Small;
                flag::HANDSHAKE_REPLY | flag::SMALL_KEEPALIVE
            } else {
                self.mode = Mode::Normal;
                flag::HANDSHAKE_REPLY
            };

            self.keepalive(reply, BASE_LEN);
        }
        Ok(())
    }

    pub fn timeout(&self) -> Instant {
        let mut t = self.next_tick.min(self.last_receive + self.params.timeout);
        if self.mode == Mode::Small {
            t = t.min(self.last_large_receive + self.params.mtu_timeout);
        }
        if !self.warned {
            t = t.min(self.last_receive + self.warning);
        }
        t
    }

    pub fn on_timeout(&mut self, now: Instant) {
        if now >= self.last_receive + self.params.timeout {
            self.events.push(KeepAliveEvent::End(KeepAliveEnd::Timeout));
            return;
        }
        if self.mode == Mode::Small && now >= self.last_large_receive + self.params.mtu_timeout {
            self.events.push(KeepAliveEvent::End(KeepAliveEnd::MtuTimeout));
            return;
        }

        if !self.warned && now >= self.last_receive + self.warning {
            self.warned = true;
            self.events.push(KeepAliveEvent::MissedPackets);
        }

        while now >= self.next_tick {
            self.next_tick += self.params.interval;
            self.tick();
        }
    }

    fn tick(&mut self) {
        self.ticks += 1;
        match self.mode {
            Mode::Small => {
                let mut sent = false;
                if self.ticks % u64::from(self.params.mtu_frequency) == 0 {
                    self.keepalive(0, self.mtu);
                    sent = true;
                }
                if self.ticks % u64::from(self.params.mtu_probe_frequency) == 0 && self.mtu < self.max {
                    self.keepalive(0, self.grown());
                    sent = true;
                }
                if !sent && (self.last_size_word != Some(self.largest_received) || !self.sent_in_interval) {
                    self.keepalive(0, BASE_LEN);
                }
            }
            Mode::Normal => {
                if self.mtu < self.max {
                    self.keepalive(0, self.grown());
                }
                self.keepalive(0, self.mtu);
            }
        }

        self.sent_in_interval = false;
    }
}

#[cfg(test)]
mod tests;
