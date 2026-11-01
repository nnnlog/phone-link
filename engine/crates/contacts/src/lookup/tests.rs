use super::*;

#[test]
fn email_test_matches_the_address_pattern() {
    for yes in [
        "someone@example.com",
        "a.b+c,d-e@mail.example.co.kr",
        "MAILTO:x@y.io",
        "mailto:.dot@y.io",
        "홍길동@example.com",
        "x@a.b",
        "x@a..",
        "x@a.b-",
    ] {
        assert!(is_email(yes), "{yes}");
    }

    for no in [
        "",
        "+1 650 555 0100",
        "someone@",
        "@example.com",
        ".dot@example.com",
        "x@example",
        "x@.com",
        "x@ab.",
        "x@a_b.com",
        "x y@example.com",
        "x@y@example.com",
        "mailto:",
    ] {
        assert!(!is_email(no), "{no}");
    }
}

#[test]
fn normalize_keeps_unparsable_input() {
    assert_eq!(normalize("010-1234-5678", Some("KR")), "+821012345678");
    assert_eq!(normalize("not a number", Some("KR")), "not a number");
    assert_eq!(normalize("650 555 0100", None), "650 555 0100");
}
