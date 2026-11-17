use super::*;

#[test]
fn a_relative_cache_folder_is_ignored() {
    let home = || Some(OsString::from("/home/a"));
    assert_eq!(cache_dir_in(Some("/cache".into()), home()), Path::new("/cache/phonelink"));
    assert_eq!(cache_dir_in(Some("cache".into()), home()), Path::new("/home/a/.cache/phonelink"));
    assert_eq!(cache_dir_in(Some("".into()), home()), Path::new("/home/a/.cache/phonelink"));
    assert_eq!(cache_dir_in(None, Some("home".into())), std::env::temp_dir().join("phonelink"));
}

#[test]
fn names_keep_only_safe_characters() {
    assert_eq!(safe_name("com.kakao.talk"), "com.kakao.talk");
    assert_eq!(safe_name("../x y"), ".._x_y");
    assert_eq!(safe_name(".."), "__");
}

#[test]
fn file_urls_round_trip_through_local_path() {
    for name in ["/tmp/한글 문서.pdf", "/tmp/100%.png", "/tmp/C#.txt", "/tmp/q?.txt", "/tmp/[x].txt"] {
        assert_eq!(local_path(&file_url(Path::new(name))).as_deref(), Some(name));
    }
}

#[test]
fn a_qr_code_is_a_png() {
    let png = qr_png("https://example.com/pair?code=123456").expect("a QR code");
    assert!(png.starts_with(b"\x89PNG"));
}

#[test]
fn qml_url_strings_decode_to_paths() {
    assert_eq!(local_path("file:///tmp/한글 문서.pdf").as_deref(), Some("/tmp/한글 문서.pdf"));
    assert_eq!(local_path("file:///tmp/100%25.png").as_deref(), Some("/tmp/100%.png"));
    assert_eq!(local_path("file:///tmp/C%23.txt").as_deref(), Some("/tmp/C#.txt"));
    assert_eq!(local_path("https://example.com/a"), None);
}
