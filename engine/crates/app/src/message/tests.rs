use super::*;
use flate2::{Compression, write::GzEncoder};
use phonelink_core::types::MessageType;
use std::io::Write;

fn msg(headers: &[(&str, &str)], vs: &ValueSet, gzip: bool) -> TransportMessage {
    let mut payload = valueset::encode(vs);
    if gzip {
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&payload).unwrap();
        payload = enc.finish().unwrap();
    }

    TransportMessage::new(MessageType::App, HeaderMap::from_pairs(headers.iter().copied()).unwrap(), payload)
}

#[test]
fn header_wins_and_payload_fills_the_gaps() {
    let vs = ValueSet::new()
        .with("_route", "/legacy/phonecontent")
        .with("_requestId", "r-1")
        .with("contentType", "heartbeat")
        .with("correlationVector", "cv")
        .with("result", 7i32)
        .with("contractVersion", 3.99f64);

    let m = read_app_message(&msg(&[("contentType", "permissions")], &vs, false)).unwrap();

    assert_eq!(m.content_type(), Some("permissions"));
    assert_eq!(m.route(), Some("/legacy/phonecontent"));
    assert_eq!(m.request_id(), Some("r-1"));
    assert_eq!(m.correlation_vector(), Some("cv"));
    assert_eq!(m.result(), 7);
    assert_eq!(m.contract_version(), ContractVersion::Number(3.99));
}

#[test]
fn mistyped_payload_keys_are_not_lifted() {
    let vs = ValueSet::new().with("_route", 5i32).with("result", "0").with("contractVersion", "3.99");
    let m = read_app_message(&msg(&[], &vs, false)).unwrap();

    assert_eq!(m.route(), None);
    assert_eq!(m.result(), -1);
    assert_eq!(m.contract_version(), ContractVersion::Missing);
}

#[test]
fn gzip_flag_is_case_insensitive_and_non_numeric_headers_are_flagged() {
    let vs = ValueSet::new().with("a", "b");
    let m = read_app_message(&msg(
        &[("_isGzippedCompressed", "TRUE"), ("contractVersion", "x3"), ("result", "zero")],
        &vs,
        true,
    ))
    .unwrap();

    assert_eq!(m.values.str("a"), Some("b"));
    assert_eq!(m.contract_version(), ContractVersion::NotANumber);
    assert_eq!(m.contract_version().value_or_zero(), 0.0);
    assert_eq!(m.result(), -1);

    // Not flagged: the gzip bytes are parsed as protobuf and fail.
    assert!(read_app_message(&msg(&[], &vs, true)).is_err());
}
