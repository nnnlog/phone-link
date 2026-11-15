use std::sync::mpsc::{Receiver, Sender, channel};

use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;
use phonelink_screen_mirroring::edge::AudioStreamFormat;

const CODEC_OPUS: u32 = 0;
const CODEC_PCM: u32 = 1;
const OPUS_MAX_FRAME: usize = 5760;

pub fn has_output() -> bool {
    true
}

fn open(app: &str, format: Format, rate: u32, channels: u8) -> Option<Simple> {
    let spec = Spec { format, rate, channels };
    Simple::new(None, "Phone Link", Direction::Playback, None, app, &spec, None, None)
        .inspect_err(|e| eprintln!("the audio of {app} did not open: {e}"))
        .ok()
}

fn pcm_format(format: &AudioStreamFormat) -> Option<Format> {
    match (format.is_float, format.bytes_per_sample) {
        (true, 4) => Some(Format::F32le),
        (false, 2) => Some(Format::S16le),
        (false, 3) => Some(Format::S24le),
        (false, 4) => Some(Format::S32le),
        _ => None,
    }
}

fn opus_channels(channels: u32) -> Option<opus::Channels> {
    match channels {
        1 => Some(opus::Channels::Mono),
        2 => Some(opus::Channels::Stereo),
        _ => None,
    }
}

fn decode_opus(decoder: &mut opus::Decoder, channels: usize, packet: &[u8]) -> Vec<u8> {
    let mut pcm = vec![0i16; OPUS_MAX_FRAME * channels];
    let Ok(samples) = decoder.decode(packet, &mut pcm, false) else { return Vec::new() };
    pcm.truncate(samples * channels);
    pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect()
}

fn run(app: String, format: AudioStreamFormat, frames: Receiver<Vec<u8>>) {
    let Ok(channels) = u8::try_from(format.channels) else { return };
    let (stream, mut opus) = match format.codec {
        CODEC_OPUS => {
            let decoder =
                opus_channels(format.channels).and_then(|layout| opus::Decoder::new(format.sample_rate, layout).ok());
            let Some(decoder) = decoder else {
                eprintln!("no Opus decoder for the audio of {app}: {format:?}");
                return;
            };
            (open(&app, Format::S16le, format.sample_rate, channels), Some(decoder))
        }
        CODEC_PCM => {
            let Some(sample) = pcm_format(&format) else {
                eprintln!("the audio of {app} is in a PCM layout this app does not play: {format:?}");
                return;
            };
            (open(&app, sample, format.sample_rate, channels), None)
        }
        _ => {
            eprintln!("the audio of {app} is AAC, which this app does not play");
            return;
        }
    };
    let Some(stream) = stream else { return };
    for frame in frames {
        let bytes = match opus.as_mut() {
            Some(decoder) => decode_opus(decoder, usize::from(channels), &frame),
            None => frame,
        };
        if let Err(e) = stream.write(&bytes) {
            eprintln!("the audio of {app} stopped: {e}");
            return;
        }
    }
}

pub struct Player {
    frames: Sender<Vec<u8>>,
}

impl Player {
    pub fn start(app: &str, format: AudioStreamFormat) -> Self {
        let (frames, received) = channel();
        let app = app.to_owned();
        std::thread::spawn(move || run(app, format, received));
        Self { frames }
    }

    pub fn play(&self, frame: Vec<u8>) {
        let _ = self.frames.send(frame);
    }
}
