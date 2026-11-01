use super::standins::MemoryPasteboard;
use super::*;

fn types(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn classification_is_by_kind_not_declared_order() {
    assert_eq!(classify(&types(&[uti::PLAIN_TEXT, uti::HTML, uti::TIFF])), Some(ClipKind::Image));
    assert_eq!(classify(&types(&[uti::PLAIN_TEXT, uti::PNG])), Some(ClipKind::Image));
    assert_eq!(classify(&types(&[uti::PLAIN_TEXT, uti::HTML])), Some(ClipKind::Html));
    assert_eq!(classify(&types(&[uti::HTML])), Some(ClipKind::Html));
    assert_eq!(classify(&types(&[uti::PLAIN_TEXT, "public.rtf"])), Some(ClipKind::Text));
    assert_eq!(classify(&types(&["com.adobe.pdf"])), None, "a vector rendering is not an image");
    assert_eq!(classify(&types(&["public.rtf"])), None, "RTF only");
    assert_eq!(classify(&types(&[])), None);
}

#[test]
fn files_and_markers_mean_nothing_recognized() {
    assert_eq!(classify(&types(&[uti::FILE_URL, uti::PLAIN_TEXT, uti::TIFF])), None);
    assert_eq!(classify(&types(&[uti::PLAIN_TEXT, uti::CONCEALED])), None);
    assert_eq!(classify(&types(&[uti::TRANSIENT, uti::HTML, uti::PLAIN_TEXT])), None);
}

#[test]
fn the_text_limit_counts_utf16_units() {
    let pasteboard = MemoryPasteboard::new();
    // U+1F600 is two UTF-16 code units (four UTF-8 bytes).
    pasteboard.copy_text("ab\u{1F600}");
    assert_eq!(read_plain_text(&*pasteboard, 4).unwrap(), "ab\u{1F600}");
    assert_eq!(read_plain_text(&*pasteboard, 3), Err(ReadError::TooLong { uti: uti::PLAIN_TEXT, units: 4 }));

    pasteboard.copy(&[(uti::PLAIN_TEXT, &[0x61, 0xFF, 0x62])]);
    assert_eq!(read_plain_text(&*pasteboard, 10).unwrap(), "a\u{FFFD}b", "invalid UTF-8 is replaced");

    assert_eq!(read_html(&*pasteboard, 10), Err(ReadError::Absent(uti::HTML)));
}

#[test]
fn png_is_read_before_tiff() {
    let pasteboard = MemoryPasteboard::new();
    pasteboard.copy(&[(uti::TIFF, b"tiff"), (uti::PNG, b"png")]);
    assert_eq!(read_image(&*pasteboard).unwrap(), b"png");

    pasteboard.copy(&[(uti::TIFF, b"tiff")]);
    assert_eq!(read_image(&*pasteboard).unwrap(), b"tiff");

    pasteboard.copy_text("x");
    assert_eq!(read_image(&*pasteboard), Err(ReadError::Absent(uti::PNG)));
}

#[test]
fn the_memory_pasteboard_counts_changes() {
    let pasteboard = MemoryPasteboard::new();
    assert_eq!(pasteboard.change_count().unwrap(), 0);

    pasteboard.copy_text("a");
    let written = pasteboard.write(vec![representation(uti::PLAIN_TEXT, b"b".to_vec())]).unwrap();
    assert_eq!(written, ClipboardPasteboardWrite { change_count: 2, failure: None });
    assert_eq!(pasteboard.writes().len(), 1);

    // A refused representation stops the write after the clear: the count still moved.
    pasteboard.refuse_type(Some(uti::TIFF));
    let partial = pasteboard
        .write(vec![representation(uti::PNG, b"png".to_vec()), representation(uti::TIFF, b"tiff".to_vec())])
        .unwrap();
    assert_eq!(partial.change_count, 3);
    assert!(partial.failure.is_some_and(|f| f.contains(uti::TIFF)));
    assert_eq!(pasteboard.contents(), vec![representation(uti::PNG, b"png".to_vec())]);

    pasteboard.set_failing(true);
    assert!(pasteboard.types().is_err());
    assert!(pasteboard.write(vec![]).is_err());
    assert_eq!(pasteboard.current_change_count(), 3, "a failed write changes nothing");
}
