use super::writer::encode_variant;
use super::*;

fn rt(v: Value) -> Value {
    let vs = ValueSet::new().with("k", v);
    decode(&encode(&vs)).unwrap().get("k").cloned().unwrap()
}

#[test]
fn spec_example_bytes() {
    let vs = ValueSet::new().with("a", "b");
    assert_eq!(encode(&vs), vec![0x0A, 0x0A, 0x0A, 0x01, 0x61, 0x12, 0x05, 0x08, 0x0C, 0x6A, 0x01, 0x62]);
}

#[test]
fn every_type_round_trips() {
    let g = Uuid::parse_str("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap();
    let nested = ValueSet::new().with("x", 1i32);
    let all = vec![
        Value::UInt8(200),
        Value::Int16(-2),
        Value::UInt16(65535),
        Value::Int32(-7),
        Value::UInt32(u32::MAX),
        Value::Int64(-1),
        Value::UInt64(u64::MAX),
        Value::Single(1.5),
        Value::Double(3.99),
        Value::Char16(0x263A),
        Value::Bool(true),
        Value::String("héllo".into()),
        Value::Guid(g),
        Value::Point(Point { x: 1.0, y: -2.0 }),
        Value::Size(Size { width: 3.0, height: 4.0 }),
        Value::Rect(Rect { x: 1.0, y: 2.0, width: 3.0, height: 4.0 }),
        Value::Object(nested.clone()),
        Value::DateTime(crate::time::filetime_from_unix_ms(1_700_000_000_000)),
        Value::TimeSpan(10_000_000),
        Value::UInt8Array(vec![1, 2, 3]),
        Value::Int16Array(vec![-1, 2]),
        Value::UInt16Array(vec![1, 65535]),
        Value::Int32Array(vec![-1, 5]),
        Value::UInt32Array(vec![0, u32::MAX]),
        Value::Int64Array(vec![-5, i64::MAX]),
        Value::UInt64Array(vec![1, u64::MAX]),
        Value::SingleArray(vec![0.5, -1.0]),
        Value::DoubleArray(vec![0.25, 1e300]),
        Value::Char16Array(vec![65, 66]),
        Value::BoolArray(vec![true, false, true]),
        Value::StringArray(vec!["a".into(), "".into()]),
        Value::GuidArray(vec![g, Uuid::nil()]),
        Value::PointArray(vec![Point { x: 1.0, y: 1.0 }, Point::default()]),
        Value::SizeArray(vec![Size::default()]),
        Value::RectArray(vec![Rect::default()]),
        Value::ObjectArray(vec![nested.clone(), ValueSet::new()]),
        Value::DateTimeArray(vec![1, 2]),
        Value::TimeSpanArray(vec![3]),
        Value::Empty,
    ];

    assert_eq!(all.len(), 39);
    for v in all {
        assert_eq!(rt(v.clone()), v, "{}", v.type_name());
    }
}

#[test]
fn defaults_are_omitted_and_read_back() {
    for v in [
        Value::Int32(0),
        Value::Bool(false),
        Value::String(String::new()),
        Value::Int32Array(vec![]),
        Value::UInt8Array(vec![]),
    ] {
        let body = encode_variant(&v);
        assert_eq!(body.len(), 2, "{} must write only value_type", v.type_name());
        assert_eq!(rt(v.clone()), v);
    }

    // Message-typed values are always written, even when empty.
    assert_eq!(encode_variant(&Value::Point(Point::default())), vec![0x08, 0x0E, 0x7A, 0x00]);
    assert_eq!(encode_variant(&Value::Object(ValueSet::new())), vec![0x08, 0x11, 0x92, 0x01, 0x00]);
}

#[test]
fn negative_int16_is_a_ten_byte_varint() {
    let body = encode_variant(&Value::Int16(-1));
    assert_eq!(body, vec![0x08, 0x02, 0x18, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
}

#[test]
fn unpacked_repeated_scalars_are_accepted() {
    // value_type Int32Array (21), int32_array field 24 written unpacked: tag 0xC0 0x01.
    let variant = [0x08, 0x15, 0xC0, 0x01, 0x05, 0xC0, 0x01, 0x07];
    let mut entry = vec![0x0A, 0x01, b'k', 0x12, variant.len() as u8];
    entry.extend_from_slice(&variant);
    let mut vs = vec![0x0A, entry.len() as u8];
    vs.extend_from_slice(&entry);
    assert_eq!(decode(&vs).unwrap().get("k"), Some(&Value::Int32Array(vec![5, 7])));
}

#[test]
fn unknown_or_invalid_value_type_fails() {
    let mk = |vt: u8| {
        let variant = [0x08, vt];
        let mut entry = vec![0x0A, 0x01, b'k', 0x12, 2];
        entry.extend_from_slice(&variant);
        let mut vs = vec![0x0A, entry.len() as u8];
        vs.extend_from_slice(&entry);
        vs
    };

    assert_eq!(decode(&mk(40)), Err(ValueSetError::UnknownValueType(40)));

    // value_type 0 is not written at all, so the variant body is empty → Invalid.
    let entry = [0x0A, 0x01, b'k', 0x12, 0x00];
    let mut vs = vec![0x0A, entry.len() as u8];
    vs.extend_from_slice(&entry);
    assert_eq!(decode(&vs), Err(ValueSetError::InvalidValueType));

    assert_eq!(decode(&[0x0A, 0x05, 0x0A]), Err(ValueSetError::Truncated));
}

#[test]
fn order_is_kept_and_last_duplicate_wins() {
    let vs = ValueSet::new().with("z", 1i32).with("a", 2i32).with("m", 3i32);
    assert_eq!(decode(&encode(&vs)).unwrap().keys().collect::<Vec<_>>(), ["z", "a", "m"]);
    let mut bytes = encode(&ValueSet::new().with("k", 1i32));
    bytes.extend(encode(&ValueSet::new().with("k", 2i32)));
    assert_eq!(decode(&bytes).unwrap().i32("k"), Some(2));
}

#[test]
fn gzip_round_trip() {
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;
    let payload = encode(&ValueSet::new().with("a", "b"));
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&payload).unwrap();
    assert_eq!(gunzip(&enc.finish().unwrap()).unwrap(), payload);
}

#[test]
fn typed_json_view() {
    let vs = ValueSet::new().with("a", "b").with("n", ValueSet::new().with("x", 1i32));
    assert_eq!(vs.to_json(), json!({"a": {"String": "b"}, "n": {"Object": {"x": {"Int32": 1}}}}));
}
