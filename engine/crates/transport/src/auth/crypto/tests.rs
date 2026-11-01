use super::*;

#[test]
fn jws_round_trip_and_raw_signature() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_454_400);
    let pair = KeyPair::generate("0f2a63f2-7a43-4a65-9d4c-8f7b6d2e1a10", now).unwrap();

    let jwt = nonce_jwt(&pair, "n-1", now).unwrap();
    assert!(!jwt.contains('='));

    let decoded = decode_jws(&jwt).unwrap();
    assert_eq!(decoded.header_text, JWS_HEADER);
    assert_eq!(decoded.signature().len(), 96);
    decoded.verify(pair.key.verifying_key()).unwrap();

    let other = generate_signing_key();
    assert!(matches!(decoded.verify(other.verifying_key()), Err(CryptoError::BadSignature)));
}

#[test]
fn b64_std_decode_ignores_line_wrapping() {
    let pair = KeyPair::generate("trust_0f2a63f2-7a43-4a65-9d4c-8f7b6d2e1a10", SystemTime::now()).unwrap();
    let flat = pair.cert.base64();
    let wrapped: String =
        flat.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\n") + "\n";

    assert_eq!(b64_std_decode(&wrapped).unwrap(), pair.cert.der);
    assert_eq!(b64_std_decode(&wrapped.replace('\n', "\r\n")).unwrap(), pair.cert.der);
    assert!(b64_std_decode(flat.trim_end_matches('=')).is_err() || !flat.ends_with('='), "padding stays required");
    assert!(b64_std_decode("ab-_").is_err());
}

#[test]
fn rejects_unsupported_cn() {
    let key = generate_signing_key();
    assert!(self_signed_certificate(&key, "a,b", SystemTime::now()).is_err());
    assert!(self_signed_certificate(&key, "", SystemTime::now()).is_err());
}
