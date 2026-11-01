use super::*;

#[test]
fn request_bodies() {
    assert_eq!(probe_request(), [0x08, 0x01]);
    assert_eq!(init_ok("abc"), [0x0a, 0x03, 0x61, 0x62, 0x63, 0x10, 0x01]);
}

#[test]
fn receive_request_checks() {
    let ok = NanoDelegateInitRequest { file_transfer_session_id: "s".into() }.encode_to_vec();
    assert_eq!(receive_request(true, RECEIVE_PATH, &ok), ReceiveRequest::Session("s".into()));
    assert_eq!(receive_request(true, "/fileshare_android2cdeh_phase1?x=1", &ok), ReceiveRequest::Session("s".into()));

    let status = |r: ReceiveRequest| match r {
        ReceiveRequest::Refuse(b) => NanoDelegateInitResponse::decode(b.as_slice()).unwrap().status,
        ReceiveRequest::Session(_) => panic!("accepted"),
    };
    assert_eq!(status(receive_request(false, RECEIVE_PATH, &ok)), 4);
    assert_eq!(status(receive_request(true, "/other", &ok)), 5);
    assert_eq!(status(receive_request(true, "", &ok)), 5);
    assert_eq!(status(receive_request(true, RECEIVE_PATH, &[])), 3);
    assert_eq!(status(receive_request(true, RECEIVE_PATH, &[0xff])), 2);
}

#[test]
fn file_names() {
    for good in ["a.txt", "photo (1).jpg", ".bashrc", "한글 파일.pdf", "a b"] {
        assert!(valid_file_name(good), "{good}");
    }
    for bad in ["", "   ", "a/b", "a\\b", "/abs", "x:y", "q?", "s*", "t\"", "l<", "g>", "p|", "end.", "end ", "c\u{1}"]
    {
        assert!(!valid_file_name(bad), "{bad:?}");
    }
}

#[test]
fn candidates() {
    assert_eq!(candidate("a.tar.gz", 0), "a.tar.gz");
    assert_eq!(candidate("a.tar.gz", 1), "a.tar (1).gz");
    assert_eq!(candidate(".bashrc", 1), " (1).bashrc");
    assert_eq!(candidate("notes", 2), "notes (2)");
}
