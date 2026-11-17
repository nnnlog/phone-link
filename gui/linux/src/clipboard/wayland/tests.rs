use std::time::Duration;

use phonelink_clipboard::pasteboard::uti;

use super::*;

#[test]
#[ignore = "needs a Wayland session and replaces its clipboard"]
fn round_trips_through_the_live_wayland_clipboard() {
    let ours = WaylandPasteboard::<Ext>::connect().unwrap();
    let other = WaylandPasteboard::<Ext>::connect().unwrap();
    thread::sleep(Duration::from_millis(200));
    let original = other.data(uti::PLAIN_TEXT.to_owned()).unwrap();
    let count = other.change_count().unwrap();

    let written = ours
        .write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: b"phonelink".to_vec() }])
        .unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(ours.change_count().unwrap(), written.change_count);
    assert!(other.change_count().unwrap() > count);
    assert!(other.types().unwrap().contains(&uti::PLAIN_TEXT.to_owned()));
    assert_eq!(other.data(uti::PLAIN_TEXT.to_owned()).unwrap().as_deref(), Some(&b"phonelink"[..]));

    if let Some(original) = original {
        ours.write(vec![ClipboardPasteboardData { uti: uti::PLAIN_TEXT.to_owned(), data: original }]).unwrap();
        thread::sleep(Duration::from_millis(300));
    }
}
