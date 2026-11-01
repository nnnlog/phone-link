//! The text of an MMS subject.
//!
//! The phone sends a subject as Android's MMS store keeps it: the subject's bytes in its own character
//! set, each byte as the character of the same value (ISO-8859-1), with the set's IANA `MIBenum` in
//! `subjectCharsets`. Shown as it is, a UTF-8 subject is mojibake (`텔` = `ED 85 94` arrives as `í`,
//! U+0085, U+0094), so the text is decoded from those bytes as Android's own MMS reader decodes it.

use encoding_rs::Encoding;

/// The text of a received MMS subject: `raw` is its `subjects` element, `charset` its
/// `subjectCharsets` element.
pub(super) fn mms_subject_text(raw: &str, charset: i32) -> String {
    // A character above U+00FF is no byte: the phone sent text, which is shown as it is.
    let Some(bytes) = raw.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect::<Option<Vec<u8>>>() else {
        return raw.to_owned();
    };

    match charset {
        // 0 is "any character set", which Android reads as its default, UTF-8.
        0 | 106 => decode(encoding_rs::UTF_8, &bytes),
        3 => bytes.iter().map(|&b| if b.is_ascii() { char::from(b) } else { char::REPLACEMENT_CHARACTER }).collect(),
        5 => decode(encoding_rs::ISO_8859_2, &bytes),
        6 => decode(encoding_rs::ISO_8859_3, &bytes),
        7 => decode(encoding_rs::ISO_8859_4, &bytes),
        8 => decode(encoding_rs::ISO_8859_5, &bytes),
        9 => decode(encoding_rs::ISO_8859_6, &bytes),
        10 => decode(encoding_rs::ISO_8859_7, &bytes),
        11 => decode(encoding_rs::ISO_8859_8, &bytes),
        12 => bytes.iter().map(|&b| iso_8859_9(b)).collect(),
        17 => decode(encoding_rs::SHIFT_JIS, &bytes),
        18 => decode(encoding_rs::EUC_JP, &bytes),
        38 => decode(encoding_rs::EUC_KR, &bytes),
        // ISO-2022-JP-2 (40) is read as ISO-2022-JP: its further sets decode to U+FFFD.
        39 | 40 => decode(encoding_rs::ISO_2022_JP, &bytes),
        113 | 2025 => decode(encoding_rs::GBK, &bytes),
        114 => decode(encoding_rs::GB18030, &bytes),
        2026 => decode(encoding_rs::BIG5, &bytes),
        1000 => decode(encoding_rs::UTF_16BE, &bytes),
        1015 => match bytes.as_slice() {
            [0xFF, 0xFE, rest @ ..] => decode(encoding_rs::UTF_16LE, rest),
            [0xFE, 0xFF, rest @ ..] => decode(encoding_rs::UTF_16BE, rest),
            _ => decode(encoding_rs::UTF_16BE, &bytes),
        },
        // ISO-8859-1 (4), HZ-GB-2312 (2085), which is not decoded, and a MIBenum Android does not
        // know: the bytes read as ISO-8859-1, which is the string received.
        _ => raw.to_owned(),
    }
}

/// The bytes in `encoding`, a malformed sequence as U+FFFD; a byte order mark is text.
fn decode(encoding: &'static Encoding, bytes: &[u8]) -> String {
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

/// ISO-8859-9 (Latin-5) is ISO-8859-1 with six Turkish letters in place of Icelandic ones.
fn iso_8859_9(b: u8) -> char {
    match b {
        0xD0 => 'Ğ',
        0xDD => 'İ',
        0xDE => 'Ş',
        0xF0 => 'ğ',
        0xFD => 'ı',
        0xFE => 'ş',
        _ => char::from(b),
    }
}

#[cfg(test)]
mod tests;
