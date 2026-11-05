use super::*;

#[test]
fn reason_mapping() {
    let encode = |reason: i32| PlatformDisconnectRequest { trace_context: None, reason }.encode_to_vec();

    assert_eq!(disconnect_reason(&encode(PlatformDisconnectRequestReason::UserRequested as i32)), 1);
    assert_eq!(disconnect_reason(&encode(PlatformDisconnectRequestReason::TwoPhoneModeEnabled as i32)), 6);
    assert_eq!(disconnect_reason(&encode(PlatformDisconnectRequestReason::PcLock as i32)), 7);
    assert_eq!(disconnect_reason(&encode(8)), 0);
    assert_eq!(disconnect_reason(&encode(17)), 0);
    assert_eq!(disconnect_reason(b"{}"), 0, "a key-value body fails the protobuf parser");
    assert_eq!(disconnect_reason(&[]), 0);

    let persistent: Vec<i32> = (0..=8).filter(|r| is_persistent_disconnect_reason(*r)).collect();
    assert_eq!(persistent, [0, 1, 2, 3, 4, 7]);
}

#[test]
fn the_outbound_request_is_a_protobuf_with_the_stops_trace_and_no_reason() {
    let trace = TraceContext::new_root(vec![("ypScenarioId".into(), "StopSession".into())]);
    let message = disconnect_request_message(&trace);

    assert_eq!(message.message_type, MessageType::Platform);
    assert_eq!(message.route(), Some(route::PLATFORM_DISCONNECT_REQUEST));
    assert_eq!(message.headers.get(header::MS_CONTENT_TYPE), Some(header::CONTENT_TYPE_BINARY));

    let body = PlatformDisconnectRequest::decode(message.payload.as_slice()).expect("protobuf");
    assert_eq!(body.reason, PlatformDisconnectRequestReason::Unspecified as i32);
    assert_eq!(body.trace_context, Some(proto_trace(&trace)));
}
