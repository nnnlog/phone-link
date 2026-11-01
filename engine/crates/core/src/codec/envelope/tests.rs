use super::*;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    HeaderMap::from_pairs(pairs.iter().copied()).unwrap()
}

#[test]
fn writer_includes_the_count_in_the_blob_length() {
    let bytes = encode(&headers(&[("a", "b")]), &[]).unwrap();
    // 4 + (4+1+4+1) = 14
    assert_eq!(&bytes[1..5], &14i32.to_le_bytes());
    assert_eq!(encode(&HeaderMap::new(), &[]), Err(EnvelopeError::NoHeaders));
}

#[test]
fn reader_accepts_both_blob_conventions() {
    let h = headers(&[("_route", "/x"), ("_requestId", "1")]);
    let mut bytes = encode(&h, b"pp").unwrap();
    assert_eq!(decode(&bytes).unwrap(), Envelope { headers: h.clone(), payload: b"pp".to_vec() });

    // Exclude-the-count form (4 less) parses identically.
    let blob = i32::from_le_bytes(bytes[1..5].try_into().unwrap()) - 4;
    bytes[1..5].copy_from_slice(&blob.to_le_bytes());
    assert_eq!(decode(&bytes).unwrap().headers, h);
}

#[test]
fn reader_accepts_slack_in_the_blob_length() {
    let h = headers(&[("_route", "/x")]);
    let mut bytes = encode(&h, b"payload").unwrap();

    // A larger headersBlobLength parses identically as long as that many bytes are present.
    let blob = i32::from_le_bytes(bytes[1..5].try_into().unwrap()) + 5;
    bytes[1..5].copy_from_slice(&blob.to_le_bytes());

    let env = decode(&bytes).unwrap();
    assert_eq!(env.headers, h);
    assert_eq!(env.payload, b"payload");
}

#[test]
fn malformed_cases() {
    let good = encode(&headers(&[("k", "v")]), b"x").unwrap();

    let mut zero_blob = good.clone();
    zero_blob[1..5].copy_from_slice(&0i32.to_le_bytes());
    assert_eq!(decode(&zero_blob), Err(EnvelopeError::NonPositiveHeadersBlobLength(0)));

    let mut huge_blob = good.clone();
    huge_blob[1..5].copy_from_slice(&1000i32.to_le_bytes());
    assert_eq!(decode(&huge_blob), Err(EnvelopeError::HeadersBlobBeyondMessage(1000)));

    let mut zero_count = good.clone();
    zero_count[5..9].copy_from_slice(&0i32.to_le_bytes());
    assert_eq!(decode(&zero_count), Err(EnvelopeError::NonPositiveHeaderCount(0)));

    let truncated = &good[..good.len() - 1];
    assert_eq!(decode(truncated), Err(EnvelopeError::Truncated("payload")));

    let mut negative = good.clone();
    let at = negative.len() - 1 - 8;
    negative[at..at + 8].copy_from_slice(&(-1i64).to_le_bytes());
    assert_eq!(decode(&negative), Err(EnvelopeError::NegativePayloadLength(-1)));

    // Duplicate key, hand-built.
    let mut dup = vec![1u8];
    dup.extend_from_slice(&(4 + 2 * 10i32).to_le_bytes());
    dup.extend_from_slice(&2i32.to_le_bytes());
    for _ in 0..2 {
        dup.extend_from_slice(&1i32.to_le_bytes());
        dup.push(b'k');
        dup.extend_from_slice(&1i32.to_le_bytes());
        dup.push(b'v');
    }
    dup.extend_from_slice(&0i64.to_le_bytes());
    assert_eq!(decode(&dup), Err(EnvelopeError::DuplicateHeader("k".into())));
}

#[test]
fn version_byte_is_ignored_and_trailing_bytes_tolerated() {
    let mut bytes = encode(&headers(&[("k", "v")]), b"x").unwrap();
    bytes[0] = 0x7F;
    bytes.push(0xEE);
    assert_eq!(decode(&bytes).unwrap().payload, b"x");
}
