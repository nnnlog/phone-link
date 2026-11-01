//! Inbound flow control: which mux packets to deliver and the acknowledgements that keep the phone's
//! sender at full rate.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::rc_header::{Ack, AckVector, MAX_GAPS, RcHeader, reconstruct};

/// SNs the receiver tracks beyond its last acknowledgement at most.
const RING_LIMIT: u64 = 1 << 14;
/// The delayed-ACK timer's floor.
const DELAYED_ACK_FLOOR: Duration = Duration::from_millis(10);
/// An `OVERHEADSIZE` goes out at least on every 64th ACK.
const OVERHEAD_EVERY: u32 = 64;

/// Whether a packet's payload goes up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Deliver,
    Drop,
}

/// The receive side of one connection.
pub struct Receiver {
    /// Origin of the 4 µs receive timestamps.
    t0: Instant,
    /// Lowest SN not received in order.
    next: u64,
    /// `next` at the last acknowledgement.
    acked: u64,
    /// Highest SN received; `None` before any.
    highest: Option<u64>,
    /// Receive times of SNs `acked …`, `None` = missing.
    ring: VecDeque<Option<Instant>>,
    /// Arrival time of the most recently received packet.
    newest_arrival: Option<Instant>,
    /// Out-of-order duplicates below `next` to acknowledge individually.
    reacks: VecDeque<(u64, Instant)>,
    /// The hole state changed since the vector was last sent.
    vector_dirty: bool,
    /// The peer's delayed-ACK policy; `None` before its first `DELAYACKINFO`.
    policy: Option<(u64, Duration)>,
    timer: Option<Instant>,
    immediate: bool,
    overhead: u8,
    overhead_sent: u8,
    acks_sent: u32,
}

impl Receiver {
    /// `initial` is the phone's `initialSendSN` from the RC handshake.
    pub fn new(now: Instant, initial: u64) -> Self {
        Self {
            t0: now,
            next: initial,
            acked: initial,
            highest: None,
            ring: VecDeque::new(),
            newest_arrival: None,
            reacks: VecDeque::new(),
            vector_dirty: false,
            policy: None,
            timer: None,
            immediate: false,
            overhead: 0,
            overhead_sent: 0,
            acks_sent: 0,
        }
    }

    fn slot(&mut self, sn: u64) -> Option<&mut Option<Instant>> {
        let i = sn.checked_sub(self.acked)? as usize;
        if self.ring.len() <= i {
            self.ring.resize(i + 1, None);
        }
        self.ring.get_mut(i)
    }

    fn time_of(&self, sn: u64) -> Option<Instant> {
        sn.checked_sub(self.acked).and_then(|i| self.ring.get(i as usize).copied().flatten())
    }

    fn received(&self, sn: u64) -> bool {
        self.time_of(sn).is_some()
    }

    fn advance_next(&mut self) {
        while let Some(h) = self.highest {
            if self.next <= h && self.received(self.next) {
                self.next += 1;
            } else {
                break;
            }
        }
    }

    fn has_hole(&self) -> bool {
        self.highest.is_some_and(|h| h >= self.next)
    }

    #[cfg(feature = "test-support")]
    /// Reference for 16-bit `dataSN` and `ackOfAcksSN`.
    pub fn reference(&self) -> u64 {
        self.next
    }

    /// Applies the parts of `header` that concern this side (`DELAYACKINFO`, `AOA`) and the packet's
    /// own `DATA`; `raw_size − payload_size` is the overhead the next ACK reports.
    pub fn on_packet(&mut self, now: Instant, header: &RcHeader, overhead: Option<usize>) -> Verdict {
        if let Some((max, timeout)) = header.delay_ack_info {
            self.policy = Some((u64::from(max), Duration::from_millis(u64::from(timeout))));
        }
        if let Some(o) = overhead.and_then(|o| u8::try_from(o).ok()) {
            self.overhead = o;
        }
        if let Some(aoa) = header.ack_of_acks {
            self.on_ack_of_acks(reconstruct(aoa, self.next));
        }

        let Some(sn) = header.data_sn.map(|v| reconstruct(v, self.next)) else {
            return Verdict::Deliver;
        };
        let verdict = self.on_data(now, sn);
        self.schedule(now);
        verdict
    }

    fn on_data(&mut self, now: Instant, sn: u64) -> Verdict {
        if sn < self.next {
            self.reacks.push_back((sn, now));
            return Verdict::Deliver;
        }
        if sn - self.acked >= RING_LIMIT {
            return Verdict::Drop;
        }

        let fresh = match self.highest {
            Some(h) if sn <= h => {
                if self.received(sn) {
                    return Verdict::Drop;
                }
                false
            }
            _ => true,
        };

        *self.slot(sn).expect("sn ≥ acked") = Some(now);
        if fresh {
            self.highest = Some(sn);
        }
        self.newest_arrival = Some(now);
        self.advance_next();
        self.vector_dirty = true;
        Verdict::Deliver
    }

    fn on_ack_of_acks(&mut self, aoa: u64) {
        let a = aoa.max(self.acked);
        let moved = aoa > self.next;

        if moved {
            let mut n = a;
            let top = self.highest.map_or(a, |h| h + 1);
            while n < top && self.received(n) {
                n += 1;
            }
            self.next = n.max(a);
            self.advance_next();
            self.vector_dirty = true;
            self.immediate = true;
        }

        let drop = (a - self.acked) as usize;
        self.ring.drain(..drop.min(self.ring.len()));
        self.acked = a;
    }

    fn schedule(&mut self, now: Instant) {
        let pending = self.next - self.acked;
        let urgent = self.has_hole()
            || !self.reacks.is_empty()
            || match self.policy {
                Some((max, _)) => pending > max + 1,
                None => pending >= 2,
            };

        if urgent {
            self.immediate = true;
            return;
        }
        if pending == 0 || self.timer.is_some() {
            return;
        }

        let delay = match self.policy {
            Some((_, timeout)) => {
                let age = self.time_of(self.acked).map_or(Duration::ZERO, |t| now.saturating_duration_since(t));
                timeout.saturating_sub(age).max(DELAYED_ACK_FLOOR)
            }
            None => DELAYED_ACK_FLOOR,
        };
        self.timer = Some(now + delay);
    }

    /// An acknowledgement must go out now.
    pub fn ack_due(&self, now: Instant) -> bool {
        self.immediate || self.timer.is_some_and(|t| t <= now)
    }

    /// When the delayed-ACK timer fires.
    pub fn timeout(&self) -> Option<Instant> {
        if self.immediate { None } else { self.timer }
    }

    /// Something remains to acknowledge on the next outgoing packet.
    pub fn has_feedback(&self) -> bool {
        self.next != self.acked || (self.has_hole() && self.vector_dirty) || !self.reacks.is_empty()
    }

    fn ts(&self, t: Instant) -> u32 {
        ((t.saturating_duration_since(self.t0).as_micros() >> 2) & 0xFF_FFFF) as u32
    }

    fn delay_ms(now: Instant, t: Instant) -> u8 {
        now.saturating_duration_since(t).as_millis().min(255) as u8
    }

    /// Writes the feedback of the next outgoing packet into `header` (at most one of `ACK`/`ACKVEC`).
    pub fn fill(&mut self, now: Instant, header: &mut RcHeader) {
        let mut wrote = false;

        if self.next != self.acked {
            let ack_sn = self.next - 1;
            let t = self.time_of(ack_sn).expect("SNs below next were received");
            let count = (ack_sn - self.acked).min(MAX_GAPS as u64);
            let raw: Vec<u64> = (0..count)
                .map(|i| {
                    let newer = self.time_of(ack_sn - i);
                    let older = self.time_of(ack_sn - i - 1);
                    match (newer, older) {
                        (Some(n), Some(o)) => n.saturating_duration_since(o).as_micros() as u64,
                        _ => 0,
                    }
                })
                .collect();

            let max = raw.iter().copied().max().unwrap_or(0);
            let shift = (0..=15u8).find(|s| max >> s <= 255).unwrap_or(15);
            header.ack = Some(Ack {
                sn: ack_sn as u16,
                received_ts: self.ts(t),
                ack_delay_ms: Self::delay_ms(now, t),
                shift,
                gaps: raw.iter().map(|g| (g >> shift).min(255) as u8).collect(),
            });

            let drop = (self.next - self.acked) as usize;
            self.ring.drain(..drop.min(self.ring.len()));
            self.acked = self.next;
            wrote = true;
        } else if self.has_hole() && self.vector_dirty {
            let highest = self.highest.expect("a hole has a highest");
            let states: Vec<bool> = (self.next..=highest).map(|sn| self.received(sn)).collect();
            let newest = self.newest_arrival.unwrap_or(now);
            let highest_time = self.time_of(highest).unwrap_or(now);
            header.ack_vector = Some(AckVector::encode(
                self.next as u16,
                &states,
                (self.ts(newest), Self::delay_ms(now, highest_time)),
            ));
            self.vector_dirty = false;
            wrote = true;
        } else if let Some((sn, t)) = self.reacks.pop_front() {
            header.ack = Some(Ack {
                sn: sn as u16,
                received_ts: self.ts(t),
                ack_delay_ms: Self::delay_ms(now, t),
                shift: 0,
                gaps: Vec::new(),
            });
            wrote = true;
        }

        if wrote {
            self.acks_sent += 1;
            if self.overhead != 0 && (self.overhead != self.overhead_sent || self.acks_sent % OVERHEAD_EVERY == 0) {
                header.overhead = Some(self.overhead);
                self.overhead_sent = self.overhead;
            }
        }

        self.immediate = !self.reacks.is_empty();
        self.timer = None;
        if self.next != self.acked {
            self.schedule(now);
        }
    }
}

#[cfg(test)]
mod tests;
