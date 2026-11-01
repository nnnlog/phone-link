use super::mms_subject_text;

/// The string the phone sends for `bytes`: each byte as the character of the same value.
fn as_sent(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[test]
fn a_utf8_subject_is_decoded_from_its_bytes() {
    let subject = "[알림] 이번 주 모임 장소 변경 안내";
    let sent = as_sent(subject.as_bytes());
    assert!(sent.starts_with("[ì\u{95}\u{8c}"), "the mojibake the phone sends");
    assert_eq!(mms_subject_text(&sent, 106), subject);
    assert_eq!(mms_subject_text(&sent, 0), subject, "any charset reads as UTF-8");
}

#[test]
fn multibyte_charsets() {
    // "안녕" in EUC-KR, "日本" in Shift_JIS, "中文" in GBK, in GB2312 and in Big5.
    assert_eq!(mms_subject_text(&as_sent(&[0xBE, 0xC8, 0xB3, 0xE7]), 38), "안녕");
    assert_eq!(mms_subject_text(&as_sent(&[0x93, 0xFA, 0x96, 0x7B]), 17), "日本");
    assert_eq!(mms_subject_text(&as_sent(&[0xD6, 0xD0, 0xCE, 0xC4]), 113), "中文");
    assert_eq!(mms_subject_text(&as_sent(&[0xD6, 0xD0, 0xCE, 0xC4]), 2025), "中文");
    assert_eq!(mms_subject_text(&as_sent(&[0xA4, 0xA4, 0xA4, 0xE5]), 2026), "中文");
}

#[test]
fn utf16_charsets_follow_the_byte_order_mark_else_big_endian() {
    assert_eq!(mms_subject_text(&as_sent(&[0xD5, 0x5C]), 1000), "한");
    assert_eq!(mms_subject_text(&as_sent(&[0xD5, 0x5C]), 1015), "한");
    assert_eq!(mms_subject_text(&as_sent(&[0xFE, 0xFF, 0xD5, 0x5C]), 1015), "한");
    assert_eq!(mms_subject_text(&as_sent(&[0xFF, 0xFE, 0x5C, 0xD5]), 1015), "한");
}

#[test]
fn single_byte_charsets() {
    assert_eq!(mms_subject_text("café", 4), "café");
    assert_eq!(mms_subject_text(&as_sent(&[b'a', 0xE9]), 3), "a\u{FFFD}");
    assert_eq!(mms_subject_text(&as_sent(&[0xF0, 0xFD, 0xE9]), 12), "ğıé");
    assert_eq!(mms_subject_text(&as_sent(&[0xB0, 0xB1]), 8), "АБ", "ISO-8859-5");
}

#[test]
fn malformed_bytes_decode_to_the_replacement_character() {
    assert_eq!(mms_subject_text(&as_sent(&[b'a', 0xED, 0x85]), 106), "a\u{FFFD}");
}

#[test]
fn text_and_unknown_charsets_are_shown_as_they_are() {
    assert_eq!(mms_subject_text("텔레콤", 106), "텔레콤", "not a byte mapping");
    assert_eq!(mms_subject_text("~{abc~}", 2085), "~{abc~}");
    assert_eq!(mms_subject_text("plain", 9999), "plain");
    assert_eq!(mms_subject_text("", 106), "");
}
