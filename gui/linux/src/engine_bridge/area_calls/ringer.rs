//! The ring of an incoming call on this computer: a short bell, every two seconds until stopped.

use std::f32::consts::TAU;
use std::sync::mpsc;

use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use tracing::{info, warn};

use crate::l10n::translate;

/// The ring of an incoming call on this computer.
///
/// The phone's own ringtone does not play here: the phone's voice link is not taken while a call
/// rings.
#[derive(Default)]
pub(super) struct Ringer {
    /// Dropping it stops the ring.
    stop: Option<mpsc::Sender<()>>,
}

impl Ringer {
    pub(super) fn start(&mut self) {
        if self.stop.is_some() {
            return;
        }
        let (stop, stopped) = mpsc::channel();
        match std::thread::Builder::new().name("call-ring".to_owned()).spawn(move || ring(&stopped)) {
            Ok(_) => self.stop = Some(stop),
            Err(e) => warn!(error = %e, "the incoming call does not ring"),
        }
    }

    pub(super) fn stop(&mut self) {
        self.stop = None;
    }
}

/// The application the ring's sound plays as.
///
/// Audio servers keep a volume and a mute per application, so the ring gets its own name, apart from
/// the mirrored apps' audio, which plays as "Phone Link".
const RING_APPLICATION: &str = "Phone Link Ring";
pub(super) const RING_RATE: u32 = 48_000;
pub(super) const RING_PERIOD_MS: u32 = 2_000;
/// The ring checks for its stop between writes of this many samples (100 ms).
const RING_CHUNK: usize = 4_800;

/// One period of the ring: two short bell tones, then silence until the next period.
pub(super) fn ring_period() -> Vec<i16> {
    let rate = RING_RATE as f32;
    let mut samples = vec![0f32; (RING_RATE * RING_PERIOD_MS / 1000) as usize];
    for (start_ms, frequency, length_ms) in [(0u32, 1318.5f32, 180u32), (200, 1046.5, 520)] {
        let start = (RING_RATE * start_ms / 1000) as usize;
        let length = (RING_RATE * length_ms / 1000) as usize;
        let decay = length_ms as f32 / 1000.0 / 3.0;
        for (i, sample) in samples[start..start + length].iter_mut().enumerate() {
            let t = i as f32 / rate;
            let attack = (t / 0.004).min(1.0);
            let release = ((length - i) as f32 / (rate * 0.01)).min(1.0);
            let tone = (TAU * frequency * t).sin() + 0.3 * (TAU * 2.0 * frequency * t).sin();
            *sample += 0.25 * attack * release * (-t / decay).exp() * tone;
        }
    }
    samples.into_iter().map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).collect()
}

/// Plays the ring until `stopped` disconnects; what is still buffered then is dropped.
fn ring(stopped: &mpsc::Receiver<()>) {
    let sample_spec = Spec { format: Format::S16le, rate: RING_RATE, channels: 1 };
    // The stream's name shows in the desktop's sound mixer.
    let stream_name = translate("Incoming Call");
    let stream =
        match Simple::new(None, RING_APPLICATION, Direction::Playback, None, stream_name, &sample_spec, None, None) {
            Ok(stream) => stream,
            Err(e) => {
                warn!(error = %e, "the incoming call does not ring on this computer");
                return;
            }
        };

    let period: Vec<u8> = ring_period().iter().flat_map(|sample| sample.to_le_bytes()).collect();
    loop {
        for chunk in period.chunks(RING_CHUNK * 2) {
            if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                if let Err(e) = stream.flush() {
                    info!(error = %e, "the ring's last sound was not dropped");
                }
                return;
            }
            if let Err(e) = stream.write(chunk) {
                warn!(error = %e, "the ring stopped");
                return;
            }
        }
    }
}
