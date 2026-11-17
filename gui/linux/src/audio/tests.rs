use std::time::Duration;

use libpulse_binding::sample::{Format, Spec};
use phonelink_screen_mirroring::edge::AudioStreamFormat;

use super::decoding::{CODEC_AAC, CODEC_OPUS, CODEC_PCM, decode_opus, whole_frames};
use super::playback::{Playback, buffer_attr};
use super::queue::AudioQueue;

fn sample_spec(format: Format, rate: u32, channels: u8) -> Spec {
    Spec { format, rate, channels }
}

#[test]
fn the_queue_is_bounded_by_the_frame_length() {
    let phone = AudioQueue::new(40);
    assert_eq!((phone.desired, phone.maximum), (Duration::from_millis(100), Duration::from_millis(170)));
    let short = AudioQueue::new(10);
    assert_eq!((short.desired, short.maximum), (Duration::from_millis(100), Duration::from_millis(170)));
    let long = AudioQueue::new(60);
    assert_eq!((long.desired, long.maximum), (Duration::from_millis(120), Duration::from_millis(240)));
    let unnamed = AudioQueue::new(0);
    assert_eq!((unnamed.desired, unnamed.maximum), (Duration::from_millis(100), Duration::from_millis(170)));
}

/// Past the maximum, frames are dropped until what waits is back at the desired depth.
#[test]
fn frames_are_dropped_past_the_maximum_until_the_queue_drains() {
    let mut queue = AudioQueue::new(40);
    let ms = Duration::from_millis;
    assert!(queue.admit(ms(0)));
    assert!(queue.admit(ms(170)), "at the maximum frames still play");
    assert!(!queue.admit(ms(171)), "past it they are dropped");
    assert!(!queue.admit(ms(150)), "and stay dropped while above the desired depth");
    assert!(!queue.admit(ms(101)));
    assert!(queue.admit(ms(100)), "back at the desired depth they play again");
    assert!(queue.admit(ms(160)), "and go on playing up to the maximum");
}

/// The frames not yet written count with what the stream holds, which is at most its target latency.
#[test]
fn what_waits_is_the_backlog_and_the_streams_buffer() {
    let queue = AudioQueue::new(40);
    let ms = Duration::from_millis;
    assert_eq!(queue.waiting(ms(40), 0, ms(60)), ms(60));
    assert_eq!(queue.waiting(ms(40), 3, ms(60)), ms(180));
    assert_eq!(queue.waiting(ms(40), 1, ms(250)), ms(140), "an output's own latency is not the queue's");
}

/// An output device slower than the queue's maximum still plays: once the frames not yet written are
/// gone, frames play again.
#[test]
fn a_slow_output_device_does_not_silence_the_app() {
    let mut queue = AudioQueue::new(40);
    let ms = Duration::from_millis;
    assert!(!queue.admit(queue.waiting(ms(40), 2, ms(300))), "two frames behind a full stream are dropped");
    assert!(!queue.admit(queue.waiting(ms(40), 1, ms(300))));
    assert!(queue.admit(queue.waiting(ms(40), 0, ms(300))));
}

#[test]
fn the_stream_asks_for_two_frames_of_latency_and_starts_with_one() {
    // The phone's Opus at 48 kHz stereo, decoded to 16-bit samples: 192 bytes a millisecond.
    let opus = buffer_attr(&sample_spec(Format::S16le, 48_000, 2), 40);
    assert_eq!((opus.tlength, opus.prebuf), (100 * 192, 40 * 192));
    assert_eq!((opus.maxlength, opus.minreq, opus.fragsize), (u32::MAX, u32::MAX, u32::MAX));

    // 60 ms frames of 44.1 kHz float stereo: 120 and 60 ms in whole sample frames of 8 bytes.
    let pcm = buffer_attr(&sample_spec(Format::F32le, 44_100, 2), 60);
    assert_eq!((pcm.tlength, pcm.prebuf), (5292 * 8, 2646 * 8));
    assert_eq!(pcm.tlength % 8, 0);

    let unnamed = buffer_attr(&sample_spec(Format::S16le, 48_000, 1), 0);
    assert_eq!((unnamed.tlength, unnamed.prebuf), (100 * 96, u32::MAX), "the server starts it with the latency");
}

#[test]
fn pcm_plays_whole_sample_frames() {
    assert_eq!(whole_frames(7, 4), 4);
    assert_eq!(whole_frames(8, 4), 8);
    assert_eq!(whole_frames(3, 4), 0, "less than a frame is nothing to play");
}

#[test]
fn opus_decodes_to_interleaved_samples() {
    let mut encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
    let packet = encoder.encode_vec(&[0i16; 1920 * 2], 4000).unwrap();
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Stereo).unwrap();
    let samples = decode_opus(&mut decoder, 2, &packet).unwrap();
    assert_eq!(samples.len(), 1920 * 2 * 2, "40 ms of two 16-bit channels");
}

#[test]
fn a_format_this_app_does_not_play_has_no_playback() {
    let format = |codec, channels, bytes_per_sample, is_float| AudioStreamFormat {
        channels,
        sample_rate: 48_000,
        codec,
        bytes_per_sample,
        is_float,
        frame_length_ms: 40,
    };
    assert!(Playback::new("app".into(), &format(CODEC_OPUS, 2, 0, false)).is_some());
    assert!(Playback::new("app".into(), &format(CODEC_PCM, 2, 3, false)).is_some());
    assert!(Playback::new("app".into(), &format(CODEC_AAC, 2, 0, false)).is_none(), "AAC");
    assert!(Playback::new("app".into(), &format(7, 2, 0, false)).is_none(), "an unknown codec");
    assert!(Playback::new("app".into(), &format(CODEC_OPUS, 6, 0, false)).is_none(), "Opus of six channels");
    assert!(Playback::new("app".into(), &format(CODEC_PCM, 2, 3, true)).is_none(), "24-bit float");
    assert!(Playback::new("app".into(), &format(CODEC_PCM, 0, 2, false)).is_none(), "no channels");
}
