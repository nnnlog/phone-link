use super::*;
use phonelink_clipboard::pasteboard::uti;
use std::time::Duration;

#[test]
fn a_read_takes_only_the_answer_to_itself() {
    let (sender, receiver) = mpsc::channel();
    let (target, property) = (31, 401);
    for event in [
        ReaderEvent::Converted { target: 30, property },
        ReaderEvent::Converted { target, property: 400 },
        ReaderEvent::NewValue(property),
        ReaderEvent::Converted { target, property },
    ] {
        sender.send(event).unwrap();
    }
    drop(sender);
    let answer = |event: &ReaderEvent| match *event {
        ReaderEvent::Converted { target: t, property: p } if t == target && (p == property || p == NONE) => Some(p),
        _ => None,
    };
    assert_eq!(wait_for(&receiver, answer), Some(property));
    assert_eq!(wait_for(&receiver, answer), None, "nothing more comes");
}

#[test]
#[ignore = "needs an X11 display and replaces its clipboard"]
fn round_trips_through_the_live_x11_clipboard() {
    let ours = X11Pasteboard::connect().unwrap();
    let other = X11Pasteboard::connect().unwrap();
    let original = other.data(uti::PLAIN_TEXT.to_owned()).unwrap();
    let count = other.change_count().unwrap();

    let written = ours
        .write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: "phonelink é".into() }])
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(ours.change_count().unwrap(), written.change_count);
    assert!(other.change_count().unwrap() > count);
    assert!(other.types().unwrap().contains(&uti::PLAIN_TEXT.to_owned()));
    assert_eq!(other.data(uti::PLAIN_TEXT.to_owned()).unwrap().as_deref(), Some("phonelink é".as_bytes()));
    let string = intern(&other.clipboard.connection, "STRING").unwrap();
    assert_eq!(other.convert(string).unwrap().as_deref(), Some(&b"phonelink \xe9"[..]));

    let large = vec![b'x'; INCR_CHUNK * 3 + 17];
    ours.write(vec![ClipboardPasteboardData { uti: uti::HTML.to_owned(), data: large.clone() }]).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(other.data(uti::HTML.to_owned()).unwrap(), Some(large));

    let representation = |uti: &str, data: &str| ClipboardPasteboardData { uti: uti.to_owned(), data: data.into() };
    let written = ours
        .write(vec![
            representation(uti::HTML, ""),
            representation(uti::FILE_URL, "https://example.com/\r\n"),
            representation("public.rtf", "{\\rtf1}"),
            representation(uti::PLAIN_TEXT, "after"),
        ])
        .unwrap();
    assert_eq!(written.failure.as_deref(), Some("the clipboard did not take public.rtf"));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(other.types().unwrap(), [uti::HTML], "web links are no files; nothing after the refusal");
    assert_eq!(other.data(uti::HTML.to_owned()).unwrap(), Some(Vec::new()), "empty data is data");
    ours.write(vec![representation(uti::FILE_URL, "file:///tmp/a.png\r\n")]).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(other.types().unwrap(), [uti::FILE_URL]);

    if let Some(original) = original {
        ours.write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: original }]).unwrap();
        thread::sleep(Duration::from_millis(300));
    }
}
