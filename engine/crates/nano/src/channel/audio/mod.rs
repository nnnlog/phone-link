//! The audio target (the PC receives): no in-band header — the packet type is the payload type minus
//! `0x23`.
//!
//! The phone's `ServerHandshake`, the PC's format choice and `ClientHandshake`, Control start,
//! stop and reinitialize, and every Data frame in the order it arrives — the phone gives all the packets
//! of one capture buffer the same frame id, so the id only shows lost buffers. Decoding is the consumer's.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::{Announcement, ChannelError, ChannelSender, PT_CHANNEL, RawChannel, RawInbound, Reader, property};
use crate::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod kind {
    pub(super) const SERVER_HANDSHAKE: u8 = 1;
    pub(super) const CLIENT_HANDSHAKE: u8 = 2;
    pub(super) const CONTROL: u8 = 3;
    pub(super) const DATA: u8 = 4;
}

pub const PROTOCOL_VERSION: u32 = 7;

pub mod control {
    pub const STOP: u32 = 0x08;
    pub const START: u32 = 0x10;
    pub const REINITIALIZE: u32 = 0x40;
}

pub mod codec {
    pub const OPUS: u32 = 0;
    pub const PCM: u32 = 1;
    pub const AAC: u32 = 2;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioFormat {
    pub channels: u32,
    pub sample_rate: u32,
    pub codec: u32,
    /// PCM only: `(bytesPerSample, isFloat)` — bytes, not bits: a sample block is `channels ×
    /// bytesPerSample` bytes.
    pub pcm: Option<(u32, u32)>,
}

impl AudioFormat {
    pub const FALLBACK: AudioFormat = AudioFormat { channels: 2, sample_rate: 48_000, codec: codec::OPUS, pcm: None };

    fn encode(&self, out: &mut Vec<u8>) {
        for v in [self.channels, self.sample_rate, self.codec] {
            out.extend_from_slice(&v.to_le_bytes());
        }

        if self.codec == codec::PCM {
            let (bytes, float) = self.pcm.unwrap_or((2, 0));
            out.extend_from_slice(&bytes.to_le_bytes());
            out.extend_from_slice(&float.to_le_bytes());
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, ChannelError> {
        let (channels, sample_rate, codec) = (r.u32()?, r.u32()?, r.u32()?);
        let pcm = if codec == codec::PCM { Some((r.u32()?, r.u32()?)) } else { None };
        Ok(Self { channels, sample_rate, codec, pcm })
    }
}

/// The lowest codec advertised, then the highest sample rate, the most channels, and for PCM the
/// most bytes per sample with float before integer; nothing advertised → Opus, 2 channels, 48 kHz.
pub fn choose_format(advertised: &[AudioFormat]) -> AudioFormat {
    advertised
        .iter()
        .copied()
        .min_by_key(|f| {
            let (bytes, float) = f.pcm.unwrap_or((0, 0));
            (f.codec, std::cmp::Reverse((f.sample_rate, f.channels, bytes, float)))
        })
        .unwrap_or(AudioFormat::FALLBACK)
}

/// One audio frame as the phone sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFrame {
    pub flags: u32,
    pub frame_id: u32,
    pub timestamp: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    Ready { reference_timestamp: u64, format: AudioFormat },
    Frame(AudioFrame),
    Closed,
}

/// The channel's protocol state (sans-IO).
pub struct AudioCore {
    handshake_done: bool,
    start_pending: bool,
    /// The id the next buffer should carry; only for counting lost buffers.
    expected_frame: Option<u32>,
    out: Vec<(u8, Vec<u8>)>,
    events: Vec<AudioEvent>,
    failed: bool,
    /// A malformed or unknown inbound packet was logged at `warn`; later ones, which come with
    /// every buffer while the condition lasts, are logged at `debug`.
    anomaly_noted: bool,
}

impl AudioCore {
    pub fn new() -> Self {
        Self {
            handshake_done: false,
            start_pending: false,
            expected_frame: None,
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

    fn log_unknown_kind(&mut self, kind: u8) {
        if self.first_anomaly() {
            warn!(kind, "audio packet of an unknown kind dropped");
        } else {
            debug!(kind, "audio packet of an unknown kind dropped");
        }
    }

    fn control(&mut self, flags: u32) {
        self.out.push((kind::CONTROL, flags.to_le_bytes().to_vec()));
    }

    pub fn take_output(&mut self) -> Vec<(u8, Vec<u8>)> {
        std::mem::take(&mut self.out)
    }

    pub fn take_events(&mut self) -> Vec<AudioEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn failed(&self) -> bool {
        self.failed
    }

    pub fn start(&mut self) {
        if self.handshake_done {
            self.control(control::START);
        } else {
            self.start_pending = true;
        }
    }

    pub fn stop(&mut self) {
        self.start_pending = false;
        if self.handshake_done {
            self.control(control::STOP);
        }
    }

    /// The phone answers with a new handshake; until then it throws on another stop or reinitialize.
    pub fn reinitialize(&mut self) {
        if self.handshake_done {
            self.control(control::REINITIALIZE);
            self.handshake_done = false;
        }
    }

    /// One packet of payload type `0x23 + kind`. A handshake the PC cannot take fails the channel,
    /// which then closes.
    pub fn on_packet(&mut self, payload_type: u8, payload: &[u8]) {
        if let Err(e) = self.handle(payload_type.wrapping_sub(PT_CHANNEL), payload) {
            if self.failed {
                warn!(error = %e, "audio handshake refused; the channel closes");
            } else if self.first_anomaly() {
                warn!(error = %e, "malformed audio packet dropped");
            } else {
                debug!(error = %e, "malformed audio packet dropped");
            }
        }
    }

    fn handle(&mut self, kind: u8, payload: &[u8]) -> Result<(), ChannelError> {
        let mut r = Reader::new(payload);
        match kind {
            kind::SERVER_HANDSHAKE => {
                let version = r.u32()?;
                if version != PROTOCOL_VERSION {
                    self.failed = true;
                    return Err(ChannelError::Handshake(format!("audio protocol version {version}")));
                }

                let reference_timestamp = r.u64()?;
                let count = r.u32()?;
                let formats = (0..count).map(|_| AudioFormat::decode(&mut r)).collect::<Result<Vec<_>, _>>()?;
                let format = choose_format(&formats);

                let mut b = rand::random::<u32>().to_le_bytes().to_vec();
                format.encode(&mut b);
                self.out.push((kind::CLIENT_HANDSHAKE, b));

                self.handshake_done = true;
                debug!(?format, "Nano audio handshake done");
                self.events.push(AudioEvent::Ready { reference_timestamp, format });

                // The audio target starts the stream itself once the handshake is done.
                self.start_pending = false;
                self.control(control::START);
            }
            kind::DATA => {
                let flags = r.u32()?;
                let frame_id = r.u32()?;
                let timestamp = r.u64()?;
                let len = r.u32()? as usize;
                let data = r.take(len)?.to_vec();

                if let Some(lost) = self.expected_frame.map(|e| frame_id.wrapping_sub(e) as i32).filter(|n| *n > 0) {
                    debug!(lost, frame_id, "audio buffers lost");
                }

                self.expected_frame = Some(frame_id.wrapping_add(1));
                self.events.push(AudioEvent::Frame(AudioFrame { flags, frame_id, timestamp, data }));
            }
            kind::CONTROL => {}
            other => self.log_unknown_kind(other),
        }

        Ok(())
    }
}

impl Default for AudioCore {
    fn default() -> Self {
        Self::new()
    }
}

/// An open audio target.
pub struct AudioTarget {
    pub announcement: Announcement,
    sender: ChannelSender,
    core: Arc<Mutex<AudioCore>>,
    events: mpsc::UnboundedReceiver<AudioEvent>,
}

impl AudioTarget {
    /// Opens the channel; the announcement must carry `syncId`.
    pub async fn open(mut raw: RawChannel) -> Result<Self, ChannelError> {
        if raw.announcement.property(property::SYNC_ID).is_none() {
            raw.close();
            return Err(ChannelError::Invalid("audio channel without syncId".into()));
        }

        let early = raw.open().await?;
        let (sender, announcement, mut inbound) = raw.split();

        let core = Arc::new(Mutex::new(AudioCore::new()));
        let (tx, events) = mpsc::unbounded_channel();

        let (c, s) = (core.clone(), sender.clone());
        tokio::spawn(async move {
            let mut queued = early.into_iter();
            loop {
                let item = match queued.next() {
                    Some(i) => i,
                    None => inbound.recv().await.unwrap_or(RawInbound::Closed),
                };

                match item {
                    RawInbound::Data { payload_type, payload } => {
                        let (out, evs, failed) = {
                            let mut core = c.lock_or_panic();
                            core.on_packet(payload_type, &payload);
                            (core.take_output(), core.take_events(), core.failed())
                        };

                        flush(&s, out);
                        for e in evs {
                            let _ = tx.send(e);
                        }

                        if failed {
                            s.close();
                            let _ = tx.send(AudioEvent::Closed);
                            return;
                        }
                    }
                    RawInbound::Opened => {}
                    RawInbound::Closed => {
                        let _ = tx.send(AudioEvent::Closed);
                        return;
                    }
                }
            }
        });

        Ok(Self { announcement, sender, core, events })
    }

    fn with_core(&self, f: impl FnOnce(&mut AudioCore)) {
        let out = {
            let mut c = self.core.lock_or_panic();
            f(&mut c);
            c.take_output()
        };
        flush(&self.sender, out);
    }

    pub fn start(&self) {
        self.with_core(AudioCore::start);
    }

    pub fn stop(&self) {
        self.with_core(AudioCore::stop);
    }

    pub fn reinitialize(&self) {
        self.with_core(AudioCore::reinitialize);
    }

    pub async fn next_event(&mut self) -> Option<AudioEvent> {
        self.events.recv().await
    }

    pub fn close(&self) {
        self.sender.close();
    }
}

fn flush(sender: &ChannelSender, out: Vec<(u8, Vec<u8>)>) {
    for (kind, body) in out {
        if let Err(e) = sender.send(SendMode::Reliable, PT_CHANNEL + kind, body) {
            super::log_not_sent("audio", &e);
        }
    }
}

#[cfg(test)]
mod tests;
