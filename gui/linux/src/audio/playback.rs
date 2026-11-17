//! One app's stream on the audio server: what it asks of the server, opening it again after it fails,
//! and each frame decoded, bounded by the queue and written.

use std::time::{Duration, Instant};

use libpulse_binding::def::BufferAttr;
use libpulse_binding::error::PAErr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use phonelink_screen_mirroring::edge::AudioStreamFormat;

use super::decoding::{
    CODEC_AAC, CODEC_OPUS, CODEC_PCM, Decoding, decode_opus, opus_channels, pcm_format, whole_frames,
};
use super::queue::{AudioQueue, desired_ms};

/// How long a player waits before it opens its stream again after it did not open or stopped.
const REOPEN_AFTER: Duration = Duration::from_secs(1);

/// The bytes `ms` milliseconds of `sample_spec` take: a whole number of sample frames.
fn bytes(sample_spec: &Spec, ms: u64) -> u32 {
    let samples = u64::from(sample_spec.rate) * ms / 1000;
    u32::try_from(samples * sample_spec.frame_size() as u64).unwrap_or(u32::MAX)
}

/// What a stream of frames `frame_ms` long asks of the server.
///
/// It asks for about two frames of latency (the queue's desired depth), and playback that starts once a
/// frame is there (the server's own start, with the latency, for a stream that names no frame length).
/// The server chooses the rest.
pub(super) fn buffer_attr(sample_spec: &Spec, frame_ms: u32) -> BufferAttr {
    BufferAttr {
        maxlength: u32::MAX,
        tlength: bytes(sample_spec, desired_ms(frame_ms)),
        prebuf: if frame_ms == 0 { u32::MAX } else { bytes(sample_spec, u64::from(frame_ms)) },
        minreq: u32::MAX,
        fragsize: u32::MAX,
    }
}

/// How a stream failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamFailure {
    /// It did not open.
    NotOpened,
    /// It stopped taking frames.
    Stopped,
}

/// One app's stream on the audio server, and what it plays.
pub(super) struct Playback {
    app: String,
    sample_spec: Spec,
    attr: BufferAttr,
    decoding: Decoding,
    /// The length of one of the app's frames.
    frame: Duration,
    queue: AudioQueue,
    stream: Option<Simple>,
    /// When the stream last did not open or stopped taking frames: it opens again `REOPEN_AFTER` later.
    failed_at: Option<Instant>,
    /// Whether a failure was logged since the stream last took a frame: the failures after it are not
    /// logged until the stream plays again.
    failure_logged: bool,
}

impl Playback {
    /// The playback of `format`; `None` for a format this app does not play (logged).
    pub(super) fn new(app: String, format: &AudioStreamFormat) -> Option<Self> {
        let Ok(channels) = u8::try_from(format.channels) else {
            tracing::warn!(package = %app, ?format, "the audio of an app has more channels than the audio server takes");
            return None;
        };

        let (sample, decoding) = match format.codec {
            CODEC_OPUS => {
                let decoder = opus_channels(format.channels)
                    .and_then(|layout| opus::Decoder::new(format.sample_rate, layout).ok());
                let Some(decoder) = decoder else {
                    tracing::warn!(package = %app, ?format, "no Opus decoder for the audio of an app");
                    return None;
                };
                (Format::S16le, Decoding::Opus(decoder))
            }
            CODEC_PCM => {
                let Some(sample) = pcm_format(format) else {
                    tracing::warn!(package = %app, ?format, "the audio of an app is in a PCM layout this app does not play");
                    return None;
                };
                (sample, Decoding::Pcm)
            }
            CODEC_AAC => {
                tracing::warn!(package = %app, "the audio of an app is AAC, which this app does not play");
                return None;
            }
            _ => {
                tracing::warn!(package = %app, ?format, "the audio of an app is in a codec this app does not know");
                return None;
            }
        };

        let sample_spec = Spec { format: sample, rate: format.sample_rate, channels };
        if !sample_spec.is_valid() {
            tracing::warn!(package = %app, ?format, "the audio of an app is in a sample format the audio server does not take");
            return None;
        }
        Some(Self {
            app,
            attr: buffer_attr(&sample_spec, format.frame_length_ms),
            sample_spec,
            decoding,
            frame: Duration::from_millis(u64::from(format.frame_length_ms)),
            queue: AudioQueue::new(format.frame_length_ms),
            stream: None,
            failed_at: None,
            failure_logged: false,
        })
    }

    /// Opens the stream when it is closed: at once the first time, then `REOPEN_AFTER` after it failed.
    pub(super) fn open_when_due(&mut self) {
        if self.stream.is_some() || self.failed_at.is_some_and(|at| at.elapsed() < REOPEN_AFTER) {
            return;
        }

        let opened = Simple::new(
            None,
            "Phone Link",
            Direction::Playback,
            None,
            &self.app,
            &self.sample_spec,
            None,
            Some(&self.attr),
        );
        match opened {
            Ok(stream) => self.stream = Some(stream),
            Err(e) => self.failed(&e, StreamFailure::NotOpened),
        }
    }

    /// The stream did not open or stopped taking frames: it is closed, to open again later.
    fn failed(&mut self, error: &PAErr, failure: StreamFailure) {
        self.stream = None;
        self.failed_at = Some(Instant::now());
        if std::mem::replace(&mut self.failure_logged, true) {
            return;
        }

        match failure {
            StreamFailure::NotOpened => {
                tracing::warn!(package = %self.app, error = %error, "the audio of an app did not open; it is opened again while its frames come, and later failures are not logged");
            }
            StreamFailure::Stopped => {
                tracing::warn!(package = %self.app, error = %error, "the audio of an app stopped; it is opened again while its frames come, and later failures are not logged");
            }
        }
    }

    /// Plays one of the app's frames, behind which `backlog` more wait to be played.
    pub(super) fn play(&mut self, mut frame: Vec<u8>, backlog: u32) {
        // Decoded even when it is dropped, so that the decoder follows the stream.
        let samples = match &mut self.decoding {
            Decoding::Opus(decoder) => match decode_opus(decoder, usize::from(self.sample_spec.channels), &frame) {
                Ok(samples) => samples,
                Err(e) => {
                    tracing::debug!(package = %self.app, error = %e, "an Opus frame of an app's audio did not decode; it is skipped");
                    return;
                }
            },
            Decoding::Pcm => {
                frame.truncate(whole_frames(frame.len(), self.sample_spec.frame_size()));
                frame
            }
        };
        if samples.is_empty() {
            return;
        }

        self.open_when_due();
        let Some(stream) = &self.stream else { return };
        let latency = match stream.get_latency() {
            Ok(latency) => Duration::from_micros(latency.0),
            Err(e) => {
                self.failed(&e, StreamFailure::Stopped);
                return;
            }
        };

        let waiting = self.queue.waiting(self.frame, backlog, latency);
        let was_draining = self.queue.draining;
        let admitted = self.queue.admit(waiting);
        if self.queue.draining != was_draining {
            let waiting_ms = u64::try_from(waiting.as_millis()).unwrap_or(u64::MAX);
            if self.queue.draining {
                tracing::info!(
                    waiting_ms,
                    "an app's audio waits past its maximum; its frames are dropped until it drains"
                );
            } else {
                tracing::info!(waiting_ms, "an app's audio drained; its frames play again");
            }
        }
        if !admitted {
            return;
        }

        match stream.write(&samples) {
            Ok(()) => self.failure_logged = false,
            Err(e) => self.failed(&e, StreamFailure::Stopped),
        }
    }

    /// Drops what the server holds of the stream.
    pub(super) fn flush(&self) {
        if let Some(stream) = &self.stream
            && let Err(e) = stream.flush()
        {
            tracing::debug!(package = %self.app, error = %e, "an app's audio was not flushed");
        }
    }
}
