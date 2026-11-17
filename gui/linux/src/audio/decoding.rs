//! How an app's audio frames become samples: the codecs the phone names, Opus decoded to 16-bit
//! samples, and PCM played as it comes.

use libpulse_binding::sample::Format;
use phonelink_screen_mirroring::edge::AudioStreamFormat;

pub(super) const CODEC_OPUS: u32 = 0;
pub(super) const CODEC_PCM: u32 = 1;
pub(super) const CODEC_AAC: u32 = 2;
const OPUS_MAX_FRAME: usize = 5760;

/// The sample format of PCM in `format`; `None` for a layout this app does not play.
pub(super) fn pcm_format(format: &AudioStreamFormat) -> Option<Format> {
    match (format.is_float, format.bytes_per_sample) {
        (true, 4) => Some(Format::F32le),
        (false, 2) => Some(Format::S16le),
        (false, 3) => Some(Format::S24le),
        (false, 4) => Some(Format::S32le),
        _ => None,
    }
}

pub(super) fn opus_channels(channels: u32) -> Option<opus::Channels> {
    match channels {
        1 => Some(opus::Channels::Mono),
        2 => Some(opus::Channels::Stereo),
        _ => None,
    }
}

/// An Opus packet decoded to interleaved 16-bit samples, as little-endian bytes.
pub(super) fn decode_opus(decoder: &mut opus::Decoder, channels: usize, packet: &[u8]) -> Result<Vec<u8>, opus::Error> {
    let mut pcm = vec![0i16; OPUS_MAX_FRAME * channels];
    let samples = decoder.decode(packet, &mut pcm, false)?;
    pcm.truncate(samples * channels);
    Ok(pcm.iter().flat_map(|sample| sample.to_le_bytes()).collect())
}

/// The bytes of whole sample frames, `frame_size` bytes each, in `len` bytes of PCM: a part frame at the
/// end is cut.
pub(super) fn whole_frames(len: usize, frame_size: usize) -> usize {
    len / frame_size * frame_size
}

/// How an app's frames become samples for its stream.
pub(super) enum Decoding {
    /// Opus packets, decoded to 16-bit samples.
    Opus(opus::Decoder),
    /// Interleaved PCM, played as it comes.
    Pcm,
}
