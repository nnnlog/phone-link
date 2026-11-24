//! A call's sound on this computer: the microphone, captured from the audio server's default input,
//! and the tone of a keypad key.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use phonelink_core::sync::MutexExt;
use tracing::warn;

use crate::l10n::translate;

/// The application a call's sound plays and records as, the same as the mirrored apps' audio.
const APPLICATION: &str = "Phone Link";
/// The microphone is read this much at a time, and the audio server asked for fragments as long.
const CAPTURE_MS: u32 = 10;
/// The most the microphone's samples wait to be sent; older ones are dropped, so the far end never
/// hears the speaker late.
const WAITING_MS: u32 = 120;

/// The call's stream name in the desktop's sound mixer.
pub(super) fn stream_name() -> &'static str {
    translate("Phone Call")
}

/// The microphone while a call is on this computer.
pub(super) struct Microphone {
    waiting: Arc<Mutex<VecDeque<i16>>>,
    /// Dropping the microphone stops its thread.
    stopped: Arc<AtomicBool>,
}

impl Microphone {
    /// Starts capturing mono samples at `rate` on a thread of its own. When the microphone does not
    /// open, nothing is captured and the far end hears silence.
    pub(super) fn start(rate: u32) -> Self {
        let microphone = Self { waiting: Arc::default(), stopped: Arc::default() };
        let (waiting, stopped) = (microphone.waiting.clone(), microphone.stopped.clone());
        let spawned = std::thread::Builder::new()
            .name("call-microphone".to_owned())
            .spawn(move || capture(rate, &waiting, &stopped));
        if let Err(e) = spawned {
            warn!(error = %e, "the microphone is not captured; the other party hears silence");
        }
        microphone
    }

    /// Exactly `count` samples for the far end: the oldest waiting ones, then silence for what is not
    /// captured yet. While `muted`, silence, and the waiting samples are dropped.
    pub(super) fn take(&self, count: usize, muted: bool) -> Vec<i16> {
        let mut waiting = self.waiting.lock_or_recover();
        let captured = count.min(waiting.len());
        let mut samples: Vec<i16> = waiting.drain(..captured).collect();
        if muted {
            samples.fill(0);
        }
        samples.resize(count, 0);
        samples
    }
}

impl Drop for Microphone {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
    }
}

/// Captures the default input into `waiting` until `stopped`.
fn capture(rate: u32, waiting: &Mutex<VecDeque<i16>>, stopped: &AtomicBool) {
    let sample_spec = Spec { format: Format::S16le, rate, channels: 1 };
    let chunk_len = (rate * CAPTURE_MS / 1000) as usize * 2;
    let attr = BufferAttr {
        maxlength: u32::MAX,
        tlength: u32::MAX,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: u32::try_from(chunk_len).unwrap_or(u32::MAX),
    };
    let name = stream_name();
    let stream = match Simple::new(None, APPLICATION, Direction::Record, None, name, &sample_spec, None, Some(&attr)) {
        Ok(stream) => stream,
        Err(e) => {
            warn!(error = %e, "the microphone did not open; the other party hears silence");
            return;
        }
    };

    let limit = (rate * WAITING_MS / 1000) as usize;
    let mut chunk = vec![0u8; chunk_len];
    while !stopped.load(Ordering::SeqCst) {
        if let Err(e) = stream.read(&mut chunk) {
            warn!(error = %e, "the microphone stopped; the other party hears silence");
            return;
        }

        let mut waiting = waiting.lock_or_recover();
        waiting.extend(chunk.as_chunks::<2>().0.iter().map(|sample| i16::from_le_bytes(*sample)));
        let late = waiting.len().saturating_sub(limit);
        waiting.drain(..late);
    }
}

/// The sample rate a key's tone is made and played at.
pub(super) const TONE_RATE: u32 = 48_000;
/// How long a key's tone sounds.
pub(super) const TONE_MS: u32 = 150;

/// The two frequencies of a keypad key's tone (its row's and its column's); `None` for no key.
pub(super) fn key_frequencies(key: char) -> Option<(f32, f32)> {
    let (row, column) = match key {
        '1' => (0, 0),
        '2' => (0, 1),
        '3' => (0, 2),
        '4' => (1, 0),
        '5' => (1, 1),
        '6' => (1, 2),
        '7' => (2, 0),
        '8' => (2, 1),
        '9' => (2, 2),
        '*' => (3, 0),
        '0' => (3, 1),
        '#' => (3, 2),
        _ => return None,
    };
    Some(([697.0, 770.0, 852.0, 941.0][row], [1209.0, 1336.0, 1477.0][column]))
}

/// The samples of a key's tone: its two frequencies, faded in and out so the tone does not click.
pub(super) fn key_tone(low: f32, high: f32) -> Vec<i16> {
    let rate = TONE_RATE as f32;
    let length = (TONE_RATE * TONE_MS / 1000) as usize;
    (0..length)
        .map(|i| {
            let t = i as f32 / rate;
            let fade = (t / 0.005).min(1.0) * ((length - i) as f32 / (rate * 0.005)).min(1.0);
            let tone = (TAU * low * t).sin() + (TAU * high * t).sin();
            (0.15 * fade * tone * f32::from(i16::MAX)) as i16
        })
        .collect()
}

/// Plays the tone of keypad key `key` on the default output, on a thread of its own.
pub(super) fn play_key(key: char) {
    let Some((low, high)) = key_frequencies(key) else { return };
    let spawned = std::thread::Builder::new().name("call-key-tone".to_owned()).spawn(move || {
        let sample_spec = Spec { format: Format::S16le, rate: TONE_RATE, channels: 1 };
        let name = stream_name();
        let played = Simple::new(None, APPLICATION, Direction::Playback, None, name, &sample_spec, None, None)
            .and_then(|stream| {
                let tone: Vec<u8> = key_tone(low, high).iter().flat_map(|sample| sample.to_le_bytes()).collect();
                stream.write(&tone)?;
                stream.drain()
            });
        if let Err(e) = played {
            warn!(error = %e, "a keypad key's tone did not play");
        }
    });
    if let Err(e) = spawned {
        warn!(error = %e, "a keypad key's tone did not play");
    }
}
