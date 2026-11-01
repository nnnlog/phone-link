use super::*;

// RFC 5769 §2.1 sample request.
const SAMPLE_REQUEST: &str = concat!(
    "000100582112a442b7e7a701bc34d686fa87dfae",
    "802200105354554e20746573742063",
    "6c69656e74",
    "002400046e0001ff",
    "80290008932ff9b151263b36",
    "000600096576746a3a68367659202020",
    "000800149aeaa70cbfd8cb56781ef2b5b2d3f249c1b571a2",
    "80280004e57a3bcf",
);

#[test]
fn rfc5769_request_verifies() {
    let raw = hex::decode(SAMPLE_REQUEST).unwrap();
    let d = Message::decode(&raw).unwrap();
    assert_eq!(d.message.class, Class::Request);
    assert_eq!(d.message.method, method::BINDING);
    assert_eq!(d.message.text(attr::USERNAME).unwrap(), "evtj:h6vY");
    assert_eq!(d.message.u32_attribute(attr::PRIORITY), Some(0x6e0001ff));
    d.verify_integrity(b"VOkJxbRl1RmTxUk/WvJxBt").unwrap();
    assert_eq!(d.verify_integrity(b"wrong"), Err(StunError::Integrity));

    let mut bad = raw.clone();
    *bad.last_mut().unwrap() ^= 1;
    assert_eq!(Message::decode(&bad), Err(StunError::Fingerprint));
}

#[test]
fn round_trip_with_integrity() {
    let tx = [7u8; 12];
    let peer: SocketAddr = "[2001:db8::5]:4321".parse().unwrap();
    let m = Message::new(Class::Success, method::BINDING, tx)
        .with(attr::XOR_MAPPED_ADDRESS, encode_xor_address(peer, &tx))
        .with(attr::SOFTWARE, SOFTWARE);

    let bytes = m.encode(Some(b"key"), false);
    assert!(is_stun(&bytes));

    let d = Message::decode(&bytes).unwrap();
    d.verify_integrity(b"key").unwrap();
    assert_eq!(d.message.mapped_address(), Some(peer));
    assert_eq!(d.message.attributes, m.attributes);

    let with_fp = m.encode(None, true);
    assert!(Message::decode(&with_fp).is_ok());
}

#[test]
fn method_and_class_bits() {
    assert_eq!(message_type(Class::Success, method::BINDING), 0x0101);
    assert_eq!(message_type(Class::Error, method::ALLOCATE), 0x0113);
    assert_eq!(message_type(Class::Indication, method::DATA), 0x0017);
    assert_eq!(method_of(0x0113), method::ALLOCATE);
}

#[test]
fn channel_data_pads() {
    let cd = channel_data(0x4000, b"abcde");
    assert_eq!(cd.len(), 12);
    assert_eq!(parse_channel_data(&cd), Some((0x4000, &b"abcde"[..])));
    assert!(parse_channel_data(&[0x80, 0, 0, 0]).is_none());
}
