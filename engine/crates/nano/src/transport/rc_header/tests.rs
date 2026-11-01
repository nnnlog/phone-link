use super::*;

#[test]
fn header_round_trips_in_field_order() {
    let h = RcHeader {
        log_window: PC_LOG_WINDOW,
        ack: Some(Ack { sn: 0x1234, received_ts: 0xABCDEF, ack_delay_ms: 3, shift: 2, gaps: vec![9, 8] }),
        overhead: Some(30),
        delay_ack_info: Some((6, 20)),
        ack_of_acks: Some(0x0102),
        data_sn: Some(0x0304),
        ack_vector: None,
        extension: None,
    };

    let mut out = Vec::new();
    h.encode(&mut out);

    assert_eq!(&out[..2], &(0xD155u16).to_le_bytes(), "flags 0x155 with log window 0xD");
    assert_eq!(&out[2..10], &[0x34, 0x12, 0xEF, 0xCD, 0xAB, 3, 0x22, 9]);
    assert_eq!(RcHeader::decode(&out).unwrap(), (h, out.len()));
}

#[test]
fn ack_vector_codes() {
    // base missing, then 10 received, then 3 missing, 2 received.
    let mut states = vec![false];
    states.extend([true; 10]);
    states.extend([false; 3]);
    states.extend([true; 2]);

    let v = AckVector::encode(7, &states, (5, 1));
    assert_eq!(v.coded[0], 0b0111_1110, "bitmap: missing then six received");
    assert_eq!(v.states(), states);

    let long = vec![true; 2000];
    let v = AckVector::encode(0, &long, (0, 0));
    assert_eq!(v.coded.len(), MAX_ACK_VECTOR_BYTES);
    assert_eq!(v.timestamp, None, "truncated vectors carry no timestamp");

    let h =
        RcHeader { log_window: 0xD, ack_vector: Some(AckVector::encode(1, &states, (77, 4))), ..Default::default() };
    let mut out = Vec::new();
    h.encode(&mut out);
    assert_eq!(RcHeader::decode(&out).unwrap().0, h);
}

#[test]
fn decode_errors() {
    assert!(RcHeader::decode(&[0x09, 0x00]).is_err(), "ACK and ACKVEC");
    assert!(RcHeader::decode(&[0x04, 0xD0, 0x01]).is_err(), "truncated DATA");

    // 0x200 without ACK/ACKVEC carries no extension word.
    let (h, len) = RcHeader::decode(&[0x04, 0xD2, 0x01, 0x00]).unwrap();
    assert_eq!((h.data_sn, h.extension, len), (Some(1), None, 4));
}

#[test]
fn sn_reconstruction() {
    assert_eq!(reconstruct(5, 0x1_0000), 0x1_0005);
    assert_eq!(reconstruct(0xFFFF, 0x1_0002), 0xFFFF);
    assert_eq!(reconstruct(2, 0x1_FFFE), 0x2_0002);
    assert_eq!(reconstruct(100, 100), 100);
}
