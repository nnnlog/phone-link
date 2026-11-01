use super::*;

fn packet(kind: u32, generation: u32, body: &[u8]) -> Vec<u8> {
    let mut p = kind.to_le_bytes().to_vec();
    p.extend_from_slice(&generation.to_le_bytes());
    p.extend_from_slice(&(body.len() as u32).to_le_bytes());
    p.extend_from_slice(body);
    p
}

fn server_handshake(formats: &[VideoFormat]) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [6u32, 1080, 2400, 30] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(&1_700_000_000_000u64.to_le_bytes());
    b.extend_from_slice(&(formats.len() as u32).to_le_bytes());
    for f in formats {
        f.encode(&mut b);
    }
    packet(1, 0, &b)
}

fn data(frame_id: u32, flags: u32, total: u32, offset: u32, bytes: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&frame_id.to_le_bytes());
    b.extend_from_slice(&5u64.to_le_bytes());
    for v in [0, total, 1, offset, 0, bytes.len() as u32] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(bytes);
    packet(4, 0, &b)
}

#[test]
fn format_choice() {
    let f = |w, h, c| VideoFormat { fps: 30, width: w, height: h, codec: c, rgb: None };

    assert_eq!(choose_format(&[f(1080, 2400, 0)]), f(1080, 2400, 0));
    assert_eq!(choose_format(&[f(640, 480, 0), f(320, 240, 0)]), f(320, 240, 0));
    assert_eq!(choose_format(&[f(1920, 1080, 0), f(1280, 720, 1)]), f(1280, 720, 1), "codec orders first");
    assert_eq!(choose_format(&[]), VideoFormat::FALLBACK);
}

#[test]
fn sequence_start_frames_and_loss() {
    let mut c = VideoCore::new();
    c.start();
    assert!(c.take_output().is_empty(), "remembered until the handshake");

    let phone_format = VideoFormat { fps: 30, width: 1080, height: 2400, codec: 0, rgb: None };
    c.on_packet(&server_handshake(&[phone_format]));
    let out = c.take_output();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].0, SendMode::ReliableFence);
    assert_eq!(&out[0].1[..4], &2u32.to_le_bytes(), "ClientHandshake");
    assert_eq!(&out[1].1[12..16], &0x30u32.to_le_bytes(), "StartStream | RequestKeyframe");
    assert!(matches!(c.take_events()[0], VideoEvent::Ready { format, .. } if format == phone_format));

    let first = c.expected;
    c.on_packet_at(&data(first, 0x06, 6, 0, b"abc"), 0);
    c.on_packet_at(&data(first, 0x06, 6, 3, b"def"), 0);
    let frame = match &c.take_events()[0] {
        VideoEvent::Frame(f) => f.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!((frame.data.as_slice(), frame.keyframe), (&b"abcdef"[..], true));

    c.frame_presented(first, 99);
    assert_eq!(&c.take_output()[0].1[12..16], &0x81u32.to_le_bytes());
}

/// Data ending past its frame, like any malformed packet, stops the stream and fails the channel;
/// before `StartStream` there is nothing to stop.
#[test]
fn a_malformed_packet_stops_the_stream_and_fails_the_channel() {
    let (mut c, first) = started();
    c.on_packet_at(&data(first, 0x06, 2, 1, b"yy"), 0);
    assert!(c.take_events().is_empty());
    assert_eq!(control_flags(&c.take_output()), vec![0x08], "StopStream");
    assert!(c.failed());

    let mut c = VideoCore::new();
    c.on_packet(&server_handshake(&[]));
    c.take_output();
    c.on_packet(&packet(4, 0, &[1, 2]));
    assert!(c.take_output().is_empty());
    assert!(c.failed());
}

fn started() -> (VideoCore, u32) {
    let mut c = VideoCore::new();
    c.on_packet_at(&server_handshake(&[VideoFormat { fps: 30, width: 1080, height: 2400, codec: 0, rgb: None }]), 0);
    c.start();
    c.take_output();
    c.take_events();
    let first = c.initial;
    (c, first)
}

fn frames(c: &mut VideoCore) -> Vec<u32> {
    c.take_events()
        .into_iter()
        .filter_map(|e| match e {
            VideoEvent::Frame(f) => Some(f.frame_id),
            _ => None,
        })
        .collect()
}

fn control_flags(out: &[(SendMode, Vec<u8>)]) -> Vec<u32> {
    out.iter().map(|(_, p)| u32::from_le_bytes(p[12..16].try_into().unwrap())).collect()
}

/// A frame that completes after a missing one waits one defragment timeout for it; a late arrival
/// within it keeps the order, and nothing is reported.
#[test]
fn a_later_frame_waits_for_the_missing_one() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    c.on_packet_at(&data(first + 2, 0x04, 1, 0, b"c"), 10);
    assert_eq!(frames(&mut c), vec![first]);

    c.on_packet_at(&data(first + 1, 0x04, 1, 0, b"b"), 10 + DEFRAG_FRAME_TIMEOUT_MS);
    assert_eq!(frames(&mut c), vec![first + 1, first + 2]);
    assert!(c.take_output().is_empty(), "no loss");
}

/// Past the timeout the gap is reported lost with a refresh request (`0x122`) and the waiting
/// frame is shown; the missing frame's late packets are then for a passed frame.
#[test]
fn past_the_timeout_the_gap_is_lost_with_a_refresh() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    c.on_packet_at(&data(first + 1, 0x04, 2, 0, b"b"), 0);
    c.on_packet_at(&data(first + 3, 0x04, 1, 0, b"d"), 5);
    assert_eq!(frames(&mut c), vec![first]);

    c.on_packet_at(&data(first + 4, 0x04, 2, 0, b"e"), 5 + DEFRAG_FRAME_TIMEOUT_MS + 1);
    assert_eq!(frames(&mut c), vec![first + 3]);
    let out = c.take_output();
    assert_eq!(control_flags(&out), vec![0x122]);
    assert_eq!(&out[0].1[16..24], &[(first + 1).to_le_bytes(), (first + 2).to_le_bytes()].concat(), "inclusive range");

    c.on_packet_at(&data(first + 1, 0x04, 2, 1, b"b"), 40);
    assert!(frames(&mut c).is_empty(), "a passed frame never shows");
}

/// A complete keyframe after a gap shows at once; the partial frames it passed are reported lost
/// without a request (`0x02`).
#[test]
fn a_keyframe_jumps_the_gap() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    c.on_packet_at(&data(first + 1, 0x04, 2, 0, b"b"), 0);
    c.on_packet_at(&data(first + 2, 0x06, 1, 0, b"k"), 1);
    assert_eq!(frames(&mut c), vec![first, first + 2]);
    let out = c.take_output();
    assert_eq!(control_flags(&out), vec![0x02]);
    assert_eq!(&out[0].1[16..24], &[(first + 1).to_le_bytes(), (first + 1).to_le_bytes()].concat());
}

/// While a keyframe older than it is still arriving, a complete frame waits the longer timeout;
/// a gap closed by a keyframe past its timeout is reported with a keyframe request (`0x22`).
#[test]
fn a_pending_keyframe_doubles_the_wait() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 2, 0, b"k"), 0);
    c.on_packet_at(&data(first + 1, 0x04, 1, 0, b"b"), 0);
    c.on_packet_at(&data(first + 2, 0x04, 1, 0, b"c"), DEFRAG_FRAME_TIMEOUT_MS + 1);
    assert!(frames(&mut c).is_empty(), "the keyframe timeout applies");

    c.on_packet_at(&data(first + 3, 0x06, 2, 0, b"q"), DEFRAG_KEYFRAME_TIMEOUT_MS + 1);
    assert_eq!(frames(&mut c), vec![first + 1, first + 2]);
    assert_eq!(control_flags(&c.take_output()), vec![0x122]);

    let (mut c, first) = started();
    c.on_packet_at(&data(first + 1, 0x06, 1, 0, b"k"), 0);
    assert_eq!(frames(&mut c), vec![first + 1], "a keyframe does not wait");
    assert_eq!(control_flags(&c.take_output()), Vec::<u32>::new(), "nothing of the gap had arrived");

    let (mut c, first) = started();
    c.on_packet_at(&data(first, 0x04, 2, 0, b"a"), 0);
    c.on_packet_at(&data(first + 1, 0x06, 1, 0, b"k"), 0);
    assert_eq!(frames(&mut c), vec![first + 1]);
    assert_eq!(control_flags(&c.take_output()), vec![0x02]);
}

/// A gap reported lost whose frames left nothing behind keeps the next drop of passed frames
/// silent, in a later pass; that drop ends it.
#[test]
fn a_reported_gap_silences_the_next_drop_of_passed_frames() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    c.on_packet_at(&data(first + 2, 0x04, 1, 0, b"c"), 0);
    c.on_packet_at(&data(first + 2, 0x04, 1, 0, b"c"), DEFRAG_FRAME_TIMEOUT_MS + 1);
    assert_eq!(frames(&mut c), vec![first, first + 2]);
    assert_eq!(control_flags(&c.take_output()), vec![0x122], "nothing of the gap had arrived");

    c.on_packet_at(&data(first + 3, 0x04, 2, 0, b"d"), 20);
    c.on_packet_at(&data(first + 4, 0x06, 1, 0, b"k"), 20);
    assert_eq!(frames(&mut c), vec![first + 4]);
    assert!(c.take_output().is_empty(), "silenced by the earlier report");

    c.on_packet_at(&data(first + 5, 0x04, 2, 0, b"f"), 21);
    c.on_packet_at(&data(first + 6, 0x06, 1, 0, b"k"), 21);
    assert_eq!(frames(&mut c), vec![first + 6]);
    assert_eq!(control_flags(&c.take_output()), vec![0x02]);
}

/// A packet overlapping bytes already received is ignored whole; one filling the gap completes
/// the frame.
#[test]
fn an_overlapping_packet_is_ignored() {
    let (mut c, first) = started();

    c.on_packet_at(&data(first, 0x06, 4, 0, b"ab"), 0);
    c.on_packet_at(&data(first, 0x06, 4, 1, b"xyz"), 0);
    assert!(frames(&mut c).is_empty(), "the overlap filled nothing");

    c.on_packet_at(&data(first, 0x06, 4, 2, b"cd"), 0);
    match &c.take_events()[..] {
        [VideoEvent::Frame(f)] => assert_eq!(f.data.as_slice(), b"abcd"),
        other => panic!("{other:?}"),
    }
}

/// More than 20 Data packets in a row for passed frames reinitialize the stream, once.
#[test]
fn old_packets_reinitialize_once() {
    let (mut c, first) = started();
    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    frames(&mut c);

    for _ in 0..OLD_PACKETS_BEFORE_REINITIALIZE {
        c.on_packet_at(&data(first.wrapping_sub(5), 0x04, 1, 0, b"x"), 1);
    }
    assert!(c.take_output().is_empty());

    c.on_packet_at(&data(first.wrapping_sub(5), 0x04, 1, 0, b"x"), 1);
    assert_eq!(control_flags(&c.take_output()), vec![0x40]);

    for _ in 0..30 {
        c.on_packet_at(&data(first.wrapping_sub(5), 0x04, 1, 0, b"x"), 1);
    }
    assert!(c.take_output().is_empty(), "once per channel");

    // A packet of a current frame resets the count.
    let (mut c, first) = started();
    for _ in 0..OLD_PACKETS_BEFORE_REINITIALIZE {
        c.on_packet_at(&data(first.wrapping_sub(1), 0x04, 1, 0, b"x"), 1);
    }

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 1);
    c.on_packet_at(&data(first.wrapping_sub(1), 0x04, 1, 0, b"x"), 1);
    assert!(c.take_output().is_empty());
}

#[test]
fn wrong_version_fails_and_generations() {
    let mut c = VideoCore::new();
    let mut bad = server_handshake(&[]);
    bad[12] = 5;
    c.on_packet(&bad);
    assert!(c.failed());

    let mut c = VideoCore::new();
    c.on_packet(&packet(3, 2, &0u32.to_le_bytes()));
    c.on_packet(&server_handshake(&[]));
    assert!(c.take_events().is_empty(), "older generation dropped");

    c.reinitialize();
    assert!(c.take_output().is_empty(), "no handshake yet");
}

/// The phone answers a reinitialize (the PC's or its own) with a new handshake and sends no frames
/// until `StartStream` comes again.
#[test]
fn a_handshake_while_streaming_starts_the_stream_again() {
    let phone_format = VideoFormat { fps: 30, width: 1080, height: 2400, codec: 0, rgb: None };
    let mut c = VideoCore::new();
    c.on_packet(&server_handshake(&[phone_format]));
    c.start();
    c.take_output();

    c.reinitialize();
    assert_eq!(c.take_output()[0].1, [3, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0x40, 0, 0, 0]);

    let mut again = server_handshake(&[phone_format]);
    again[4] = 1;
    c.on_packet(&again);

    let out = c.take_output();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].0, SendMode::ReliableFence);

    let mut hs = vec![2, 0, 0, 0, 1, 0, 0, 0, 20, 0, 0, 0];
    hs.extend_from_slice(&c.expected.to_le_bytes());
    hs.extend_from_slice(&[30, 0, 0, 0, 0x38, 4, 0, 0, 0x60, 9, 0, 0, 0, 0, 0, 0]);
    assert_eq!(out[0].1, hs, "ClientHandshake in the new generation");
    assert_eq!(out[1], (SendMode::ReliableFence, vec![3, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 0x30, 0, 0, 0]));
}

/// The phone numbers its frames from the handshake's initial id again on every `StartStream`, so a
/// stop and start on the same handshake takes them as new.
#[test]
fn a_start_after_a_stop_expects_the_initial_id_again() {
    let mut c = VideoCore::new();
    c.on_packet(&server_handshake(&[VideoFormat { fps: 30, width: 1080, height: 2400, codec: 0, rgb: None }]));
    c.start();
    let first = c.initial;

    c.on_packet_at(&data(first, 0x06, 1, 0, b"a"), 0);
    c.on_packet_at(&data(first + 1, 0x04, 1, 0, b"b"), 0);
    assert_eq!(c.take_events().len(), 3, "ready and two frames");

    c.stop();
    c.start();
    c.take_output();

    c.on_packet_at(&data(first, 0x06, 1, 0, b"c"), 0);
    match &c.take_events()[..] {
        [VideoEvent::Frame(f)] => assert_eq!((f.frame_id, f.data.as_slice()), (first, &b"c"[..])),
        other => panic!("{other:?}"),
    }
    assert!(c.take_output().is_empty(), "no loss reported");
}

#[test]
fn frame_ack_bytes() {
    let mut c = VideoCore::new();
    c.frame_presented(7, 1_000_000);
    assert_eq!(
        c.take_output(),
        vec![(
            SendMode::Reliable,
            vec![3, 0, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0x81, 0, 0, 0, 7, 0, 0, 0, 0x40, 0x42, 0x0f, 0, 0, 0, 0, 0]
        )]
    );
}
