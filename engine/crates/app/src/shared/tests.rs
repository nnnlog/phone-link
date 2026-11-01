use super::*;

#[test]
fn ms_phone_uri_rule() {
    assert!(is_ms_phone_uri("ms-phone:notifications"));
    assert!(is_ms_phone_uri("MS-PHONE://x?y=1"));
    assert!(is_ms_phone_uri("ms-phone:"));
    assert!(!is_ms_phone_uri("ms-phone"));
    assert!(!is_ms_phone_uri("/relative"));
    assert!(!is_ms_phone_uri("https://example.com"));
    assert!(!is_ms_phone_uri("ms-phone:a b"));
    assert!(!is_ms_phone_uri(" ms-phone:x"));
    assert!(!is_ms_phone_uri("ms-phones:x"));
}
