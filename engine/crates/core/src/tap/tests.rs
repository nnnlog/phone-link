use super::*;

#[test]
fn secrets_are_redacted() {
    assert_eq!(redact_secret_header("Authorization", "Bearer abc"), "<redacted 10 bytes>");
    assert_eq!(redact_secret_header("DCG-AppId", "x"), "x");
}
