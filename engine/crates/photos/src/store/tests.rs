use super::*;
use phonelink_core::types::{DcgClient, Environment};
use phonelink_features::shared::db::StoreLocation;

fn db() -> FeatureDb {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    FeatureDb::open_phone(&StoreLocation::Memory, &phone, FEATURE, MIGRATIONS).unwrap()
}

fn item(id: i64, modified: i64, taken: i64, checksum: i64, thumb: Option<&[u8]>) -> MediaItem {
    MediaItem {
        id,
        name: format!("IMG_{id}.jpg"),
        modified_ms: modified,
        taken_ms: taken,
        orientation: "0".into(),
        mime_type: "image/jpeg".into(),
        height: 10,
        width: 20,
        size: 30,
        uri: format!("content://{id}"),
        checksum,
        thumbnail: thumb.map(<[u8]>::to_vec),
    }
}

fn meta(ids: &[i64], times: &[i64], checksums: Option<&[i64]>) -> Metadata {
    Metadata {
        correlation_vector: "cv".into(),
        ids: ids.to_vec(),
        times: times.to_vec(),
        checksums: checksums.map(<[i64]>::to_vec),
        settings: None,
        sequence_number: Some(5),
        repeated: 0,
    }
}

#[test]
fn new_state_row_holds_the_defaults() {
    let s = state(&db()).unwrap();
    assert_eq!(
        s,
        PhotosState {
            current_sequence: -1,
            expected_sequence: -1,
            last_sync_ms: None,
            capabilities: 0,
            consent: true,
            deletion_allowed: true,
            new_photo_notifications: true,
            full_access_reminder_dismissed: false,
        }
    );
    assert!(s.up_to_date());
}

#[test]
fn reconcile_with_checksums() {
    let db = db();
    apply_items(
        &db,
        &[item(1, 10, 10, 100, None), item(2, 20, 20, 200, Some(b"t")), item(9, 1, 1, 9, None)],
        &[],
        7,
        None,
    )
    .unwrap();

    let r = reconcile(&db, &meta(&[3, 2, 1], &[30, 20, 10], Some(&[300, 201, 100])), None, 50).unwrap();

    // 9 is gone; 3 is new; 2's checksum differs; 1 matches even without a thumbnail.
    assert_eq!(r.deleted, [9]);
    assert_eq!(r.outgoing, [3, 2]);
    assert_eq!(state(&db).unwrap().current_sequence, -1, "pending: no completion");
}

#[test]
fn reconcile_without_checksums_needs_thumbnail_and_time() {
    let db = db();
    apply_items(
        &db,
        &[item(1, 10, 10, 0, Some(b"t")), item(2, 20, 20, 0, None), item(3, 30, 30, 0, Some(b"t"))],
        &[],
        7,
        None,
    )
    .unwrap();

    let r = reconcile(&db, &meta(&[1, 2, 3], &[10, 20, 31], None), None, 50).unwrap();
    assert_eq!(r.outgoing, [2, 3]);

    // Nothing pending: the sync completes with its sequence number and the sync time.
    let r = reconcile(&db, &meta(&[1], &[10], None), Some(3), 60).unwrap();
    assert_eq!((r.outgoing.len(), r.deleted.clone()), (0, vec![2, 3]));
    assert!(r.capabilities_changed);
    let s = state(&db).unwrap();
    assert_eq!((s.current_sequence, s.last_sync_ms, s.capabilities), (5, Some(60), 3));
}

#[test]
fn an_empty_list_clears_the_cache() {
    for sums in [None, Some(&[][..])] {
        let db = db();
        apply_items(&db, &[item(1, 10, 10, 0, None)], &[], 7, None).unwrap();
        let r = reconcile(&db, &meta(&[], &[], sums), None, 50).unwrap();
        assert_eq!(r.deleted, [1]);
        assert!(gallery(&db).unwrap().is_empty());
    }
}

#[test]
fn overwrite_rule_and_last_seen() {
    let db = db();
    apply_items(&db, &[item(1, 10, 10, 1, Some(b"a"))], &[], 7, None).unwrap();
    set_full_bytes(&db, &[(1, b"FULL".to_vec())]).unwrap();

    // Same times and URI, cached thumbnail present: nothing is overwritten (not even the checksum),
    // but the carried thumbnail with a different checksum drops the full bytes.
    let mut same = item(1, 10, 10, 2, Some(b"b"));
    same.name = "other".into();
    apply_items(&db, &[same], &[], 99, None).unwrap();
    let r = super::item(&db, 1).unwrap().unwrap();
    assert_eq!((r.name.as_str(), r.checksum, r.last_seen_ms, r.has_full), ("IMG_1.jpg", 1, Some(7), false));
    assert_eq!(thumbnail(&db, 1).unwrap().as_deref(), Some(&b"a"[..]));

    // A changed time overwrites everything except the last-seen time; no thumbnail clears it.
    apply_items(&db, &[item(1, 11, 10, 3, None)], &[], 99, None).unwrap();
    let r = super::item(&db, 1).unwrap().unwrap();
    assert_eq!((r.modified_ms, r.checksum, r.has_thumbnail, r.last_seen_ms), (11, 3, false, Some(7)));

    // Without a cached thumbnail any item overwrites.
    apply_items(&db, &[item(1, 11, 10, 4, Some(b"c"))], &[], 99, None).unwrap();
    assert_eq!(thumbnail(&db, 1).unwrap().as_deref(), Some(&b"c"[..]));

    // Same checksum with a thumbnail keeps full bytes.
    set_full_bytes(&db, &[(1, b"FULL".to_vec())]).unwrap();
    apply_items(&db, &[item(1, 11, 10, 4, Some(b"d"))], &[], 99, None).unwrap();
    assert!(super::item(&db, 1).unwrap().unwrap().has_full);

    // Removed ids are deleted.
    apply_items(&db, &[], &[1], 99, Some(Completion { now_ms: 100, sequence_number: None })).unwrap();
    assert!(gallery(&db).unwrap().is_empty());
    assert_eq!(state(&db).unwrap().last_sync_ms, Some(100));
}

#[test]
fn an_item_message_reports_the_entries_it_wrote() {
    let db = db();
    let written = apply_items(&db, &[item(1, 10, 10, 1, Some(b"a")), item(2, 20, 20, 2, Some(b"b"))], &[], 7, None);
    assert_eq!(written.unwrap(), [1, 2], "inserted");

    set_full_bytes(&db, &[(1, b"FULL".to_vec())]).unwrap();

    // Same times and URI with a cached thumbnail, and no other checksum with a carried thumbnail:
    // the upsert writes nothing.
    let unchanged = apply_items(&db, &[item(1, 10, 10, 1, Some(b"a")), item(2, 20, 20, 2, None)], &[], 8, None);
    assert!(unchanged.unwrap().is_empty());

    // A carried thumbnail with another checksum drops the full bytes: written.
    assert_eq!(apply_items(&db, &[item(1, 10, 10, 9, Some(b"a"))], &[], 8, None).unwrap(), [1]);
    assert!(!super::item(&db, 1).unwrap().unwrap().has_full);

    // A changed time overwrites: written.
    assert_eq!(apply_items(&db, &[item(2, 21, 20, 2, None)], &[], 8, None).unwrap(), [2]);

    // A removed id is not written.
    assert!(apply_items(&db, &[], &[1], 8, None).unwrap().is_empty());
}

#[test]
fn full_bytes_only_for_cached_items_and_display_order() {
    let db = db();
    apply_items(&db, &[item(1, 5, 10, 0, None), item(2, 6, 30, 0, None), item(3, 7, 30, 0, None)], &[], 7, None)
        .unwrap();

    assert_eq!(set_full_bytes(&db, &[(2, vec![1]), (77, vec![2])]).unwrap(), [2]);
    assert_eq!(gallery(&db).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [3, 2, 1]);
    assert_eq!(ids_without_full(&db).unwrap(), HashSet::from([1, 3]));
    assert_eq!(ids_without_thumbnail(&db).unwrap(), HashSet::from([1, 2, 3]));
    assert_eq!(full_bytes(&db, 2).unwrap(), Some(vec![1]));
    assert_eq!(full_bytes(&db, 1).unwrap(), None);
    assert_eq!(full_bytes(&db, 77).unwrap(), None);
}

#[test]
fn maintenance_keeps_the_25_most_recent_and_recently_seen() {
    let db = db();
    let day = 86_400_000i64;
    let now = 100 * day;

    let items: Vec<MediaItem> = (0..30).map(|i| item(i, i, i, 0, None)).collect();
    apply_items(&db, &items, &[], 0, None).unwrap();
    let images: Vec<(i64, Vec<u8>)> = (0..30).map(|i| (i, vec![1])).collect();
    set_full_bytes(&db, &images).unwrap();

    // Item 0 was seen recently; items 1..30 have last_seen 0 → stamped now first.
    set_last_seen(&db, 0, now - day).unwrap();
    assert!(maintain(&db, now, 25, 30 * day).unwrap().is_empty(), "stamped now: nothing is old");

    // 40 days later: the 5 oldest (0..5) are outside the 25 most recent.
    let later = now + 40 * day;
    set_last_seen(&db, 2, later - day).unwrap();
    let mut dropped = maintain(&db, later, 25, 30 * day).unwrap();
    dropped.sort_unstable();
    assert_eq!(dropped, [0, 1, 3, 4]);

    let r = super::item(&db, 5).unwrap().unwrap();
    assert!(r.has_full, "inside the 25 most recent");
    assert_eq!(r.last_seen_ms, Some(now));
}

#[test]
fn settings_and_sequence() {
    let db = db();
    set_setting(&db, Setting::Consent, false).unwrap();
    set_setting(&db, Setting::FullAccessReminderDismissed, true).unwrap();
    set_expected(&db, 9).unwrap();

    assert!(!set_capabilities(&db, 0).unwrap());
    assert!(set_capabilities(&db, 1).unwrap());

    let s = state(&db).unwrap();
    assert!(!s.consent && s.full_access_reminder_dismissed && !s.up_to_date());
    assert_eq!((s.expected_sequence, s.capabilities), (9, 1));
}
