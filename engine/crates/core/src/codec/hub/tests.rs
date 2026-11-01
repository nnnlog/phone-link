use super::*;

#[test]
fn handshake_bytes_match_the_spec() {
    let expected: Vec<u8> = vec![
        0x7B, 0x22, 0x70, 0x72, 0x6F, 0x74, 0x6F, 0x63, 0x6F, 0x6C, 0x22, 0x3A, 0x22, 0x6D, 0x65, 0x73, 0x73, 0x61,
        0x67, 0x65, 0x70, 0x61, 0x63, 0x6B, 0x22, 0x2C, 0x22, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6F, 0x6E, 0x22, 0x3A,
        0x32, 0x7D, 0x1E,
    ];
    assert_eq!(HANDSHAKE_REQUEST, expected.as_slice());
}

#[test]
fn handshake_response_with_coalesced_frame() {
    let mut buf = b"{}\x1e".to_vec();
    let ping = encode_frame(&HubMessage::Ping);
    buf.extend_from_slice(&ping);

    let (resp, used) = parse_handshake_response(&buf).unwrap().unwrap();
    assert_eq!(resp.error, None);
    assert_eq!(&buf[used..], ping.as_slice());

    let (resp, _) = parse_handshake_response(b"{\"error\":\"nope\"}\x1e").unwrap().unwrap();
    assert_eq!(resp.error.as_deref(), Some("nope"));

    assert_eq!(parse_handshake_response(b"{}").unwrap(), None);
}

#[test]
fn varint_prefix_rules() {
    let mut out = Vec::new();
    write_frame(&[0u8; 300], &mut out);
    assert_eq!(&out[..2], &[0xAC, 0x02]);

    let (payload, used) = read_frame(&out).unwrap().unwrap();
    assert_eq!(payload.len(), 300);
    assert_eq!(used, 302);

    assert_eq!(read_frame(&[0x80]).unwrap(), None);
    assert_eq!(read_frame(&[0x05, 1, 2]).unwrap(), None);

    assert_eq!(read_frame(&[0xFF, 0xFF, 0xFF, 0xFF, 0x08]), Err(HubCodecError::BadLengthPrefix));
    assert_eq!(read_frame(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]), Err(HubCodecError::BadLengthPrefix));

    // 5th byte of 7 is allowed (prefix decodes; payload incomplete).
    assert_eq!(read_frame(&[0x80, 0x80, 0x80, 0x80, 0x07]).unwrap(), None);
}

#[test]
fn ping_and_invocation_bytes() {
    assert_eq!(encode_message(&HubMessage::Ping), vec![0x91, 0x06]);
    let inv = HubMessage::invocation("1", "T", vec![]);
    // [1, {}, "1", "T", [], []]
    assert_eq!(encode_message(&inv), vec![0x96, 0x01, 0x80, 0xA1, b'1', 0xA1, b'T', 0x90, 0x90]);
    assert_eq!(decode_message(&encode_message(&inv)).unwrap(), inv);
}

#[test]
fn completion_kinds_and_unknown_type() {
    for result in
        [CompletionResult::Void, CompletionResult::Error("boom".into()), CompletionResult::Value(Value::from(5))]
    {
        let m = HubMessage::Completion { headers: vec![], invocation_id: "7".into(), result };
        assert_eq!(decode_message(&encode_message(&m)).unwrap(), m);
    }

    let bad = to_bytes(&Value::Array(vec![Value::from(3), Value::Map(vec![]), Value::from("1"), Value::from(9)]));
    assert_eq!(decode_message(&bad), Err(HubCodecError::UnknownCompletionKind(9)));

    let unknown = to_bytes(&Value::Array(vec![Value::from(42), Value::from("x")]));
    assert_eq!(decode_message(&unknown).unwrap(), HubMessage::Unknown { message_type: 42 });
}

#[test]
fn close_reads_optional_allow_reconnect() {
    let two = to_bytes(&Value::Array(vec![Value::from(7), Value::Nil]));
    assert_eq!(decode_message(&two).unwrap(), HubMessage::Close { error: None, allow_reconnect: None });
    let three = to_bytes(&Value::Array(vec![Value::from(7), Value::from("e"), Value::Boolean(true)]));
    assert_eq!(
        decode_message(&three).unwrap(),
        HubMessage::Close { error: Some("e".into()), allow_reconnect: Some(true) }
    );
}

#[test]
fn send_connected_matches_the_spec_shape() {
    let ctx = TraceContext {
        trace_id: "0123456789abcdef0123456789abcdef".into(),
        parent_id: None,
        correlation_id: "0011223344556677".into(),
        flags: 0,
        trace_state: send_connected_trace_state(),
    };

    let args = send_connected_arguments(&ctx, "peer");
    assert_eq!(to_bytes(&args[2]), vec![0x80]);

    let packet = TraceContextPacket::from_value(&args[0]).unwrap().unwrap();
    assert_eq!(packet.parent_id.as_deref(), Some("0011223344556677"));
    assert_eq!(packet.trace_flags, 0);

    let keys: Vec<_> = match &args[0] {
        Value::Map(e) => e.iter().map(|(k, _)| k.as_str().unwrap().to_string()).collect(),
        _ => unreachable!(),
    };
    assert_eq!(keys, ["TraceId", "ParentId", "TraceFlags", "TraceState"]);
}

#[test]
fn inbound_trace_context_rules() {
    let p = TraceContextPacket {
        trace_id: Some("0123456789abcdef0123456789abcdef".into()),
        parent_id: Some("aa".into()),
        trace_flags: 1,
        trace_state: Some(vec![("ypScenarioId".into(), "S".into())]),
    };

    let ctx = p.to_inbound_context().unwrap();
    assert_eq!(ctx.trace_id, "0123456789abcdef0123456789abcdef");
    assert_eq!(ctx.parent_id.as_deref(), Some("aa"));
    assert_eq!(ctx.flags, 1);
    assert_eq!(ctx.trace_state, vec![("ypScenarioId".to_string(), "S".to_string())]);
    assert_eq!(ctx.correlation_id.len(), 16);
    assert_ne!(ctx.correlation_id, p.to_inbound_context().unwrap().correlation_id, "a new correlation id");

    // Upper case, any letter or digit, a nil ParentId and an empty TraceState pass.
    let upper = TraceContextPacket {
        trace_id: Some("ZYX".repeat(10) + "AB"),
        parent_id: None,
        trace_state: Some(vec![]),
        ..p.clone()
    };
    assert!(upper.to_inbound_context().is_ok());

    let rejected = [
        TraceContextPacket { trace_id: None, ..p.clone() },
        TraceContextPacket { trace_id: Some("0".repeat(32)), ..p.clone() },
        TraceContextPacket { trace_id: Some("0123456789abcdef".into()), ..p.clone() },
        TraceContextPacket { trace_id: Some("0123456789abcdef0123456789abcde-".into()), ..p.clone() },
        TraceContextPacket { trace_id: Some("0123456789abcdef0123456789abcdef0".into()), ..p.clone() },
        TraceContextPacket { trace_state: None, ..p.clone() },
    ];
    for packet in rejected {
        assert!(matches!(packet.to_inbound_context(), Err(HubCodecError::RejectedTraceContext(_))), "{packet:?}");
    }
}

#[test]
fn multiplex_packet_reads_keys_in_any_order() {
    let v = Value::Map(vec![
        (Value::from("Raw"), Value::Binary(vec![1, 2])),
        (Value::from("Extra"), Value::from(1)),
        (Value::from("Type"), Value::from("ms-dcg")),
        (Value::from("Properties"), Value::Map(vec![(Value::from("Version"), Value::F64(1.0))])),
    ]);

    let p = MultiplexPacket::from_value(&v).unwrap();
    assert_eq!(p.handler_type.as_deref(), Some("ms-dcg"));
    assert_eq!(p.raw.as_deref(), Some(&[1u8, 2][..]));

    let back = p.to_value();
    let keys: Vec<_> = match &back {
        Value::Map(e) => e.iter().map(|(k, _)| k.as_str().unwrap().to_string()).collect(),
        _ => unreachable!(),
    };
    assert_eq!(keys, ["Properties", "Type", "Raw"]);
}

#[test]
fn on_connected_payload() {
    let v = Value::Map(vec![
        (Value::from("RegionName"), Value::from("westus")),
        (Value::from("Partners"), Value::Array(vec![Value::from("a")])),
    ]);
    let p = OnConnectedPayload::from_value(&v).unwrap();
    assert_eq!(p.region_name.as_deref(), Some("westus"));
    assert_eq!(p.partners.unwrap(), vec!["a".to_string()]);
}
