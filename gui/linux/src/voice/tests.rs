//! The voice link's bytes: addresses, mSBC framing, the codec, the exchange of packets, the keypad
//! tones.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::carry::{Packets, Sound, exchange_packets};
use super::exchange::{Exchange, FrameCodec};
use super::msbc::{Deframer, FRAME_LEN, FRAME_SAMPLES, FRAMED_LEN, Framer};
use super::sbc::Msbc;
use super::sco::Address;
use super::sound::{TONE_MS, TONE_RATE, key_frequencies, key_tone};

#[test]
fn an_address_is_read_in_either_spelling_and_nothing_else() {
    let colons = Address::parse("AA:BB:CC:DD:EE:FF").expect("colons");
    assert_eq!(Address::parse("aa-bb-cc-dd-ee-ff"), Some(colons));
    assert_ne!(Address::parse("ff-ee-dd-cc-bb-aa"), Some(colons));

    for bad in ["", "aa-bb-cc-dd-ee", "aa-bb-cc-dd-ee-ff-00", "aa-bb-cc-dd-ee-fg", "aabbccddeeff", "a-bb-cc-dd-ee-ff"] {
        assert_eq!(Address::parse(bad), None, "{bad:?} is no address");
    }
}

/// A coded frame whose bytes after the frame mark are `fill`.
fn coded(fill: u8) -> [u8; FRAME_LEN] {
    let mut frame = [fill; FRAME_LEN];
    frame[0] = 0xAD;
    frame
}

#[test]
fn frames_are_numbered_in_turn_and_padded() {
    let mut framer = Framer::default();
    let headers: Vec<[u8; 2]> = (0..5).map(|_| framer.frame(&coded(7))).map(|f| [f[0], f[1]]).collect();
    assert_eq!(headers, [[0x01, 0x08], [0x01, 0x38], [0x01, 0xC8], [0x01, 0xF8], [0x01, 0x08]]);

    let framed = Framer::default().frame(&coded(7));
    assert_eq!((framed.len(), framed[2], framed[FRAMED_LEN - 1]), (60, 0xAD, 0x00));
}

#[test]
fn frames_are_found_across_packets_of_any_length() {
    let mut framer = Framer::default();
    let stream: Vec<u8> = [1u8, 2, 3].iter().flat_map(|fill| framer.frame(&coded(*fill))).collect();

    for packet_len in [1, 7, 24, 48, 60, 100, 180] {
        let mut deframer = Deframer::default();
        let found: Vec<u8> =
            stream.chunks(packet_len).flat_map(|packet| deframer.push(packet)).map(|frame| frame[1]).collect();
        assert_eq!(found, [1, 2, 3], "packets of {packet_len} bytes");
    }
}

#[test]
fn bytes_before_a_frame_are_dropped() {
    let mut deframer = Deframer::default();
    let mut stream = vec![0x00, 0x01, 0x55, 0xAD, 0x01];
    stream.extend(Framer::default().frame(&coded(9)));

    let found = deframer.push(&stream);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0][1], 9);
}

/// A codec that keeps a frame's first sample in the coded frame's second byte.
struct MarkingCodec;

impl FrameCodec for MarkingCodec {
    fn encode(&mut self, samples: &[i16; FRAME_SAMPLES]) -> Option<[u8; FRAME_LEN]> {
        Some(coded(samples[0] as u8))
    }
    fn decode(&mut self, frame: &[u8; FRAME_LEN]) -> Option<[i16; FRAME_SAMPLES]> {
        (frame[1] != 0xEE).then_some([i16::from(frame[1]); FRAME_SAMPLES])
    }
}

#[test]
fn narrow_band_voice_crosses_as_samples_whatever_the_packet_length() {
    let mut exchange = Exchange::narrow_band();
    assert_eq!(exchange.sample_rate(), 8_000);

    assert_eq!(exchange.heard(&[0x01, 0x00, 0x02]), [1], "a cut sample waits");
    assert_eq!(exchange.heard(&[0x00, 0xFF, 0xFF]), [2, -1]);

    let mut asked = Vec::new();
    let mut microphone = |count: usize| {
        asked.push(count);
        vec![0x0102; count]
    };
    assert_eq!(exchange.reply(3, &mut microphone), [0x02, 0x01, 0x02]);
    assert_eq!(exchange.reply(3, &mut microphone), [0x01, 0x02, 0x01]);
    assert_eq!(asked, [2, 1], "only the samples a packet still needs are taken");
}

#[test]
fn wide_band_voice_is_coded_a_frame_at_a_time_and_sent_in_the_packets_length() {
    let mut exchange = Exchange::wide_band(Box::new(MarkingCodec));
    assert_eq!(exchange.sample_rate(), 16_000);

    let mut next = 0i16;
    let mut microphone = |count: usize| {
        next += 1;
        vec![next; count]
    };
    let sent: Vec<u8> = (0..5).flat_map(|_| exchange.reply(24, &mut microphone)).collect();
    assert_eq!(sent.len(), 120);
    assert_eq!(sent[..4], [0x01, 0x08, 0xAD, 1]);
    assert_eq!(sent[60..64], [0x01, 0x38, 0xAD, 2]);
    assert_eq!(next, 2, "two frames fill five packets of 24 bytes");

    let mut framer = Framer::default();
    let received: Vec<u8> = [5u8, 0xEE, 6].iter().flat_map(|fill| framer.frame(&coded(*fill))).collect();
    let heard: Vec<i16> = received.chunks(48).flat_map(|packet| exchange.heard(packet)).collect();
    assert_eq!(heard.len(), 3 * FRAME_SAMPLES);
    assert_eq!(
        (heard[0], heard[FRAME_SAMPLES], heard[2 * FRAME_SAMPLES]),
        (5, 0, 6),
        "a frame that does not decode is silence"
    );
}

#[test]
fn the_system_codec_codes_a_frame_and_decodes_it_back() {
    let mut codec = Msbc::new().expect("the SBC library starts");
    // A 1 kHz tone at 16 kHz.
    let tone = |frame: usize| -> [i16; FRAME_SAMPLES] {
        std::array::from_fn(|i| {
            let t = (frame * FRAME_SAMPLES + i) as f32 / 16_000.0;
            ((std::f32::consts::TAU * 1000.0 * t).sin() * 10_000.0) as i16
        })
    };

    let mut loudest = 0;
    for frame in 0..8 {
        let coded = codec.encode(&tone(frame)).expect("a frame codes");
        assert_eq!(coded[0], 0xAD, "an mSBC frame starts with its sync word");
        let decoded = codec.decode(&coded).expect("a frame decodes");
        loudest = decoded.iter().map(|sample| sample.unsigned_abs()).max().unwrap_or(0).max(loudest);
    }
    assert!((8_000..12_000).contains(&loudest), "the tone comes back at its level: {loudest}");

    assert_eq!(codec.decode(&[0u8; FRAME_LEN]), None, "bytes that are no frame do not decode");
}

#[test]
fn every_keypad_key_has_its_two_tones() {
    let cases = [
        ('1', (697.0, 1209.0)),
        ('5', (770.0, 1336.0)),
        ('9', (852.0, 1477.0)),
        ('*', (941.0, 1209.0)),
        ('0', (941.0, 1336.0)),
        ('#', (941.0, 1477.0)),
    ];
    for (key, expected) in cases {
        assert_eq!(key_frequencies(key), Some(expected), "key {key}");
    }
    assert_eq!(key_frequencies('A'), None);

    let tone = key_tone(697.0, 1209.0);
    assert_eq!(tone.len(), (TONE_RATE * TONE_MS / 1000) as usize);
    assert_eq!((tone[0], tone[tone.len() - 1].unsigned_abs() < 400), (0, true), "the tone fades in and out");
}

/// A link whose packets a test scripts: each read takes the next step, and the link ends after the
/// last one.
#[derive(Default)]
struct ScriptedLink {
    /// `Ok(None)` is a wait with no packet.
    reads: VecDeque<io::Result<Option<Vec<u8>>>>,
    written: Vec<Vec<u8>>,
}

impl Packets for ScriptedLink {
    fn read(&mut self, packet: &mut [u8], _timeout: Duration) -> io::Result<Option<usize>> {
        match self.reads.pop_front() {
            None => Ok(Some(0)),
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(bytes))) => {
                packet[..bytes.len()].copy_from_slice(&bytes);
                Ok(Some(bytes.len()))
            }
            Some(Err(e)) => Err(e),
        }
    }

    fn write(&mut self, packet: &[u8]) -> io::Result<()> {
        self.written.push(packet.to_vec());
        Ok(())
    }
}

/// Runs the loop over `link` with a microphone that says `spoken` in every sample; what was heard.
fn carry(link: &mut ScriptedLink, stopped: bool, spoken: i16) -> (io::Result<()>, Vec<i16>) {
    let mut heard = Vec::new();
    let sound = Sound {
        heard: &mut |samples: &[i16]| heard.extend_from_slice(samples),
        spoken: &mut |count| vec![spoken; count],
    };
    let mut exchange = Exchange::narrow_band();
    let result = exchange_packets(link, 64, &mut exchange, sound, &AtomicBool::new(stopped), Duration::from_millis(1));
    (result, heard)
}

#[test]
fn every_packet_from_the_phone_is_played_and_answered_with_one_as_long_until_the_link_ends() {
    let mut link = ScriptedLink::default();
    link.reads.extend([Ok(Some(vec![1, 0, 2, 0])), Ok(None), Ok(Some(vec![3, 0, 4, 0, 5, 0]))]);

    let (result, heard) = carry(&mut link, false, 0x0707);
    assert!(result.is_ok(), "the link's end ends the loop: {result:?}");
    assert_eq!(heard, [1, 2, 3, 4, 5]);
    assert_eq!(link.written, [vec![7; 4], vec![7; 6]], "nothing is sent for a wait with no packet");
}

#[test]
fn a_stopped_link_reads_nothing_more() {
    let mut link = ScriptedLink::default();
    link.reads.push_back(Ok(Some(vec![1, 0])));

    let (result, heard) = carry(&mut link, true, 0);
    assert!(result.is_ok() && heard.is_empty() && link.written.is_empty());
    assert_eq!(link.reads.len(), 1);
}

#[test]
fn a_failed_read_ends_the_loop_with_its_error() {
    let mut link = ScriptedLink::default();
    link.reads.extend([Ok(Some(vec![1, 0])), Err(io::Error::other("the adapter went away"))]);

    let (result, heard) = carry(&mut link, false, 0);
    assert_eq!(result.map_err(|e| e.to_string()), Err("the adapter went away".to_owned()));
    assert_eq!((heard, link.written.len()), (vec![1], 1));
}
