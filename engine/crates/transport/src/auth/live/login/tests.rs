use super::*;

#[test]
fn parses_top_level() {
    let json = r#"{"sDAToken":"t","sDAStartTime":"2026-01-01T00:00:00Z","sDAExpires":"2026-01-01T01:00:00Z","sSTSInlineFlowToken":"ft","sSigninName":"me@example.com","K":"0003BFFD1234"}"#;
    let p = parse_da_property(json).unwrap();

    assert_eq!(p.da_token, "t");
    assert_eq!(p.puid, "0003BFFD1234");
    assert_eq!(p.sign_in_name, "me@example.com");
    assert_eq!(p.inline_flow_token, "ft");
}

#[test]
fn parses_nested_server_data() {
    let json = r#"{"ServerData":{"DAProperty":{"sDAToken":"t2","K":"PUID2","sSigninName":"a@b.c"}}}"#;
    let p = parse_da_property(json).unwrap();

    assert_eq!(p.da_token, "t2");
    assert_eq!(p.puid, "PUID2");
}

#[test]
fn missing_is_an_error() {
    assert!(matches!(parse_da_property(r#"{"foo":1}"#).unwrap_err(), LoginError::NoDaProperty));
    assert!(matches!(parse_da_property("not json").unwrap_err(), LoginError::Json(_)));
}
