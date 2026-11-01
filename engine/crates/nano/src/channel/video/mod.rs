//! The video target (the PC receives): the phone's `ServerHandshake`, the PC's format choice and
//! `ClientHandshake`, Control packets both ways, and the reassembly of frames from Data packets.
//!
//! Decoding the frames is the consumer's.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::{Announcement, ChannelError, ChannelSender, PT_CHANNEL, RawChannel, RawInbound, Reader};
use crate::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod kind {
    pub(super) const SERVER_HANDSHAKE: u32 = 1;
    pub(super) const CLIENT_HANDSHAKE: u32 = 2;
    pub(super) const CONTROL: u32 = 3;
    pub(super) const DATA: u32 = 4;
}

pub const PROTOCOL_VERSION: u32 = 6;

/// Control flags.
pub mod control {
    pub const FRAME_COMPLETE_ACK: u32 = 0x01;
    pub const FRAME_LOSS: u32 = 0x02;
    pub const FRAME_QUEUE: u32 = 0x04;
    pub const STOP_STREAM: u32 = 0x08;
    pub const START_STREAM: u32 = 0x10;
    pub const REQUEST_KEYFRAME: u32 = 0x20;
    pub const REINITIALIZE: u32 = 0x40;
    pub const FRAME_PRESENTED_ACK: u32 = 0x80;
    pub const SOFT_KEYFRAME_REFRESH: u32 = 0x100;
    pub const DYNAMIC_FORMAT: u32 = 0x200;
    pub const BITRATE_UPDATE: u32 = 0x400;
    pub const SMOOTH_RENDERING: u32 = 0x800;
    pub const CHRONOMETER: u32 = 0x1000;
}

/// Data flags.
pub mod data_flags {
    pub const STREAM_RESET: u32 = 0x01;
    pub const KEYFRAME: u32 = 0x02;
    pub const CHECKSUM: u32 = 0x08;
    pub const FRAME_CHECKSUM: u32 = 0x10;
    pub const NALU_INFO: u32 = 0x20;
}

/// `VideoFormat.codec`.
pub mod codec {
    pub const H264: u32 = 0;
    pub const H265: u32 = 1;
    pub const YUV: u32 = 2;
    pub const RGB: u32 = 3;
    pub const H265_10BIT: u32 = 4;
}

/// The RGB fields of a codec-3 format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbLayout {
    pub bpp: u32,
    pub unnamed: u32,
    pub red_mask: u64,
    pub green_mask: u64,
    pub blue_mask: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFormat {
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub codec: u32,
    pub rgb: Option<RgbLayout>,
}

impl VideoFormat {
    /// What the PC asks for when the phone advertises nothing.
    pub const FALLBACK: VideoFormat = VideoFormat { fps: 30, width: 1280, height: 720, codec: codec::H264, rgb: None };

    fn order_key(&self) -> (u32, u32, u32, u32) {
        (self.codec, self.fps, self.height, self.width)
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        for v in [self.fps, self.width, self.height, self.codec] {
            out.extend_from_slice(&v.to_le_bytes());
        }

        if self.codec == codec::RGB {
            let rgb = self.rgb.unwrap_or(RgbLayout { bpp: 0, unnamed: 0, red_mask: 0, green_mask: 0, blue_mask: 0 });
            out.extend_from_slice(&rgb.bpp.to_le_bytes());
            out.extend_from_slice(&rgb.unnamed.to_le_bytes());
            for m in [rgb.red_mask, rgb.green_mask, rgb.blue_mask] {
                out.extend_from_slice(&m.to_le_bytes());
            }
        }
    }

    pub fn decode(r: &mut Reader<'_>) -> Result<Self, ChannelError> {
        let (fps, width, height, codec) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
        let rgb = if codec == codec::RGB {
            Some(RgbLayout {
                bpp: r.u32()?,
                unnamed: r.u32()?,
                red_mask: r.u64()?,
                green_mask: r.u64()?,
                blue_mask: r.u64()?,
            })
        } else {
            None
        };
        Ok(Self { fps, width, height, codec, rgb })
    }
}

/// The PC's choice: from the greatest advertised format down, the first of at least 1280×720; else
/// the smallest; nothing advertised → 1280×720, 30 fps, H.264.
pub fn choose_format(advertised: &[VideoFormat]) -> VideoFormat {
    let mut sorted: Vec<VideoFormat> = advertised.iter().copied().filter(|f| f.width != 0 && f.height != 0).collect();
    sorted.sort_by_key(VideoFormat::order_key);

    sorted
        .iter()
        .rev()
        .find(|f| f.width >= 1280 && f.height >= 720)
        .or_else(|| sorted.first())
        .copied()
        .unwrap_or(VideoFormat::FALLBACK)
}

/// The phone's `ServerHandshake`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerHandshake {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Phone clock, ms since the Unix epoch.
    pub reference_timestamp_ms: u64,
    pub formats: Vec<VideoFormat>,
}

/// One reassembled frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub frame_id: u32,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub stream_reset: bool,
    /// The frame's data bytes (Annex-B on the phone's path).
    pub data: Vec<u8>,
    pub metadata: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// The handshake finished with this format.
    Ready {
        handshake: ServerHandshake,
        format: VideoFormat,
    },
    /// The phone changed the format (`DynamicVideoFormatUpdate`).
    FormatChanged(VideoFormat),
    /// The phone's target bitrate (bit/s).
    BitrateChanged(u32),
    Frame(VideoFrame),
    Closed,
}

/// One video data packet: a slice of a frame's metadata and data.
#[derive(Clone, Copy)]
struct DataPacket<'a> {
    flags: u32,
    frame_id: u32,
    timestamp_us: u64,
    metadata_size: usize,
    data_size: usize,
    offset: usize,
    metadata: &'a [u8],
    data: &'a [u8],
}

impl<'a> DataPacket<'a> {
    /// Reads a data packet's fields, in wire order.
    fn read(r: &mut Reader<'a>) -> Result<Self, ChannelError> {
        let flags = r.u32()?;
        let frame_id = r.u32()?;
        let timestamp_us = r.u64()?;
        let metadata_size = r.u32()? as usize;
        let data_size = r.u32()? as usize;
        let _packet_count = r.u32()?;
        let offset = r.u32()? as usize;
        let metadata_len = r.u32()? as usize;
        let data_len = r.u32()? as usize;
        let metadata = r.take(metadata_len)?;
        let data = r.take(data_len)?;
        Ok(Self { flags, frame_id, timestamp_us, metadata_size, data_size, offset, metadata, data })
    }
}

struct PartialFrame {
    buffer: Vec<u8>,
    filled: Vec<bool>,
    count: usize,
    metadata_size: usize,
    flags: u32,
    timestamp_us: u64,
    /// The first packet of the frame was flagged Keyframe.
    keyframe: bool,
    /// When the last missing byte arrived (ms on [`now_ms`]'s clock).
    completed_ms: Option<u64>,
}

/// How long a complete frame waits for the missing frames before it, in ms: one frame interval at
/// 60 fps, twice that while a keyframe older than it is still arriving.
pub const DEFRAG_FRAME_TIMEOUT_MS: u64 = 16;
pub const DEFRAG_KEYFRAME_TIMEOUT_MS: u64 = 33;
/// More than this many Data packets in a row of frames already passed make the PC reinitialize the
/// stream, once per channel.
pub const OLD_PACKETS_BEFORE_REINITIALIZE: u32 = 20;

/// What a loss report asks the phone for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LossRequest {
    /// Only the range (`0x02`): a keyframe already skipped it.
    Nothing,
    /// A keyframe (`0x22`): the frame after the gap is a keyframe.
    Keyframe,
    /// A refresh (`0x122`): the frame after the gap is not a keyframe.
    Refresh,
}

/// Sequence comparison of 32-bit frame ids.
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn origin() -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(Instant::now)
}

/// A frame ack's time: microseconds on the PC's monotonic clock, as Windows writes it (the phone only
/// traces it).
pub fn presented_time_us() -> u64 {
    origin().elapsed().as_micros() as u64
}

/// The reassembly's clock: milliseconds on the PC's monotonic clock.
pub fn now_ms() -> u64 {
    origin().elapsed().as_millis() as u64
}

/// The channel's protocol state (sans-IO).
pub struct VideoCore {
    generation: u32,
    handshake_done: bool,
    start_pending: bool,
    streaming: bool,
    /// The handshake's initial frame id: the phone numbers its frames from it on every `StartStream`.
    initial: u32,
    expected: u32,
    frames: BTreeMap<u32, PartialFrame>,
    /// The id of the last frame whose first packet was flagged Keyframe, until it is delivered or
    /// passed.
    keyframe_pending: Option<u32>,
    /// Data packets in a row of frames already passed.
    old_packets: u32,
    /// The stream was reinitialized for old packets (once per channel).
    reinitialized_for_old: bool,
    /// A gap was reported lost since passed frames were last dropped: the next drop of passed frames
    /// sends no range of its own.
    loss_reported: bool,
    out: Vec<(SendMode, Vec<u8>)>,
    events: Vec<VideoEvent>,
    failed: bool,
    /// An inbound packet of an unknown kind or with a frame-size mismatch was logged at `warn`;
    /// later ones, which come with every frame while the condition lasts, are logged at `debug`.
    anomaly_noted: bool,
}

impl VideoCore {
    pub fn new() -> Self {
        Self {
            generation: 0,
            handshake_done: false,
            start_pending: false,
            streaming: false,
            initial: 0,
            expected: 0,
            frames: BTreeMap::new(),
            keyframe_pending: None,
            old_packets: 0,
            reinitialized_for_old: false,
            loss_reported: false,
            out: Vec::new(),
            events: Vec::new(),
            failed: false,
            anomaly_noted: false,
        }
    }

    /// `true` the first time an inbound anomaly is seen on the channel.
    fn first_anomaly(&mut self) -> bool {
        !std::mem::replace(&mut self.anomaly_noted, true)
    }

    fn packet(&mut self, mode: SendMode, kind: u32, body: &[u8]) {
        let mut p = Vec::with_capacity(12 + body.len());
        p.extend_from_slice(&kind.to_le_bytes());
        p.extend_from_slice(&self.generation.to_le_bytes());
        p.extend_from_slice(&(body.len() as u32).to_le_bytes());
        p.extend_from_slice(body);
        self.out.push((mode, p));
    }

    fn control(&mut self, mode: SendMode, flags: u32, fields: &[u8]) {
        let mut b = flags.to_le_bytes().to_vec();
        b.extend_from_slice(fields);
        self.packet(mode, kind::CONTROL, &b);
    }

    pub fn take_output(&mut self) -> Vec<(SendMode, Vec<u8>)> {
        std::mem::take(&mut self.out)
    }

    pub fn take_events(&mut self) -> Vec<VideoEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    /// `StartStream` with `RequestKeyframe`; remembered until the handshake completes.
    pub fn start(&mut self) {
        if !self.handshake_done {
            self.start_pending = true;
            return;
        }

        self.streaming = true;
        self.expected = self.initial;
        self.frames.clear();
        self.keyframe_pending = None;
        self.control(SendMode::ReliableFence, control::START_STREAM | control::REQUEST_KEYFRAME, &[]);
    }

    /// `StopStream` when streaming; a start still waiting for the handshake is forgotten.
    pub fn stop(&mut self) {
        self.start_pending = false;
        if self.handshake_done && self.streaming {
            self.streaming = false;
            self.control(SendMode::ReliableFence, control::STOP_STREAM, &[]);
        }
    }

    pub fn reinitialize(&mut self) {
        if self.handshake_done {
            self.control(SendMode::Reliable, control::REINITIALIZE, &[]);
        }
    }

    /// The consumer decoded and presented `frame_id` at `time_us` (`presented_time_us`).
    pub fn frame_presented(&mut self, frame_id: u32, time_us: u64) {
        let mut f = frame_id.to_le_bytes().to_vec();
        f.extend_from_slice(&time_us.to_le_bytes());
        self.control(SendMode::Reliable, control::FRAME_COMPLETE_ACK | control::FRAME_PRESENTED_ACK, &f);
    }

    /// Frames `from` to `to` (inclusive) are lost.
    fn report_loss(&mut self, from: u32, to: u32, request: LossRequest) {
        let flags = match request {
            LossRequest::Nothing => control::FRAME_LOSS,
            LossRequest::Keyframe => control::FRAME_LOSS | control::REQUEST_KEYFRAME,
            LossRequest::Refresh => control::FRAME_LOSS | control::REQUEST_KEYFRAME | control::SOFT_KEYFRAME_REFRESH,
        };

        let mut f = from.to_le_bytes().to_vec();
        f.extend_from_slice(&to.to_le_bytes());
        self.control(SendMode::Reliable, flags, &f);
    }

    pub fn on_packet(&mut self, payload: &[u8]) {
        self.on_packet_at(payload, now_ms());
    }

    /// One inbound payload at `now` (ms on [`now_ms`]'s clock); complete frames are then delivered.
    /// A malformed packet stops the stream (`StopStream` when it was streaming) and fails the channel,
    /// which its owner then closes.
    pub fn on_packet_at(&mut self, payload: &[u8], now: u64) {
        if let Err(e) = self.handle(payload, now) {
            warn!(error = %e, "malformed video packet; the stream stops and the channel closes");
            self.stop();
            self.failed = true;
            return;
        }

        if self.streaming {
            self.deliver_complete(now);
        }
    }

    fn handle(&mut self, payload: &[u8], now: u64) -> Result<(), ChannelError> {
        let mut r = Reader::new(payload);
        let kind = r.u32()?;
        let generation = r.u32()?;
        let len = r.u32()? as usize;
        let body = r.take(len)?;

        if newer(self.generation, generation) {
            return Ok(());
        }
        if newer(generation, self.generation) {
            self.generation = generation;
            self.frames.clear();
        }

        let mut r = Reader::new(body);
        match kind {
            kind::SERVER_HANDSHAKE => self.on_server_handshake(&mut r),
            kind::CONTROL => self.on_control(&mut r),
            kind::DATA => self.on_data(&mut r, now),
            other => {
                if self.first_anomaly() {
                    warn!(kind = other, "video packet of an unknown kind dropped");
                } else {
                    debug!(kind = other, "video packet of an unknown kind dropped");
                }
                Ok(())
            }
        }
    }

    fn on_server_handshake(&mut self, r: &mut Reader<'_>) -> Result<(), ChannelError> {
        let version = r.u32()?;
        if version != PROTOCOL_VERSION {
            self.failed = true;
            return Err(ChannelError::Handshake(format!("video protocol version {version}")));
        }

        let (width, height, fps) = (r.u32()?, r.u32()?, r.u32()?);
        let reference_timestamp_ms = r.u64()?;
        let count = r.u32()?;
        let formats = (0..count).map(|_| VideoFormat::decode(r)).collect::<Result<Vec<_>, _>>()?;
        let format = choose_format(&formats);

        let initial: u32 = rand::random::<u32>() & 0x7FFF_FFFF;
        self.initial = initial;
        let mut b = initial.to_le_bytes().to_vec();
        format.encode(&mut b);
        self.packet(SendMode::ReliableFence, kind::CLIENT_HANDSHAKE, &b);
        self.handshake_done = true;

        // A handshake while streaming is the phone's after a reinitialize: it sends no frames until
        // StartStream comes again.
        let restart = std::mem::take(&mut self.start_pending) || self.streaming;
        debug!(?format, restart, "Nano video handshake done");
        self.events.push(VideoEvent::Ready {
            handshake: ServerHandshake { width, height, fps, reference_timestamp_ms, formats },
            format,
        });

        if restart {
            self.start();
        }
        Ok(())
    }

    fn on_control(&mut self, r: &mut Reader<'_>) -> Result<(), ChannelError> {
        let flags = r.u32()?;
        if flags & (control::FRAME_COMPLETE_ACK | control::FRAME_PRESENTED_ACK) != 0 {
            r.u32()?;
            r.u64()?;
        }
        if flags & control::FRAME_QUEUE != 0 {
            r.u32()?;
        }
        if flags & control::FRAME_LOSS != 0 {
            r.u32()?;
            r.u32()?;
        }

        if flags & control::BITRATE_UPDATE != 0 {
            self.events.push(VideoEvent::BitrateChanged(r.u32()?));
        }
        if flags & control::DYNAMIC_FORMAT != 0 {
            self.events.push(VideoEvent::FormatChanged(VideoFormat::decode(r)?));
        }
        Ok(())
    }

    fn on_data(&mut self, r: &mut Reader<'_>, now: u64) -> Result<(), ChannelError> {
        let packet = DataPacket::read(r)?;
        let frame_id = packet.frame_id;

        if !self.streaming {
            trace!(frame_id, "video data before StartStream; dropped");
            return Ok(());
        }
        if newer(self.expected, frame_id) {
            self.on_old_packet(frame_id);
            return Ok(());
        }

        self.old_packets = 0;
        self.store_packet(&packet, now)
    }

    /// Counts a packet of a frame already passed, reinitializing once when too many arrive.
    fn on_old_packet(&mut self, frame_id: u32) {
        self.old_packets += 1;
        if self.old_packets > OLD_PACKETS_BEFORE_REINITIALIZE && !self.reinitialized_for_old {
            warn!(frame_id, expected = self.expected, "video keeps arriving for passed frames; reinitializing");
            self.reinitialized_for_old = true;
            self.reinitialize();
        }
    }

    /// Writes a data packet into its partial frame, marking the frame complete at its last byte.
    fn store_packet(&mut self, packet: &DataPacket<'_>, now: u64) -> Result<(), ChannelError> {
        let DataPacket { flags, frame_id, timestamp_us, metadata_size, data_size, offset, metadata, data } = *packet;
        let metadata_len = metadata.len();
        let total = metadata_size + data_size;
        let end = offset + metadata_len + data.len();

        if end > total {
            return Err(ChannelError::Invalid(format!("video data of frame {frame_id} ends past the frame")));
        }

        if !self.frames.contains_key(&frame_id) && flags & data_flags::KEYFRAME != 0 {
            self.keyframe_pending = Some(frame_id);
        }

        let f = self.frames.entry(frame_id).or_insert_with(|| PartialFrame {
            buffer: vec![0; total],
            filled: vec![false; total],
            count: 0,
            metadata_size,
            flags,
            timestamp_us,
            keyframe: flags & data_flags::KEYFRAME != 0,
            completed_ms: None,
        });

        if f.buffer.len() != total {
            if self.first_anomaly() {
                warn!(frame_id, "video data that disagrees with its frame's size dropped");
            } else {
                debug!(frame_id, "video data that disagrees with its frame's size dropped");
            }
            return Ok(());
        }
        if f.completed_ms.is_some() {
            return Ok(());
        }

        // A packet that overlaps bytes already received (a duplicate or an overlap) is ignored whole.
        if f.filled[offset..end].iter().any(|b| *b) {
            trace!(frame_id, offset, "video data overlaps bytes already received; ignored");
            return Ok(());
        }

        f.flags |= flags;
        f.buffer[offset..offset + metadata_len].copy_from_slice(metadata);
        f.buffer[offset + metadata_len..end].copy_from_slice(data);
        f.filled[offset..end].fill(true);
        f.count += end - offset;
        if f.count >= total {
            f.completed_ms = Some(now);
        }
        Ok(())
    }

    /// Delivers the complete frames in frame-id order. A complete frame after missing ones waits
    /// for them up to the defragment timeout (from its completion; the longer one while a keyframe
    /// older than it is still arriving), then the gap is reported lost and the frame delivered; a
    /// complete keyframe is delivered at once. Frames left behind are dropped, and reported lost
    /// (range only) unless a gap was reported since frames were last dropped this way, in this pass
    /// or an earlier one.
    fn deliver_complete(&mut self, now: u64) {
        let mut ids: Vec<u32> =
            self.frames.iter().filter(|(_, f)| f.completed_ms.is_some()).map(|(id, _)| *id).collect();
        ids.sort_by_key(|id| id.wrapping_sub(self.expected));

        for id in ids {
            if newer(self.expected, id) {
                continue;
            }

            let (keyframe, completed) = {
                let f = &self.frames[&id];
                (f.keyframe, f.completed_ms.unwrap_or(now))
            };

            if id != self.expected {
                let timeout = if self.keyframe_pending.is_some_and(|k| newer(id, k)) {
                    DEFRAG_KEYFRAME_TIMEOUT_MS
                } else {
                    DEFRAG_FRAME_TIMEOUT_MS
                };

                if now.saturating_sub(completed) > timeout {
                    let request = if keyframe { LossRequest::Keyframe } else { LossRequest::Refresh };
                    self.report_loss(self.expected, id.wrapping_sub(1), request);
                    self.expected = id;
                    self.loss_reported = true;
                }

                if id != self.expected && !keyframe {
                    continue;
                }
            }

            self.deliver(id);
        }

        // Frames before the expected one are passed: complete ones are never shown, partial ones never
        // complete.
        let passed: Vec<u32> = self.frames.keys().copied().filter(|id| newer(self.expected, *id)).collect();
        if let (Some(first), Some(last)) = (
            passed.iter().copied().min_by_key(|id| id.wrapping_sub(self.expected)),
            passed.iter().copied().max_by_key(|id| id.wrapping_sub(self.expected)),
        ) {
            for id in &passed {
                self.frames.remove(id);
            }
            if !std::mem::take(&mut self.loss_reported) {
                self.report_loss(first, last, LossRequest::Nothing);
            }
        }

        if self.keyframe_pending.is_some_and(|k| newer(self.expected, k)) {
            self.keyframe_pending = None;
        }
    }

    fn deliver(&mut self, frame_id: u32) {
        let f = self.frames.remove(&frame_id).expect("present");
        self.expected = frame_id.wrapping_add(1);
        if self.keyframe_pending == Some(frame_id) {
            self.keyframe_pending = None;
        }

        let mut buffer = f.buffer;
        let data = buffer.split_off(f.metadata_size);
        self.events.push(VideoEvent::Frame(VideoFrame {
            frame_id,
            timestamp_us: f.timestamp_us,
            keyframe: f.flags & data_flags::KEYFRAME != 0,
            stream_reset: f.flags & data_flags::STREAM_RESET != 0,
            data,
            metadata: buffer,
        }));
    }
}

impl Default for VideoCore {
    fn default() -> Self {
        Self::new()
    }
}

/// An open video target.
pub struct VideoTarget {
    pub announcement: Announcement,
    sender: ChannelSender,
    core: Arc<Mutex<VideoCore>>,
    events: mpsc::UnboundedReceiver<VideoEvent>,
}

impl VideoTarget {
    pub async fn open(mut raw: RawChannel) -> Result<Self, ChannelError> {
        let early = raw.open().await?;
        let (sender, announcement, mut inbound) = raw.split();

        let core = Arc::new(Mutex::new(VideoCore::new()));
        let (tx, events) = mpsc::unbounded_channel();

        let (reader_core, reader_sender) = (core.clone(), sender.clone());
        tokio::spawn(async move {
            let mut queued = early.into_iter();
            loop {
                let item = match queued.next() {
                    Some(i) => i,
                    None => inbound.recv().await.unwrap_or(RawInbound::Closed),
                };

                match item {
                    RawInbound::Data { payload, .. } => {
                        let (out, evs, failed) = {
                            let mut c = reader_core.lock_or_panic();
                            c.on_packet(&payload);
                            (c.take_output(), c.take_events(), c.failed())
                        };

                        flush(&reader_sender, out);
                        for e in evs {
                            let _ = tx.send(e);
                        }

                        if failed {
                            reader_sender.close();
                            let _ = tx.send(VideoEvent::Closed);
                            return;
                        }
                    }
                    RawInbound::Opened => {}
                    RawInbound::Closed => {
                        let _ = tx.send(VideoEvent::Closed);
                        return;
                    }
                }
            }
        });

        Ok(Self { announcement, sender, core, events })
    }

    fn with_core(&self, f: impl FnOnce(&mut VideoCore)) {
        let out = {
            let mut c = self.core.lock_or_panic();
            f(&mut c);
            c.take_output()
        };
        flush(&self.sender, out);
    }

    pub fn start(&self) {
        self.with_core(VideoCore::start);
    }

    pub fn stop(&self) {
        self.with_core(VideoCore::stop);
    }

    pub fn reinitialize(&self) {
        self.with_core(VideoCore::reinitialize);
    }

    pub fn frame_presented(&self, frame_id: u32, time_us: u64) {
        self.with_core(|c| c.frame_presented(frame_id, time_us));
    }

    pub async fn next_event(&mut self) -> Option<VideoEvent> {
        self.events.recv().await
    }

    pub fn close(&self) {
        self.sender.close();
    }
}

fn flush(sender: &ChannelSender, out: Vec<(SendMode, Vec<u8>)>) {
    for (mode, p) in out {
        if let Err(e) = sender.send(mode, PT_CHANNEL, p) {
            super::log_not_sent("video", &e);
        }
    }
}

#[cfg(test)]
mod tests;
