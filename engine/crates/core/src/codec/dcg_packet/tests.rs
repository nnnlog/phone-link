use super::*;
use crate::codec::hub::to_bytes;

fn fragment() -> Fragment {
    Fragment {
        version: 1.0,
        session_id: "s".into(),
        sequence_number: 1,
        message_id: 7,
        message_type_raw: Some(0),
        fragment_id: 1,
        fragment_count: 1,
        raw: vec![9],
    }
}

#[test]
fn fragment_property_order_and_round_trip() {
    let p = fragment().to_packet();
    let keys: Vec<_> = p.properties.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        ["Version", "Type", "SessionId", "SequenceNumber", "MessageId", "MessageType", "FragmentId", "FragmentCount"]
    );

    // Version is a float64 (0xCB), integers use the smallest encoding.
    assert_eq!(to_bytes(&p.properties[0].1)[0], 0xCB);
    assert_eq!(to_bytes(&p.properties[3].1), vec![0x01]);
    assert_eq!(parse_packet(&p).unwrap(), ParsedPacket::Fragment(fragment()));
}

#[test]
fn ack_property_order_and_outcomes() {
    let a = Ack::success("s", 4);
    let p = a.to_packet();
    let keys: Vec<_> = p.properties.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["Version", "Type", "SessionId", "SequenceNumber", "Success", "ErrorNo"]);

    assert_eq!(p.raw, None);
    assert_eq!(parse_packet(&p).unwrap(), ParsedPacket::Ack(a.clone()));
    assert_eq!(a.outcome(), AckOutcome::Acknowledged);

    let too_new = Ack { version: 1.1, ..a.clone() };
    let synthesized =
        Ack { success: false, error_no: 0, error_message: Some(TOO_NEW_ERROR_MESSAGE.into()), ..too_new.clone() };
    assert_eq!(parse_packet(&too_new.to_packet()).unwrap(), ParsedPacket::Ack(synthesized.clone()));
    assert_eq!(synthesized.outcome(), AckOutcome::RemoteTransportTooNew);
    assert_eq!(too_new.outcome(), AckOutcome::RemoteTransportTooNew);

    assert_eq!(Ack { success: false, error_no: 1, ..a.clone() }.outcome(), AckOutcome::RemoteTransportTooOld);
    assert_eq!(
        Ack { success: false, error_no: 2, ..a.clone() }.outcome(),
        AckOutcome::RemoteTransportError { error_no: 2 }
    );
    assert_eq!(error_no_result(5), DcgMessageResult::UnspecifiedError);
}

#[test]
fn lenient_integers_and_strict_types() {
    let mut p = Ack::success("s", 4).to_packet();
    // A float64 SequenceNumber and ErrorNo, as some clients send them, are accepted.
    p.properties[3].1 = Value::F64(4.0);
    p.properties[5].1 = Value::F64(0.0);
    assert!(matches!(parse_packet(&p).unwrap(), ParsedPacket::Ack(a) if a.sequence_number == 4));

    // Success must be a bool.
    let mut bad = Ack::success("s", 4).to_packet();
    bad.properties[4].1 = Value::from(1);
    assert!(matches!(parse_packet(&bad), Err(PacketError::Malformed(_))));

    // Version must be a float64.
    let mut int_version = fragment().to_packet();
    int_version.properties[0].1 = Value::from(1);
    assert!(matches!(parse_packet(&int_version), Err(PacketError::Malformed(_))));
}

#[test]
fn receive_drop_table() {
    let mut other_handler = fragment().to_packet();
    other_handler.handler_type = Some("other".into());
    assert!(matches!(parse_packet(&other_handler), Err(PacketError::InvalidHandler(_))));

    let newer = Fragment { version: 1.5, ..fragment() }.to_packet();
    assert_eq!(parse_packet(&newer), Err(PacketError::UnsupportedVersion(1.5)));

    for t in [3, 4, 5] {
        let mut p = fragment().to_packet();
        p.properties[1].1 = Value::from(t);
        assert_eq!(parse_packet(&p).unwrap(), ParsedPacket::Presence(t));
    }

    for t in [0, 6, 99] {
        let mut p = fragment().to_packet();
        p.properties[1].1 = Value::from(t);
        assert!(matches!(parse_packet(&p), Err(PacketError::Malformed(_))));
    }

    let mut no_session = fragment().to_packet();
    no_session.properties.retain(|(k, _)| k != "SessionId");
    assert!(matches!(parse_packet(&no_session), Err(PacketError::Malformed(_))));

    let mut no_message_type = fragment().to_packet();
    no_message_type.properties.retain(|(k, _)| k != "MessageType");
    match parse_packet(&no_message_type).unwrap() {
        ParsedPacket::Fragment(f) => assert_eq!(f.message_type(), MessageType::App),
        other => panic!("{other:?}"),
    }
}

#[test]
fn version_must_be_float64_and_type_may_be_any_number() {
    // `Version` as float32 is not a float64: parse failure (dropped without reply).
    let mut f32_version = fragment().to_packet();
    f32_version.properties[0].1 = Value::F32(1.0);
    assert!(matches!(parse_packet(&f32_version), Err(PacketError::Malformed(_))));

    // `Type` sent as a float is accepted and converted to an int.
    let mut float_type = fragment().to_packet();
    float_type.properties[1].1 = Value::F64(2.0);
    assert!(matches!(parse_packet(&float_type), Ok(ParsedPacket::Fragment(_))));
    let mut float_ack = Ack::success("s", 4).to_packet();
    float_ack.properties[1].1 = Value::F32(1.0);
    assert!(matches!(parse_packet(&float_ack), Ok(ParsedPacket::Ack(_))));
}

#[test]
fn message_type_is_read_as_an_int32_property() {
    let with = |v: Value| {
        let mut p = fragment().to_packet();
        p.properties.iter_mut().find(|(k, _)| k == "MessageType").unwrap().1 = v;
        parse_packet(&p).map(|parsed| match parsed {
            ParsedPacket::Fragment(f) => f.message_type(),
            other => panic!("a fragment: {other:?}"),
        })
    };

    for v in [Value::from(1), Value::F64(1.0), Value::F64(1.4), Value::from(" +1 "), Value::Boolean(true)] {
        assert_eq!(with(v.clone()).unwrap(), MessageType::Platform, "{v:?}");
    }
    for v in [Value::Nil, Value::from(0), Value::from(2), Value::F64(1.5), Value::Boolean(false), Value::from("0")] {
        assert_eq!(with(v.clone()).unwrap(), MessageType::App, "{v:?}");
    }
    for v in
        [Value::from(1u64 << 40), Value::Binary(vec![1]), Value::from("1.0"), Value::from("x"), Value::Array(vec![])]
    {
        assert!(with(v.clone()).is_err(), "{v:?} does not convert");
    }
}

#[test]
fn integer_properties_convert_by_value_type() {
    let with = |v: Value| {
        let mut p = fragment().to_packet();
        p.properties.iter_mut().find(|(k, _)| k == "SequenceNumber").unwrap().1 = v;
        parse_packet(&p).map(|parsed| match parsed {
            ParsedPacket::Fragment(f) => f.sequence_number,
            other => panic!("a fragment: {other:?}"),
        })
    };

    assert_eq!(with(Value::from(7)).unwrap(), 7);
    assert_eq!(with(Value::from("\t-12 ")).unwrap(), -12);
    assert_eq!(with(Value::Boolean(true)).unwrap(), 1);
    assert_eq!(with(Value::Nil).unwrap(), 0);
    assert_eq!(with(Value::F64(2.5)).unwrap(), 2);
    assert_eq!(with(Value::from("2147483647")).unwrap(), i32::MAX);
    assert_eq!(with(Value::from("-2147483648")).unwrap(), i32::MIN);
    assert!(with(Value::from("2147483648")).is_err());
    assert!(with(Value::from("")).is_err());
    assert!(with(Value::from("1e3")).is_err());
    assert!(with(Value::Binary(vec![7])).is_err());
}

/// A too-new ack is read for its `SessionId` and `SequenceNumber` only; `ErrorMessage` nil is a
/// parse failure, any other non-string value is read in its string form.
#[test]
fn too_new_acks_and_error_messages() {
    let mut too_new = Ack::success("s", 4).to_packet();
    too_new.properties[0].1 = Value::F64(2.0);
    too_new.properties.retain(|(k, _)| k != "Success" && k != "ErrorNo");
    match parse_packet(&too_new).unwrap() {
        ParsedPacket::Ack(a) => {
            assert_eq!((a.session_id.as_str(), a.sequence_number), ("s", 4));
            assert_eq!(a.outcome(), AckOutcome::RemoteTransportTooNew);
        }
        other => panic!("{other:?}"),
    }

    // Even a too-new ack needs those two.
    let mut no_sequence = too_new.clone();
    no_sequence.properties.retain(|(k, _)| k != "SequenceNumber");
    assert!(matches!(parse_packet(&no_sequence), Err(PacketError::Malformed(_))));

    let with_message = |v: Value| {
        let mut p = Ack { success: false, error_no: 7, ..Ack::success("s", 4) }.to_packet();
        p.properties.push(("ErrorMessage".to_string(), v));
        parse_packet(&p)
    };
    assert!(matches!(with_message(Value::Nil), Err(PacketError::Malformed(_))));

    let read = |v: Value| match with_message(v) {
        Ok(ParsedPacket::Ack(a)) => a.error_message,
        other => panic!("{other:?}"),
    };
    assert_eq!(read(Value::from("boom")), Some("boom".to_string()));
    assert_eq!(read(Value::from(5)), Some("5".to_string()));
    assert_eq!(read(Value::Boolean(true)), Some("true".to_string()));
}

/// A fragment whose `Raw` is nil is accepted (empty payload); the receiver fails its message.
#[test]
fn a_nil_raw_fragment_parses() {
    let mut p = fragment().to_packet();
    p.raw = None;
    assert_eq!(parse_packet(&p).unwrap(), ParsedPacket::Fragment(Fragment { raw: Vec::new(), ..fragment() }));
}

#[test]
fn fragment_split() {
    let data: Vec<u8> = (0..10).collect();
    let parts = split_into_fragments(&data, 4);
    assert_eq!(parts, vec![&data[0..4], &data[4..8], &data[8..10]]);
    assert_eq!(split_into_fragments(&data, 536_870_912).len(), 1);
}
