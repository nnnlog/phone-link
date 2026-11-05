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

#[test]
fn phone_number_characters_are_the_only_printable_ones_allowed() {
    for yes in [
        "",
        "0123456789",
        "+1 (650) 555-0100",
        "*31#",
        "?./",
        "010\u{A0}1234\u{2011}5678",
        // Format and control characters are not printable, so they are ignored.
        "\u{202A}+82 10-1234-5678\u{202C}",
        "\u{200E}22395",
        "1\t2\n3",
        // A character outside the Basic Multilingual Plane is two surrogates, neither printable.
        "\u{1F4DE}112",
    ] {
        assert!(has_only_phone_number_chars(yes), "{yes:?}");
    }

    for no in [
        "AMAZON",
        "1-800-FLOWERS",
        "123,456",
        "x@y",
        "\u{FF11}23",
        "\u{661}\u{662}",
        "12\u{2002}34",
        "12_34",
        "e\u{301}",
        "tel:123",
    ] {
        assert!(!has_only_phone_number_chars(no), "{no:?}");
    }
}

#[test]
fn a_lookup_address_is_an_email_address_or_phone_number_characters_and_not_blank() {
    for yes in ["+821012345678", "22395", "kim@example.com", "MAILTO:x@y.io", "\u{202A}112\u{202C}"] {
        assert!(is_lookup_address(yes), "{yes:?}");
    }

    for no in [
        "",
        " ",
        "\u{A0}\u{3000}\t\r\n",
        "\u{85}\u{2028}\u{2029}",
        "AMAZON",
        "Bank of Korea",
        "kim@example",
        "1-800-FLOWERS",
    ] {
        assert!(!is_lookup_address(no), "{no:?}");
    }

    // White space alone fails even though every character is a phone-number character or not
    // printable; with anything else it passes when the rest does.
    assert!(!is_lookup_address("  \u{A0}"));
    assert!(is_lookup_address(" 1 "));
    // A format character alone is not white space: nothing printable is left, so it passes.
    assert!(is_lookup_address("\u{200E}"));
}
