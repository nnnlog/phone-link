use super::*;

fn mimes(list: &[&str]) -> Vec<String> {
    list.iter().map(|m| (*m).to_owned()).collect()
}

fn text(data: &str) -> ClipboardPasteboardData {
    ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: data.as_bytes().to_vec() }
}

#[test]
fn offered_mimes_map_to_utis() {
    assert_eq!(utis_of(&mimes(&["UTF8_STRING", "text/html"])), [uti::HTML, uti::PLAIN_TEXT]);
    assert_eq!(utis_of(&mimes(&["image/png", "x-kde-passwordManagerHint"])), [uti::PNG, uti::CONCEALED]);
    assert_eq!(first_offered(uti::PLAIN_TEXT, &mimes(&["STRING", "UTF8_STRING"])), Some("UTF8_STRING"));
    assert_eq!(first_offered(uti::PLAIN_TEXT, &mimes(&["STRING", "TEXT"])), Some("STRING"));
    assert!(utis_of(&mimes(&["application/x-unknown"])).is_empty());
}

#[test]
fn only_a_list_with_a_local_file_is_copied_files() {
    assert!(holds_file_uri(b"# copied\r\nfile:///home/u/a%20b.txt\r\n"));
    assert!(holds_file_uri(b"https://example.com/\nFILE:///tmp/x\n"));
    assert!(!holds_file_uri(b"https://example.com/page\r\n"));
    assert!(!holds_file_uri(b"# file:///commented\n\n"));

    let offered = mimes(&["text/uri-list", "text/plain"]);
    let links = utis_with_files(&offered, || Ok(Some(b"https://example.com/\r\n".to_vec()))).unwrap();
    assert_eq!(links, [uti::PLAIN_TEXT]);
    let files = utis_with_files(&offered, || Ok(Some(b"file:///tmp/a.png\r\n".to_vec()))).unwrap();
    assert_eq!(files, [uti::PLAIN_TEXT, uti::FILE_URL]);
    let unread = utis_with_files(&offered, || Ok(None)).unwrap();
    assert_eq!(unread, [uti::PLAIN_TEXT]);
    let never = utis_with_files(&mimes(&["text/plain"]), || panic!("no list to read")).unwrap();
    assert_eq!(never, [uti::PLAIN_TEXT]);
}

#[test]
fn string_holds_latin1_text() {
    let (data, failure) = mime_data(vec![text("café")]);
    assert_eq!(failure, None);
    assert_eq!(data["UTF8_STRING"].as_slice(), "café".as_bytes());
    assert_eq!(data["STRING"].as_slice(), b"caf\xe9");
    assert_eq!(utf8_text("STRING", b"caf\xe9".to_vec()), "café".as_bytes());
    assert_eq!(utf8_text("UTF8_STRING", "café".as_bytes().to_vec()), "café".as_bytes());

    let (data, _) = mime_data(vec![text("한글")]);
    assert!(!data.contains_key("STRING"), "text beyond Latin-1 is not offered as STRING");
    assert_eq!(data["text/plain;charset=utf-8"].as_slice(), "한글".as_bytes());
}

#[test]
fn a_type_without_a_mime_type_stops_the_write() {
    let html = ClipboardPasteboardData { uti: uti::HTML.to_owned(), data: b"<b>x</b>".to_vec() };
    let rtf = ClipboardPasteboardData { uti: "public.rtf".to_owned(), data: b"{\\rtf1}".to_vec() };
    let (data, failure) = mime_data(vec![html, rtf, text("after")]);
    assert_eq!(failure.as_deref(), Some("the clipboard did not take public.rtf"));
    assert_eq!(data.keys().map(String::as_str).collect::<Vec<_>>(), ["text/html"]);
}
