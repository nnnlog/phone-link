use super::*;

#[test]
fn writer_is_flat_ordered_and_empty_is_two_bytes() {
    assert_eq!(Kvp::new().to_body(), b"{}");

    let kvp = Kvp::new().with("CryptoKeyRotationResult", "0").with("TimeToExpireInSeconds", "17");
    assert_eq!(kvp.to_body(), br#"{"CryptoKeyRotationResult":"0","TimeToExpireInSeconds":"17"}"#);

    let escaped = Kvp::new().with("k", "a\"b\\c");
    assert_eq!(escaped.to_body(), br#"{"k":"a\"b\\c"}"#);

    let replaced = Kvp::new().with("a", "1").with("b", "2").with("a", "3");
    assert_eq!(replaced.iter().collect::<Vec<_>>(), vec![("a", "3"), ("b", "2")]);
}

#[test]
fn reader_rules() {
    // Present and not text/json ⇒ empty map, whatever the body.
    assert_eq!(Kvp::read(Some("application/x-binary"), br#"{"a":"b"}"#).unwrap(), Kvp::new());
    assert_eq!(Kvp::read(Some("TEXT/JSON"), br#"{"a":"b"}"#).unwrap(), Kvp::new());

    // Empty body ⇒ empty map.
    assert_eq!(Kvp::read(Some("text/json"), b"").unwrap(), Kvp::new());
    assert_eq!(Kvp::read(None, b"").unwrap(), Kvp::new());

    // Absent content type ⇒ parsed as JSON.
    assert_eq!(Kvp::read(None, br#"{"a":"b"}"#).unwrap().get("a"), Some("b"));
    assert_eq!(Kvp::read(None, b" { } ").unwrap(), Kvp::new());

    // Not JSON, not an object, a repeated key.
    assert!(matches!(Kvp::read(None, b"{"), Err(KvpError::NotJson(_))));
    assert!(matches!(Kvp::read(None, b"{\"a\":1,}"), Err(KvpError::NotJson(_))));
    assert!(matches!(Kvp::read(None, b"\xEF\xBB\xBF{}"), Err(KvpError::NotJson(_))), "a BOM is not JSON");
    assert_eq!(Kvp::read(None, b"[1]"), Err(KvpError::NotAnObject));
    assert_eq!(Kvp::read(None, b"\"s\""), Err(KvpError::NotAnObject));
    assert_eq!(Kvp::read(None, br#"{"a":"1","a":"2"}"#), Err(KvpError::DuplicateKey("a".into())));
}

#[test]
fn values_read_as_json_element_to_string() {
    let body = br#"{"s":"x\u00e9\n","n":12,"neg":-3,"f":1.50,"e":1e400,"t":true,"F":false,"z":null,"o":{ "a" : [1, "}"] },"arr":[ ],"k\"q":"v"}"#;
    let kvp = Kvp::read(Some("text/json"), body).unwrap();

    assert_eq!(
        kvp.iter().collect::<Vec<_>>(),
        vec![
            ("s", "x\u{e9}\n"),
            ("n", "12"),
            ("neg", "-3"),
            ("f", "1.50"),
            ("e", "1e400"),
            ("t", "True"),
            ("F", "False"),
            ("z", ""),
            ("o", r#"{ "a" : [1, "}"] }"#),
            ("arr", "[ ]"),
            ("k\"q", "v"),
        ]
    );

    // The certificate example of the key-rotation route: an extra boolean does not spoil the map.
    let rotation = Kvp::read(None, br#"{"Certificate":"AAEC","x":true}"#).unwrap();
    assert_eq!(rotation.get("Certificate"), Some("AAEC"));
    assert_eq!(rotation.get("x"), Some("True"));
}

#[test]
fn nesting_is_limited_to_64_levels() {
    let nested = |levels: usize| format!("{{\"a\":{}{}}}", "[".repeat(levels - 1), "]".repeat(levels - 1));
    assert!(Kvp::read(None, nested(64).as_bytes()).is_ok());
    assert!(matches!(Kvp::read(None, nested(65).as_bytes()), Err(KvpError::NotJson(_))));
}

#[test]
fn platform_message_headers() {
    let m = Kvp::new().platform_message(Some("/X"));
    assert_eq!(m.message_type, MessageType::Platform);
    assert_eq!(m.headers.iter().collect::<Vec<_>>(), vec![("_route", "/X"), ("ms-content-type", "text/json")]);
    assert_eq!(m.payload, b"{}");

    let r = Kvp::new().platform_message(None);
    assert_eq!(r.headers.iter().collect::<Vec<_>>(), vec![("ms-content-type", "text/json")]);
}
