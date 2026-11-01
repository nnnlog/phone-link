use super::*;

fn item(taken: i64, modified: i64) -> MediaItem {
    MediaItem {
        id: 1,
        name: "IMG.jpg".into(),
        modified_ms: modified,
        taken_ms: taken,
        orientation: String::new(),
        mime_type: String::new(),
        height: 0,
        width: 0,
        size: 0,
        uri: String::new(),
        checksum: 0,
        thumbnail: None,
    }
}

#[test]
fn heuristic_needs_all_three_within_the_window() {
    let now = 1_000_000;
    assert!(is_new_photo(&item(now - 5_000, now - 4_000), now, 120_000));
    assert!(!is_new_photo(&item(now - 120_000, now - 1_000), now, 120_000), "taken 120 s ago");
    assert!(!is_new_photo(&item(now - 1_000, now - 120_000), now, 120_000), "modified 120 s ago");
    assert!(!is_new_photo(&item(now - 119_000, now + 1_000), now, 120_000), "taken and modified 120 s apart");
    assert!(is_new_photo(&item(now + 50_000, now + 50_000), now, 120_000), "future times count as recent");
    assert!(!is_new_photo(&item(i64::MIN, i64::MAX), now, 120_000));
}
