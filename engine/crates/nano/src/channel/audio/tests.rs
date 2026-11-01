use super::*;

fn handshake(version: u32, formats: &[AudioFormat]) -> Vec<u8> {
    let mut b = version.to_le_bytes().to_vec();
    b.extend_from_slice(&9u64.to_le_bytes());
    b.extend_from_slice(&(formats.len() as u32).to_le_bytes());
    for f in formats {
        f.encode(&mut b);
    }
    b
}

#[test]
fn format_choice() {
    let opus = |ch, rate| AudioFormat { channels: ch, sample_rate: rate, codec: codec::OPUS, pcm: None };
    let pcm =
        |bytes, float| AudioFormat { channels: 2, sample_rate: 48_000, codec: codec::PCM, pcm: Some((bytes, float)) };

    assert_eq!(choose_format(&[pcm(2, 0), opus(1, 48_000), opus(2, 48_000), opus(2, 16_000)]), opus(2, 48_000));
    assert_eq!(choose_format(&[pcm(2, 0), pcm(4, 1), pcm(4, 0), pcm(3, 0)]), pcm(4, 1));
    assert_eq!(choose_format(&[]), AudioFormat::FALLBACK);
}

#[test]
fn pcm_carries_bytes_per_sample() {
    let mut b = Vec::new();
    AudioFormat { channels: 2, sample_rate: 48_000, codec: codec::PCM, pcm: None }.encode(&mut b);
    assert_eq!(&b[12..], &[2, 0, 0, 0, 0, 0, 0, 0], "16-bit integer PCM is 2 bytes per sample");

    let f = AudioFormat { channels: 1, sample_rate: 16_000, codec: codec::PCM, pcm: Some((4, 1)) };
    let mut b = Vec::new();
    f.encode(&mut b);
    assert_eq!(AudioFormat::decode(&mut Reader::new(&b)).unwrap(), f);
}

#[test]
fn handshake_starts_and_every_frame_is_delivered() {
    let mut c = AudioCore::new();
    c.on_packet(0x24, &handshake(7, &[]));

    let out = c.take_output();
    assert_eq!(out[0].0, kind::CLIENT_HANDSHAKE);
    assert_eq!(&out[0].1[4..], &[2, 0, 0, 0, 0x80, 0xBB, 0, 0, 0, 0, 0, 0], "Opus, 2 ch, 48 kHz");
    assert_eq!(out[1], (kind::CONTROL, 0x10u32.to_le_bytes().to_vec()));

    let frame = |id: u32| {
        let mut b = 0u32.to_le_bytes().to_vec();
        b.extend_from_slice(&id.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        b.push(7);
        b
    };
    c.take_events();

    // One capture buffer cut into packets shares its id; a late packet still plays.
    c.on_packet(0x27, &frame(5));
    c.on_packet(0x27, &frame(5));
    c.on_packet(0x27, &frame(4));
    c.on_packet(0x27, &frame(6));

    let ids: Vec<u32> = c
        .take_events()
        .into_iter()
        .map(|e| match e {
            AudioEvent::Frame(f) => f.frame_id,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![5, 5, 4, 6]);

    let mut bad = AudioCore::new();
    bad.on_packet(0x24, &handshake(6, &[]));
    assert!(bad.failed());
}

#[test]
fn no_control_between_reinitialize_and_the_next_handshake() {
    let mut c = AudioCore::new();
    c.on_packet(0x24, &handshake(7, &[]));
    c.take_output();

    c.reinitialize();
    assert_eq!(c.take_output(), vec![(kind::CONTROL, vec![0x40, 0, 0, 0])]);

    c.stop();
    c.reinitialize();
    assert!(c.take_output().is_empty(), "the phone is not streaming until it handshakes again");

    c.on_packet(0x24, &handshake(7, &[]));
    let out = c.take_output();
    assert_eq!(out[0].0, kind::CLIENT_HANDSHAKE);
    assert_eq!(out[1], (kind::CONTROL, vec![0x10, 0, 0, 0]), "started again");
}
