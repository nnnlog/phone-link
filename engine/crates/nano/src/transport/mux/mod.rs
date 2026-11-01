//! The client channel multiplexer: mux control packets (create, open, close), the channel table,
//! the per-channel sequencer, and the priority-weighted send order across channels.

use std::collections::{BTreeMap, VecDeque};

use tracing::{debug, warn};

use super::rc_header::reconstruct;
use super::sender::Outgoing;
use super::{ProtocolError, SendMode, pt};

mod control {
    pub(super) const CREATE: u32 = 2;
    pub(super) const OPEN: u32 = 3;
    pub(super) const CLOSE: u32 = 4;
}

/// Longest class name a create request may carry.
const MAX_NAME: usize = 256;
/// Control packets kept for channels not created yet.
const MAX_PARKED: usize = 10;
pub const SEQUENCER_HEADER_LEN: usize = 8;
const DEFAULT_PRIORITY: f64 = 1.0;
const SN_MASK: u64 = (1 << 48) - 1;

/// A decoded mux control packet.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Create { name: Vec<u8>, fec_type: u32, init_info: Vec<u8>, priority: f64 },
    Open { init_info: Vec<u8>, priority: f64 },
    Close,
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtocolError> {
        let out =
            self.data.get(self.at..self.at + n).ok_or_else(|| ProtocolError::new("truncated mux control packet"))?;
        self.at += n;
        Ok(out)
    }
    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    fn remaining(&self) -> usize {
        self.data.len() - self.at
    }
    /// The optional `u32 infoLen, info, f64 priority` tail.
    fn optional_tail(&mut self) -> Result<(Vec<u8>, f64), ProtocolError> {
        let mut info = Vec::new();
        let mut priority = DEFAULT_PRIORITY;

        if self.remaining() >= 4 {
            let len = self.u32()? as usize;
            info = self.take(len)?.to_vec();
            if self.remaining() >= 8 {
                priority = f64::from_le_bytes(self.take(8)?.try_into().expect("8"));
            }
        }

        Ok((info, priority))
    }
}

impl Control {
    pub fn decode(data: &[u8]) -> Result<Self, ProtocolError> {
        let mut r = Reader { data, at: 0 };
        match r.u32()? {
            control::CREATE => {
                let len = usize::from(u16::from_le_bytes(r.take(2)?.try_into().expect("2")));
                if len > MAX_NAME {
                    return Err(ProtocolError::new("channel class name longer than 256 bytes"));
                }

                let name = r.take(len)?.to_vec();
                let fec_type = r.u32()?;
                let (init_info, priority) = r.optional_tail()?;
                Ok(Control::Create { name, fec_type, init_info, priority })
            }
            control::OPEN => {
                let (init_info, priority) = r.optional_tail()?;
                Ok(Control::Open { init_info, priority })
            }
            control::CLOSE => Ok(Control::Close),
            _ => Err(ProtocolError::new("unknown mux control packet type")),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let tail = |out: &mut Vec<u8>, info: &[u8], priority: f64| {
            out.extend_from_slice(&(info.len() as u32).to_le_bytes());
            out.extend_from_slice(info);
            out.extend_from_slice(&priority.to_le_bytes());
        };

        match self {
            Control::Create { name, fec_type, init_info, priority } => {
                out.extend_from_slice(&control::CREATE.to_le_bytes());
                out.extend_from_slice(&(name.len() as u16).to_le_bytes());
                out.extend_from_slice(name);
                out.extend_from_slice(&fec_type.to_le_bytes());
                tail(&mut out, init_info, *priority);
            }
            Control::Open { init_info, priority } => {
                out.extend_from_slice(&control::OPEN.to_le_bytes());
                tail(&mut out, init_info, *priority);
            }
            Control::Close => {
                out.extend_from_slice(&control::CLOSE.to_le_bytes());
                out.extend_from_slice(&0u32.to_le_bytes());
            }
        }

        out
    }
}

/// The sequencer's send side: the channel SN and the dependency of each packet.
#[derive(Debug, Clone, Default)]
pub struct SequencerTx {
    next: u64,
    last_fence: Option<u64>,
    previous_reliable: Option<u64>,
}

impl SequencerTx {
    /// The next packet's SN and 8-byte header for `mode`.
    pub fn header(&mut self, mode: SendMode) -> (u64, [u8; SEQUENCER_HEADER_LEN]) {
        let sn = self.next;
        self.next = (self.next + 1) & SN_MASK;

        let (kind, dep) = match mode {
            SendMode::Unreliable => match self.last_fence {
                None => (0, 0),
                Some(f) => (1, f),
            },
            SendMode::Reliable => {
                let k = match self.previous_reliable {
                    None => (2, 0),
                    Some(p) => (3, p),
                };
                self.previous_reliable = Some(sn);
                k
            }
            SendMode::ReliableFence => {
                let k = match self.previous_reliable {
                    None => (4, 0),
                    Some(p) => (5, p),
                };
                self.previous_reliable = Some(sn);
                self.last_fence = Some(sn);
                k
            }
        };

        let mut h = [0u8; SEQUENCER_HEADER_LEN];
        h[0] = kind;
        h[2..4].copy_from_slice(&(dep as u16).to_le_bytes());
        h[4..8].copy_from_slice(&((dep >> 16) as u32).to_le_bytes());
        (sn, h)
    }
}

/// The sequencer's receive side: exactly-once in-order reliable delivery, best effort after fences.
#[derive(Debug, Default)]
pub struct SequencerRx {
    highest: Option<u64>,
    last_guaranteed: Option<u64>,
    last_fence: Option<u64>,
    pending: Vec<(u64, u8, u64, Vec<u8>)>,
}

impl SequencerRx {
    fn ready(&self, kind: u8, dep: u64) -> bool {
        match kind {
            0 | 2 | 4 => true,
            1 => self.last_fence.is_some_and(|f| dep <= f),
            _ => self.last_guaranteed.is_some_and(|g| dep <= g),
        }
    }

    fn delivered(&mut self, sn: u64, kind: u8) {
        if kind == 2 || kind == 3 {
            self.last_guaranteed = Some(self.last_guaranteed.map_or(sn, |g| g.max(sn)));
        }
        if kind == 4 || kind == 5 {
            self.last_fence = Some(self.last_fence.map_or(sn, |f| f.max(sn)));
            self.last_guaranteed = Some(self.last_guaranteed.map_or(sn, |g| g.max(sn)));
        }
    }

    /// Takes one data packet (descriptor sequence `seq`, payload starting with the header) and
    /// returns the bodies now deliverable, in order.
    pub fn on_packet(&mut self, seq: u16, payload: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        if payload.len() < SEQUENCER_HEADER_LEN {
            return Err(ProtocolError::new("truncated sequencer header"));
        }

        let kind = payload[0];
        if kind >= 7 {
            return Err(ProtocolError::new("sequencer kind 7 or above"));
        }
        if kind == 6 {
            return Err(ProtocolError::new("sequencer kind 6"));
        }

        let dep = u64::from(u16::from_le_bytes([payload[2], payload[3]]))
            | u64::from(u32::from_le_bytes(payload[4..8].try_into().expect("4"))) << 16;
        let sn = match self.highest {
            None => u64::from(seq),
            Some(h) => reconstruct(seq, h) & SN_MASK,
        };

        if matches!(kind, 1 | 3 | 5) && dep >= sn {
            return Err(ProtocolError::new("sequencer dependency not older than its packet"));
        }

        self.highest = Some(self.highest.map_or(sn, |h| h.max(sn)));
        let body = payload[SEQUENCER_HEADER_LEN..].to_vec();
        if kind >= 2 && (self.last_guaranteed.is_some_and(|g| sn <= g) || self.pending.iter().any(|p| p.0 == sn)) {
            return Ok(Vec::new());
        }

        let mut out = Vec::new();
        if !self.ready(kind, dep) {
            self.pending.push((sn, kind, dep, body));
            return Ok(out);
        }
        self.delivered(sn, kind);
        out.push(body);

        while let Some(i) = self.pending.iter().position(|(_, k, d, _)| self.ready(*k, *d)) {
            let (sn, kind, _, body) = self.pending.remove(i);
            if kind >= 2 && self.last_guaranteed.is_some_and(|g| sn <= g) {
                continue;
            }
            self.delivered(sn, kind);
            out.push(body);
        }

        Ok(out)
    }
}

/// One queued channel payload.
struct Queued {
    mode: SendMode,
    payload_type: u8,
    body: Vec<u8>,
}

struct Channel {
    #[cfg_attr(not(feature = "test-support"), expect(dead_code, reason = "only the tests read it"))]
    class_name: Vec<u8>,
    remote_open: bool,
    local_open: bool,
    tx: SequencerTx,
    rx: SequencerRx,
    queue: VecDeque<Queued>,
    priority: f64,
    pass: f64,
}

/// What the mux reports up.
#[derive(Debug, Clone, PartialEq)]
pub enum MuxEvent {
    Created {
        id: u16,
        class_name: Vec<u8>,
        init_info: Vec<u8>,
        priority: f64,
        fec_type: u32,
    },
    /// Both sides sent Open.
    Opened(u16),
    Data {
        id: u16,
        payload_type: u8,
        payload: Vec<u8>,
    },
    /// The phone closed the channel.
    Closed(u16),
}

#[derive(Default)]
pub struct Mux {
    channels: BTreeMap<u16, Channel>,
    parked: Vec<(u16, Control)>,
    control: VecDeque<Outgoing>,
    events: Vec<MuxEvent>,
    pass: f64,
}

impl Mux {
    pub fn events(&mut self) -> Vec<MuxEvent> {
        std::mem::take(&mut self.events)
    }

    /// One packet above the rate controller. Errors are the mux's protocol errors: the caller logs
    /// and drops the packet.
    pub fn on_packet(&mut self, payload_type: u8, ssrc: u32, seq: u16, payload: &[u8]) -> Result<(), ProtocolError> {
        let id = ssrc as u16;
        match payload_type {
            pt::FILLER => Ok(()),
            pt::MUX_CONTROL => {
                let control = Control::decode(payload)?;
                self.on_control(id, control);
                Ok(())
            }
            _ => {
                let Some(channel) = self.channels.get_mut(&id) else {
                    debug!(channel = id, "data for a channel not created; dropped");
                    return Ok(());
                };
                for body in channel.rx.on_packet(seq, payload)? {
                    self.events.push(MuxEvent::Data { id, payload_type, payload: body });
                }
                Ok(())
            }
        }
    }

    fn on_control(&mut self, id: u16, control: Control) {
        match control {
            Control::Create { name, fec_type, init_info, priority } => {
                self.on_create(id, name, fec_type, init_info, priority);
            }
            Control::Open { .. } => self.on_remote_open(id, control),
            Control::Close => {
                if self.channels.remove(&id).is_some() {
                    debug!(channel = id, "the phone closed the channel");
                    self.events.push(MuxEvent::Closed(id));
                } else {
                    self.park(id, control);
                }
            }
        }
    }

    /// The phone creates channel `id`; the controls parked for channels not yet created are
    /// replayed after it.
    fn on_create(&mut self, id: u16, name: Vec<u8>, fec_type: u32, init_info: Vec<u8>, priority: f64) {
        if self.channels.contains_key(&id) {
            warn!(channel = id, "second create request for one channel id ignored");
            return;
        }
        if fec_type != 0 {
            warn!(channel = id, fec_type, "channel created with a forward-error-correction type");
        }

        self.channels.insert(
            id,
            Channel {
                class_name: name.clone(),
                remote_open: false,
                local_open: false,
                tx: SequencerTx::default(),
                rx: SequencerRx::default(),
                queue: VecDeque::new(),
                priority: if priority > 0.0 { priority } else { DEFAULT_PRIORITY },
                pass: self.pass,
            },
        );

        self.events.push(MuxEvent::Created { id, class_name: name, init_info, priority, fec_type });
        for (parked_id, parked) in std::mem::take(&mut self.parked) {
            self.on_control(parked_id, parked);
        }
    }

    /// The phone opens channel `id` (parked when the channel is not created yet).
    fn on_remote_open(&mut self, id: u16, control: Control) {
        let Some(channel) = self.channels.get_mut(&id) else {
            self.park(id, control);
            return;
        };

        if channel.remote_open {
            warn!(channel = id, "second Open of a channel from the phone ignored");
            return;
        }

        channel.remote_open = true;
        if channel.local_open {
            self.events.push(MuxEvent::Opened(id));
        }
    }

    fn park(&mut self, id: u16, control: Control) {
        if self.parked.len() >= MAX_PARKED {
            self.parked.clear();
        }
        self.parked.push((id, control));
    }

    fn queue_control(&mut self, id: u16, control: &Control) {
        self.control.push_back(Outgoing {
            payload_type: pt::MUX_CONTROL,
            ssrc: u32::from(id),
            sequence: 0,
            body: control.encode(),
            reliable: true,
        });
    }

    /// The owner opens channel `id`: send Open with empty init info and priority 1.
    pub fn open(&mut self, id: u16) -> Result<(), ProtocolError> {
        let channel = self.channels.get_mut(&id).ok_or_else(|| ProtocolError::new("no such channel"))?;
        if channel.local_open {
            return Ok(());
        }

        channel.local_open = true;
        let opened = channel.remote_open;
        self.queue_control(id, &Control::Open { init_info: Vec::new(), priority: DEFAULT_PRIORITY });
        if opened {
            self.events.push(MuxEvent::Opened(id));
        }
        Ok(())
    }

    /// The owner closes channel `id`: send Close and forget the channel.
    pub fn close(&mut self, id: u16) {
        if self.channels.remove(&id).is_some() {
            debug!(channel = id, "closing the channel");
            self.queue_control(id, &Control::Close);
        }
    }

    /// Ids of the channels that both sides opened or that are opening.
    pub fn channel_ids(&self) -> Vec<u16> {
        self.channels.keys().copied().collect()
    }

    #[cfg(feature = "test-support")]
    pub fn class_name(&self, id: u16) -> Option<&[u8]> {
        self.channels.get(&id).map(|c| c.class_name.as_slice())
    }

    /// Queues a channel payload (`payload_type` 0x23 unless the channel says otherwise).
    pub fn send(&mut self, id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ProtocolError> {
        if matches!(payload_type, pt::MUX_CONTROL | pt::FILLER | pt::RC_HANDSHAKE | pt::KEEPALIVE | pt::PROBE) {
            return Err(ProtocolError::new("a channel must not use a transport payload type"));
        }

        let channel = self.channels.get_mut(&id).ok_or_else(|| ProtocolError::new("no such channel"))?;
        if channel.queue.is_empty() {
            channel.pass = channel.pass.max(self.pass);
        }

        channel.queue.push_back(Queued { mode, payload_type, body });
        Ok(())
    }

    /// Bytes queued across channels.
    pub fn queued_bytes(&self) -> usize {
        self.channels.values().flat_map(|c| c.queue.iter()).map(|q| q.body.len()).sum::<usize>()
            + self.control.iter().map(|c| c.body.len()).sum::<usize>()
    }

    pub fn has_pending(&self) -> bool {
        !self.control.is_empty() || self.channels.values().any(|c| !c.queue.is_empty())
    }

    /// Size of the next packet body, for pacing.
    pub fn peek_size(&self) -> Option<usize> {
        if let Some(c) = self.control.front() {
            return Some(c.body.len());
        }
        self.next_channel()
            .map(|id| self.channels[&id].queue.front().expect("non-empty").body.len() + SEQUENCER_HEADER_LEN)
    }

    fn next_channel(&self) -> Option<u16> {
        self.channels
            .iter()
            .filter(|(_, c)| !c.queue.is_empty())
            .min_by(|a, b| a.1.pass.total_cmp(&b.1.pass))
            .map(|(id, _)| *id)
    }

    /// The next packet to hand the rate controller: control packets first, then channels in
    /// proportion to their priority.
    pub fn next_packet(&mut self) -> Option<Outgoing> {
        if let Some(c) = self.control.pop_front() {
            return Some(c);
        }

        let id = self.next_channel()?;
        let channel = self.channels.get_mut(&id).expect("listed");
        let q = channel.queue.pop_front().expect("non-empty");
        self.pass = channel.pass;
        channel.pass += 1.0 / channel.priority;

        let (sn, header) = channel.tx.header(q.mode);
        let mut body = Vec::with_capacity(SEQUENCER_HEADER_LEN + q.body.len());
        body.extend_from_slice(&header);
        body.extend_from_slice(&q.body);

        Some(Outgoing {
            payload_type: q.payload_type,
            ssrc: u32::from(id),
            sequence: sn as u16,
            body,
            reliable: q.mode != SendMode::Unreliable,
        })
    }
}

#[cfg(test)]
mod tests;
