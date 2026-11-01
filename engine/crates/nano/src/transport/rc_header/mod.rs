//! The rate-control header that begins every mux packet, its ack vector, and the reconstruction of
//! full sequence numbers from their low 16 bits. Every multi-byte field is little-endian.

use super::ProtocolError;

pub mod flag {
    pub const ACK: u16 = 0x001;
    pub const DATA: u16 = 0x004;
    pub const ACKVEC: u16 = 0x008;
    pub const AOA: u16 = 0x010;
    pub const OVERHEADSIZE: u16 = 0x040;
    pub const DELAYACKINFO: u16 = 0x100;
    /// Parsed and ignored; neither side sets it.
    pub const EXTENSION_200: u16 = 0x200;
}

/// The receive window the PC advertises in every header (log2, packets).
pub const PC_LOG_WINDOW: u8 = 0xD;
/// Longest encoded header.
pub const MAX_HEADER_LEN: usize = 47;
/// Coded bytes an ack vector carries at most.
pub const MAX_ACK_VECTOR_BYTES: usize = 21;
/// Delayed acks one ACK reports at most.
pub const MAX_GAPS: usize = 15;

/// `ACK`: one acknowledged SN, its receive time and the receive-time gaps before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    pub sn: u16,
    /// 24-bit receive time of `sn` in 4 µs units.
    pub received_ts: u32,
    pub ack_delay_ms: u8,
    pub shift: u8,
    /// `gap[i]`: receive-time gap in µs `>> shift`, newest first.
    pub gaps: Vec<u8>,
}

/// `ACKVEC`: receive states from a missing `base_sn` on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckVector {
    pub base_sn: u16,
    /// The newest packet's 24-bit receive time and the ms since the highest SN arrived.
    pub timestamp: Option<(u32, u8)>,
    pub coded: Vec<u8>,
}

impl AckVector {
    /// Encodes `states` (`true` = received) from `base_sn` on; a run of at least 8 equal states at a
    /// byte boundary becomes run-length bytes (63 states each), anything else a 7-state bitmap. When
    /// the states do not fit in 21 bytes the vector stops there and carries no timestamp.
    pub fn encode(base_sn: u16, states: &[bool], timestamp: (u32, u8)) -> Self {
        let mut coded = Vec::new();
        let mut i = 0;
        let mut truncated = false;

        while i < states.len() {
            if coded.len() == MAX_ACK_VECTOR_BYTES {
                truncated = true;
                break;
            }

            let state = states[i];
            let run = states[i..].iter().take_while(|s| **s == state).count();
            if run >= 8 {
                let len = run.min(63);
                coded.push(0x80 | (state as u8) << 6 | len as u8);
                i += len;
            } else {
                let mut byte = 0u8;
                for (bit, s) in states[i..].iter().take(7).enumerate() {
                    byte |= (*s as u8) << bit;
                }
                coded.push(byte);
                i += 7;
            }
        }

        Self { base_sn, timestamp: (!truncated).then_some(timestamp), coded }
    }

    /// The states the vector describes, with trailing "not received" states (the padding of a last
    /// bitmap byte) removed.
    pub fn states(&self) -> Vec<bool> {
        let mut states = Vec::new();
        for byte in &self.coded {
            if byte & 0x80 != 0 {
                let state = byte & 0x40 != 0;
                states.extend(std::iter::repeat_n(state, usize::from(byte & 0x3F)));
            } else {
                states.extend((0..7).map(|bit| byte >> bit & 1 == 1));
            }
        }

        while states.last() == Some(&false) {
            states.pop();
        }
        states
    }
}

/// One decoded or to-be-encoded header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RcHeader {
    /// The sender's receive window (log2, packets): bits 12–15 of the flags word.
    pub log_window: u8,
    pub ack: Option<Ack>,
    pub overhead: Option<u8>,
    /// `maxDelayedAcks`, `delayedAckTimeoutMs`.
    pub delay_ack_info: Option<(u8, u16)>,
    pub ack_of_acks: Option<u16>,
    pub data_sn: Option<u16>,
    pub ack_vector: Option<AckVector>,
    pub extension: Option<u64>,
}

impl RcHeader {
    pub fn flags(&self) -> u16 {
        let mut f = 0;
        let mut set = |present: bool, bit: u16| {
            if present {
                f |= bit;
            }
        };

        set(self.ack.is_some(), flag::ACK);
        set(self.overhead.is_some(), flag::OVERHEADSIZE);
        set(self.delay_ack_info.is_some(), flag::DELAYACKINFO);
        set(self.ack_of_acks.is_some(), flag::AOA);
        set(self.data_sn.is_some(), flag::DATA);
        set(self.ack_vector.is_some(), flag::ACKVEC);
        set(self.extension.is_some() && (self.ack.is_some() || self.ack_vector.is_some()), flag::EXTENSION_200);
        f
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        assert!(!(self.ack.is_some() && self.ack_vector.is_some()), "ACK and ACKVEC in one header");
        let flags = self.flags();
        assert!(flags != 0, "a rate-control header with no flag is never sent");
        out.extend_from_slice(&(flags | u16::from(self.log_window & 0xF) << 12).to_le_bytes());

        if let Some(a) = &self.ack {
            assert!(a.gaps.len() <= MAX_GAPS && a.shift <= 15);
            out.extend_from_slice(&a.sn.to_le_bytes());
            push_ts(out, a.received_ts);
            out.push(a.ack_delay_ms);
            out.push(a.gaps.len() as u8 | a.shift << 4);
            out.extend_from_slice(&a.gaps);
        }
        if let Some(o) = self.overhead {
            out.push(o);
        }
        if let Some((max, timeout)) = self.delay_ack_info {
            out.push(max);
            out.extend_from_slice(&timeout.to_le_bytes());
        }
        if let Some(aoa) = self.ack_of_acks {
            out.extend_from_slice(&aoa.to_le_bytes());
        }
        if let Some(sn) = self.data_sn {
            out.extend_from_slice(&sn.to_le_bytes());
        }
        if let Some(v) = &self.ack_vector {
            assert!(v.coded.len() <= MAX_ACK_VECTOR_BYTES);
            out.extend_from_slice(&v.base_sn.to_le_bytes());
            out.push(v.coded.len() as u8 | (v.timestamp.is_some() as u8) << 7);
            if let Some((ts, delay)) = v.timestamp {
                push_ts(out, ts);
                out.push(delay);
            }
            out.extend_from_slice(&v.coded);
        }
        if flags & flag::EXTENSION_200 != 0 {
            out.extend_from_slice(&self.extension.expect("flag set from it").to_le_bytes());
        }
    }

    /// Decodes a header; returns it and its length.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), ProtocolError> {
        let mut r = Reader { data, at: 0 };
        let word = r.u16()?;
        let flags = word & 0x0FFF;
        let mut h = RcHeader { log_window: (word >> 12) as u8, ..Default::default() };

        if flags & flag::ACK != 0 && flags & flag::ACKVEC != 0 {
            return Err(ProtocolError::new("rate-control header with both ACK and ACKVEC"));
        }

        if flags & flag::ACK != 0 {
            let sn = r.u16()?;
            let received_ts = r.ts()?;
            let ack_delay_ms = r.u8()?;
            let cs = r.u8()?;
            let gaps = r.bytes(usize::from(cs & 0xF))?.to_vec();
            h.ack = Some(Ack { sn, received_ts, ack_delay_ms, shift: cs >> 4, gaps });
        }
        if flags & flag::OVERHEADSIZE != 0 {
            h.overhead = Some(r.u8()?);
        }
        if flags & flag::DELAYACKINFO != 0 {
            h.delay_ack_info = Some((r.u8()?, r.u16()?));
        }
        if flags & flag::AOA != 0 {
            h.ack_of_acks = Some(r.u16()?);
        }
        if flags & flag::DATA != 0 {
            h.data_sn = Some(r.u16()?);
        }
        if flags & flag::ACKVEC != 0 {
            let base_sn = r.u16()?;
            let lt = r.u8()?;
            let timestamp = if lt & 0x80 != 0 { Some((r.ts()?, r.u8()?)) } else { None };
            let coded = r.bytes(usize::from(lt & 0x7F))?.to_vec();
            h.ack_vector = Some(AckVector { base_sn, timestamp, coded });
        }
        if flags & flag::EXTENSION_200 != 0 && flags & (flag::ACK | flag::ACKVEC) != 0 {
            h.extension = Some(r.u64()?);
        }

        Ok((h, r.at))
    }
}

fn push_ts(out: &mut Vec<u8>, ts: u32) {
    out.extend_from_slice(&(ts as u16).to_le_bytes());
    out.push((ts >> 16) as u8);
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let out =
            self.data.get(self.at..self.at + n).ok_or_else(|| ProtocolError::new("truncated rate-control header"))?;
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, ProtocolError> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, ProtocolError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().expect("2 bytes")))
    }
    fn ts(&mut self) -> Result<u32, ProtocolError> {
        let low = u32::from(self.u16()?);
        Ok(low | u32::from(self.u8()?) << 16)
    }
    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().expect("8 bytes")))
    }
}

/// The full SN closest to `reference` whose low 16 bits are `value`.
pub fn reconstruct(value: u16, reference: u64) -> u64 {
    let v = u64::from(value);
    let low = reference & 0xFFFF;
    let mut c = (reference & !0xFFFF) | v;

    if v > low + 0x8000 {
        c = c.wrapping_sub(0x10000);
    } else if low > v + 0x8000 {
        c = c.wrapping_add(0x10000);
    }
    c
}

#[cfg(test)]
mod tests;
