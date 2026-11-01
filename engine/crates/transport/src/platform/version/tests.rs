use super::*;

#[test]
fn text_int_parse() {
    assert_eq!(parse_text_int("1"), Some(1));
    assert_eq!(parse_text_int(" \t2\r\n"), Some(2));
    assert_eq!(parse_text_int("+3"), Some(3));
    assert_eq!(parse_text_int("-4"), Some(-4));
    assert_eq!(parse_text_int("007"), Some(7));

    assert_eq!(parse_text_int(""), None);
    assert_eq!(parse_text_int(" "), None);
    assert_eq!(parse_text_int("1.0"), None);
    assert_eq!(parse_text_int("abc"), None);
    assert_eq!(parse_text_int("2147483648"), None);
    assert_eq!(parse_text_int("+"), None);
}

#[test]
fn version_parse_and_order() {
    assert_eq!(ClientVersion::parse("1.23112.189.0"), Some(ClientVersion::new(1, 23112, 189, 0)));
    assert_eq!(ClientVersion::parse("1.2"), Some(ClientVersion::new(1, 2, -1, -1)));
    assert_eq!(ClientVersion::parse(" 1. 2 "), Some(ClientVersion::new(1, 2, -1, -1)));

    assert_eq!(ClientVersion::parse("1"), None);
    assert_eq!(ClientVersion::parse("1.2.3.4.5"), None);
    assert_eq!(ClientVersion::parse("1..2"), None);
    assert_eq!(ClientVersion::parse("1.-2"), None);
    assert_eq!(ClientVersion::parse(""), None);

    assert!(ClientVersion::new(1, 23112, 189, -1) < ClientVersion::new(1, 23112, 189, 0));
    assert!(ClientVersion::new(1, 23112, 190, -1) > ClientVersion::new(1, 23112, 189, 0));

    assert_eq!(ClientVersion::new(1, 23112, 189, 0).to_string(), "1.23112.189.0");
    assert_eq!(ClientVersion::new(2, 0, -1, -1).to_string(), "2.0");
}

#[test]
fn client_version_check() {
    let min = ClientVersion::parse("1.23112.189.0").unwrap();
    let passes = |v: &str| client_version_passes(Some(v), min);

    assert!(passes("1.23112.189.0"));
    assert!(passes("1.23112.189.1"));
    assert!(passes("1.23112.190"));
    assert!(passes("1.24000.0.0"));
    assert!(passes("2.0"));
    assert!(passes("1.23112.189.0-beta"));
    assert!(passes("1.0.0.0"));
    assert!(passes("1.0.0"));
    assert!(passes("1.0.0-dev"));

    assert!(!passes("1.23112.189"), "a missing component is lower than 0");
    assert!(!passes("1.23112.189-beta"));
    assert!(!passes("1.23111.999.999"));
    assert!(!passes("1.0"));
    assert!(!passes("1.0.0.1"));
    assert!(!passes("1"));
    assert!(!passes(""));
    assert!(!passes("-1.23112.189.0"));
    assert!(!passes("garbage"));
    assert!(!client_version_passes(None, min));
}
