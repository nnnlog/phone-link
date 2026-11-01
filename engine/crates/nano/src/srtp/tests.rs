use super::*;

#[test]
fn rfc3711_key_derivation() {
    // RFC 3711 Appendix B.3.
    let master = MasterKey {
        key: hex::decode("E1F97A0D3E018BE0D64FA32C06DE4139").unwrap().try_into().unwrap(),
        salt: hex::decode("0EC675AD498AFEEBB6960B3AABE6").unwrap().try_into().unwrap(),
    };

    assert_eq!(hex::encode_upper(derive(&master, label::ENCRYPTION, 16)), "C61E7A93744F39EE10734AFE3FF7A087");
    assert_eq!(hex::encode_upper(derive(&master, label::SALT, 14)), "30CBBC08863D8C85D49DB34A9AE1");
}

fn packet(seq: u16) -> RtpPacket {
    RtpPacket { payload_type: 0x23, marker: false, timestamp: 0, ssrc: 3, sequence: seq, payload: b"hi".to_vec() }
}

#[test]
fn protect_unprotect() {
    let master = MasterKey { key: [1; 16], salt: [2; 14] };
    let (mut a, mut b) = (SrtpSession::new(&master), SrtpSession::new(&master));

    let wire = a.protect(&packet(0xBEEF));
    assert_eq!(wire.len(), OVERHEAD + 2);
    assert_eq!(&wire[..4], &[0x80, 0x23, 0, 0], "first index is 0 and the wire SEQ is the index");
    assert_eq!(b.unprotect(&wire).unwrap(), packet(0xBEEF));
    assert_eq!(b.unprotect(&wire), Err(SrtpError::Replay));

    let mut tampered = a.protect(&packet(1));
    tampered[13] ^= 1;
    assert_eq!(b.unprotect(&tampered), Err(SrtpError::Authentication));

    // The shared key and salt give both directions the same nonces: b's first packet uses index 0 too.
    let back = b.protect(&packet(9));
    assert_eq!(&back[2..4], &[0, 0]);
    assert_eq!(a.unprotect(&back).unwrap().sequence, 9);
}

#[test]
fn index_rolls_over_and_window_holds() {
    let master = MasterKey { key: [5; 16], salt: [6; 14] };
    let (mut tx, mut rx) = (SrtpSession::new(&master), SrtpSession::new(&master));
    let mut late = Vec::new();
    for i in 0..70_000u32 {
        let wire = tx.protect(&packet(i as u16));
        if i == 65_530 {
            late = wire;
            continue;
        }
        rx.unprotect(&wire).unwrap();
    }

    assert_eq!(rx.highest, Some(69_999));
    assert_eq!(rx.unprotect(&late), Err(SrtpError::Replay), "older than the window");

    let a = tx.protect(&packet(1));
    let b = tx.protect(&packet(2));
    rx.unprotect(&b).unwrap();
    rx.unprotect(&a).unwrap();
}
