use super::*;

fn server_v4(min: u32, max: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [min, max, 1080, 2400, 10, 1000] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    frame(kind::SERVER_HANDSHAKE_V4, &b)
}

#[test]
fn negotiation_steps_down_past_the_blacklist() {
    assert_eq!(negotiate(4, 8), Some(8), "the phone's [4..8]: version 8, as Windows");
    assert_eq!(negotiate(4, 7), Some(4), "7, 6 and 5 are blacklisted");
    assert_eq!(negotiate(4, 12), Some(8), "capped at the PC's maximum");
    assert_eq!(negotiate(3, 3), Some(3));
    assert_eq!(negotiate(5, 7), None, "nothing at or above 5 survives below 8");
    assert_eq!(negotiate(8, 4), None);
}

/// A version-8 handshake and a one-mouse frame, byte for byte.
#[test]
fn version_8_handshake_and_mouse_frame() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(4, 8));

    let hs = c.take_output();
    assert_eq!(hs[0].0, SendMode::ReliableFence);
    assert_eq!(&hs[0].1[..8], &[6, 0, 0, 0, 20, 0, 0, 0], "ClientHandshakeV4, 20-byte body");
    assert_eq!(&hs[0].1[8..20], &[4, 0, 0, 0, 8, 0, 0, 0, 10, 0, 0, 0], "min 4, max 8, 10 touches");

    let reference = u64::from_le_bytes(hs[0].1[20..28].try_into().unwrap());
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    assert!(unix - reference < 5_000, "the Unix ms at which the frame clock read 0");

    c.apply(|s| {
        let m = s.mice.entry(0).or_default();
        m.x = 5;
        m.y = -1;
        m.button(0, true);
        m.wheel_y = -120;
    })
    .unwrap();

    let f = c.take_output();
    assert_eq!(f[0].0, SendMode::Unreliable);
    assert_eq!(&f[0].1[..8], &[7, 0, 0, 0, 51, 0, 0, 0], "FrameV4, 12 + 7 + 32 bytes");
    let body = &f[0].1[8..];
    assert_eq!(&body[..4], &1001u32.to_le_bytes(), "the first frame is the one after the phone's initial id");

    #[rustfmt::skip]
        let expected: [u8; 39] = [
            0, 1, 0, 0, 0, 0, 0,          // counts and indices: fingers, mouse 0, gamepads, keyboards,
                                          // vibrations, mouse feedback
            1, 0, 0, 0, 0, 0, 0, 0,       // button counters (left pressed)
            5, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, // x, y
            0, 0, 0, 0, 0x88, 0xff, 0xff, 0xff, // wheelX, wheelY (−120)
            0, 0, 0, 0, 0, 0, 0, 0,       // xRel, yRel
        ];
    assert_eq!(&body[12..], &expected);

    let time = &body[4..12];
    c.on_tick();
    let resent = c.take_output();
    assert_eq!(resent.len(), 1, "resent until acknowledged");
    assert_eq!(&resent[0].1[8 + 4..8 + 12], time, "a resend keeps the frame's own time");
}

#[test]
fn version_4_layout_has_no_version_8_fields() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(4, 7));
    assert_eq!(&c.take_output()[0].1[8..16], &[4, 0, 0, 0, 4, 0, 0, 0]);

    c.apply(|s| s.mice.entry(0).or_default().x = 5).unwrap();
    let changes = &c.take_output()[0].1[8 + 12..];
    assert_eq!(&changes[..5], &[0, 1, 0, 0, 0]);
    assert_eq!(changes.len(), 5 + 24, "a 24-byte mouse record");
}

#[test]
fn finger_fields_go_out_only_when_non_zero() {
    let mut f = Finger { x: 1, y: 2, ..Default::default() };
    f.set_state(FingerState::InContact);
    f.pressure = Some(0);
    f.width = Some(3);
    f.orientation = Some(0x40);

    let mut out = Vec::new();
    f.encode(&mut out);
    assert_eq!(out, [1, 0, 0, 0, 2, 0, 0, 0, 1, 1, 0x12, 3, 0, 0x40]);
}

#[test]
fn frames_carry_changes_since_the_ack() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(4, 8));
    c.take_output();

    c.apply(|s| {
        s.mice.entry(0).or_default().x = 5;
    })
    .unwrap();
    c.take_output();

    c.on_packet(&frame(kind::FRAME_ACK, &1001u32.to_le_bytes()));
    c.on_tick();
    assert!(c.take_output().is_empty());

    c.apply(|s| {
        s.keyboards.entry(0).or_default().key(4, true);
    })
    .unwrap();
    let f = c.take_output();
    let changes = &f[0].1[8 + 12..];
    assert_eq!(&changes[..7], &[0, 0, 0, 1, 0, 0, 0], "only keyboard 0 changed since the ack");
    assert_eq!(&changes[7..], &[1, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0], "one key: usage 4, counter 1; no history");
}

fn frame_id(packet: &[u8]) -> u32 {
    u32::from_le_bytes(packet[8..12].try_into().unwrap())
}

#[test]
fn an_action_that_changes_nothing_sends_nothing_but_takes_an_id() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(4, 8));
    c.take_output();

    c.apply(|s| {
        s.keyboards.entry(0).or_default().key(4, false);
    })
    .unwrap();
    assert!(c.take_output().is_empty(), "releasing a key already up");
    c.on_tick();
    assert!(c.take_output().is_empty(), "nothing to resend");

    c.apply(|s| s.mice.entry(0).or_default().x = 5).unwrap();
    assert_eq!(frame_id(&c.take_output()[0].1), 1002, "the unsent action took 1001");

    c.apply(|s| s.mice.entry(0).or_default().x = 5).unwrap();
    assert!(c.take_output().is_empty(), "a move to where the pointer is");
    c.on_tick();
    let resent = c.take_output();
    assert_eq!(frame_id(&resent[0].1), 1002, "the latest frame sent is resent");

    c.apply(|s| s.mice.entry(0).or_default().x = 6).unwrap();
    assert_eq!(frame_id(&c.take_output()[0].1), 1004);
}

#[test]
fn input_before_the_handshake_goes_with_the_first_frame_after_it() {
    let mut c = InputCore::new();
    c.apply(|s| {
        s.keyboards.entry(0).or_default().key(4, true);
    })
    .unwrap();
    assert!(c.take_output().is_empty(), "no frame before the handshake");

    c.on_packet(&server_v4(4, 8));
    assert_eq!(c.take_output().len(), 1, "only the ClientHandshake");

    // An action that changes nothing on its own still sends the kept change.
    c.apply(|s| {
        s.keyboards.entry(0).or_default().key(4, true);
    })
    .unwrap();

    let f = c.take_output();
    assert_eq!(frame_id(&f[0].1), 1001);
    let changes = &f[0].1[8 + 12..];
    assert_eq!(&changes[..7], &[0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(&changes[7..10], &[1, 4, 1], "the key pressed before the handshake");
}

#[test]
fn frame_times_are_microseconds() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(4, 8));
    c.take_output();

    std::thread::sleep(Duration::from_millis(3));
    c.apply(|s| s.mice.entry(0).or_default().x = 5).unwrap();

    let f = c.take_output();
    let us = u64::from_le_bytes(f[0].1[12..20].try_into().unwrap());
    let elapsed = c.started.elapsed().as_micros() as u64;
    assert!((3_000..=elapsed).contains(&us), "{us} µs since the frame clock read 0");
}

#[test]
fn unsupported_versions_fail() {
    let mut c = InputCore::new();
    c.on_packet(&server_v4(5, 7));
    assert!(c.failed().is_some());
    assert!(c.apply(|s| s.mice.entry(0).or_default().x = 5).is_err(), "no input on a failed channel");
}
