use super::*;
use phonelink_app::RequestKind;

fn items_payload(n: usize) -> ValueSet {
    let ids: Vec<i64> = (0..n as i64).map(|i| 100 + i).collect();
    ValueSet::new()
        .with(key::MEDIA_IDS, ids)
        .with(key::MEDIA_NAMES, (0..n).map(|i| format!("IMG_{i}.jpg")).collect::<Vec<_>>())
        .with(key::MEDIA_TIMES, vec![1_000i64; n])
        .with(key::MEDIA_TAKEN_TIMES, vec![900i64; n])
        .with(key::MEDIA_ORIENTATIONS, vec!["0"; n])
        .with(key::MEDIA_MIME_TYPES, vec!["image/jpeg"; n])
        .with(key::MEDIA_HEIGHTS, vec![4000i64; n])
        .with(key::MEDIA_WIDTHS, vec![3000i64; n])
        .with(key::MEDIA_SIZES, vec![123_456i64; n])
        .with(key::MEDIA_ACTIONS, vec![0i32; n])
}

#[test]
fn metadata_keeps_the_first_occurrence_of_each_id() {
    let vs = ValueSet::new()
        .with("correlationVector", "cv")
        .with("metadata", true)
        .with(key::MEDIA_IDS, vec![3i64, 2, 3, 1, 2])
        .with(key::MEDIA_TIMES, vec![30i64, 20, 31, 10, 21])
        .with(key::CHECKSUMS, vec![300i64, 200, 301, 100, 201])
        .with(key::SEQUENCE_NUMBER, 7i64);

    let m = parse_metadata(&vs).unwrap();
    assert_eq!(m.ids, [3, 2, 1]);
    assert_eq!(m.times, [30, 20, 10]);
    assert_eq!(m.checksums.as_deref(), Some(&[300i64, 200, 100][..]));
    assert_eq!(m.repeated, 2);
    assert_eq!(m.sequence_number, Some(7));
    assert_eq!(m.settings, None);
}

#[test]
fn metadata_defects() {
    let ok = ValueSet::new()
        .with("correlationVector", "cv")
        .with(key::MEDIA_IDS, vec![1i64, 2])
        .with(key::MEDIA_TIMES, vec![1i64, 2]);
    assert!(parse_metadata(&ok).is_ok());

    let mut v = ok.clone();
    v.remove("correlationVector");
    assert!(parse_metadata(&v).is_err());

    assert!(parse_metadata(&ok.clone().with(key::MEDIA_IDS, vec![1i32, 2])).is_err(), "Int32Array ids");
    assert!(parse_metadata(&ok.clone().with(key::MEDIA_TIMES, vec![1i64])).is_err(), "short times");
    assert!(parse_metadata(&ok.clone().with(key::CHECKSUMS, vec![1i64])).is_err(), "short checksums");
    assert!(parse_metadata(&ok.clone().with(key::CHECKSUMS, vec![1i32, 2])).is_err(), "Int32Array checksums");

    // mediaSettings of another type is ignored; a mistyped member is malformed.
    let m = parse_metadata(&ok.clone().with(key::MEDIA_SETTINGS, "x")).unwrap();
    assert_eq!(m.settings, None);
    let bad = ValueSet::new().with(key::MEDIA_CAPABILITIES, 1i64);
    assert!(parse_metadata(&ok.clone().with(key::MEDIA_SETTINGS, bad)).is_err());
    let bad = ValueSet::new().with(key::MEDIA_HAS_DELETE_PERMISSION, 1i32);
    assert!(parse_metadata(&ok.clone().with(key::MEDIA_SETTINGS, bad)).is_err());

    // Absent mediaCapabilities reads None; -1 is no sequence number; a mistyped one is ignored.
    let s = ValueSet::new().with(key::MEDIA_HAS_DELETE_PERMISSION, true);
    let m = parse_metadata(&ok.clone().with(key::MEDIA_SETTINGS, s).with(key::SEQUENCE_NUMBER, -1i64)).unwrap();
    assert_eq!(m.settings, Some(MediaSettings { capabilities: 0, has_delete_permission: Some(true) }));
    assert_eq!(m.sequence_number, None);
    let m = parse_metadata(&ok.with(key::SEQUENCE_NUMBER, 5i32)).unwrap();
    assert_eq!(m.sequence_number, None);
}

#[test]
fn items_slice_thumbnails_of_non_removed_items_in_order() {
    let vs = items_payload(4)
        .with(key::MEDIA_ACTIONS, vec![0i32, 2, 1, 0])
        .with(key::MEDIA_THUMBNAILS, vec![1u8, 2, 3, 4, 5, 6])
        // Item 1 is removed: its size (99) is not read and consumes nothing.
        .with(key::MEDIA_THUMBNAIL_SIZES, vec![2i64, 99, 0, 3])
        .with(key::CHECKSUMS, vec![7i64, 8, 9, 10])
        .with(key::MEDIA_URIS, vec!["a", "b", "c", "d"]);

    let b = parse_items(&vs).unwrap();
    assert_eq!(b.removed, [101]);
    assert_eq!(b.items.iter().map(|i| i.id).collect::<Vec<_>>(), [100, 102, 103]);
    assert_eq!(b.items[0].thumbnail.as_deref(), Some(&[1u8, 2][..]));
    assert_eq!(b.items[1].thumbnail, None);
    assert_eq!(b.items[2].thumbnail.as_deref(), Some(&[3u8, 4, 5][..]));
    assert_eq!((b.items[2].checksum, b.items[2].uri.as_str()), (10, "d"));
    assert_eq!((b.items[0].height, b.items[0].width, b.items[0].size), (4000, 3000, 123_456));
}

#[test]
fn items_defaults_and_defects() {
    let b = parse_items(&items_payload(2)).unwrap();
    assert_eq!((b.items[0].checksum, b.items[0].uri.as_str(), b.items[0].thumbnail.as_ref()), (0, "", None));

    // Optional arrays of another type count as absent; the thumbnails need both keys.
    let vs = items_payload(2)
        .with(key::MEDIA_URIS, vec![1i64, 2])
        .with(key::CHECKSUMS, vec!["x", "y"])
        .with(key::MEDIA_THUMBNAILS, vec![1u8, 2]);
    let b = parse_items(&vs).unwrap();
    assert_eq!((b.items[1].checksum, b.items[1].uri.as_str(), b.items[1].thumbnail.as_ref()), (0, "", None));

    // Required arrays: missing, mistyped or short.
    for k in [
        key::MEDIA_NAMES,
        key::MEDIA_TIMES,
        key::MEDIA_TAKEN_TIMES,
        key::MEDIA_ORIENTATIONS,
        key::MEDIA_MIME_TYPES,
        key::MEDIA_HEIGHTS,
        key::MEDIA_WIDTHS,
        key::MEDIA_SIZES,
        key::MEDIA_ACTIONS,
    ] {
        let mut v = items_payload(2);
        v.remove(k);
        assert!(parse_items(&v).is_err(), "{k} missing");
        assert!(parse_items(&items_payload(2).with(k, true)).is_err(), "{k} mistyped");
    }

    assert!(parse_items(&items_payload(2).with(key::MEDIA_NAMES, vec!["a"])).is_err());
    assert!(parse_items(&items_payload(2).with(key::CHECKSUMS, vec![1i64])).is_err());
    assert!(parse_items(&items_payload(2).with(key::MEDIA_URIS, vec!["a"])).is_err());

    let vs = items_payload(2).with(key::MEDIA_THUMBNAILS, vec![1u8, 2]).with(key::MEDIA_THUMBNAIL_SIZES, vec![1i64]);
    assert!(parse_items(&vs).is_err(), "short sizes");
    let vs = items_payload(2).with(key::MEDIA_THUMBNAILS, vec![1u8, 2]).with(key::MEDIA_THUMBNAIL_SIZES, vec![1i64, 2]);
    assert!(parse_items(&vs).is_err(), "slice beyond the blob");
    let vs = items_payload(1).with(key::MEDIA_THUMBNAILS, vec![1u8]).with(key::MEDIA_THUMBNAIL_SIZES, vec![i64::MAX]);
    assert!(parse_items(&vs).is_err());

    let mut v = items_payload(1);
    v.remove(key::MEDIA_IDS);
    assert!(parse_items(&v).is_err());

    // An empty message is valid.
    assert_eq!(parse_items(&items_payload(0)).unwrap(), ItemBatch::default());
}

#[test]
fn full_reply_slices() {
    let vs = ValueSet::new()
        .with(key::MEDIA_IDS, vec![5i64, 6])
        .with(key::MEDIA_SIZES, vec![3i64, 0])
        .with(key::CONTENT, vec![9u8, 8, 7]);
    assert_eq!(
        parse_full_reply(&vs).unwrap(),
        [FullImage { id: 5, bytes: vec![9, 8, 7] }, FullImage { id: 6, bytes: vec![] }]
    );

    assert!(parse_full_reply(&vs.clone().with(key::MEDIA_SIZES, vec![4i64, 0])).is_err());
    assert!(parse_full_reply(&vs.clone().with(key::MEDIA_SIZES, vec![-1i64, 4])).is_err());
    assert!(parse_full_reply(&vs.clone().with(key::MEDIA_SIZES, vec![3i64])).is_err());

    let mut v = vs;
    v.remove(key::CONTENT);
    assert!(parse_full_reply(&v).is_err());
}

#[test]
fn delete_reply_needs_both_arrays() {
    let vs = ValueSet::new()
        .with(key::MEDIA_SUCCESSFULLY_DELETED_IDS, vec![1i64])
        .with(key::MEDIA_UNSUCCESSFULLY_DELETED_IDS, Vec::<i64>::new());
    assert_eq!(parse_delete_reply(&vs), Some((vec![1], vec![])));
    assert_eq!(parse_delete_reply(&vs.clone().with(key::MEDIA_UNSUCCESSFULLY_DELETED_IDS, vec![1i32])), None);

    let mut v = vs;
    v.remove(key::MEDIA_SUCCESSFULLY_DELETED_IDS);
    assert_eq!(parse_delete_reply(&v), None);
}

#[test]
fn requests_carry_the_page_keys() {
    let t = TraceContext::new_root(Vec::new());
    let names = |r: &AppRequest| (r.options.trigger_id.clone(), r.options.scenario_id.clone());
    let pair = |a: &str, b: &str| (a.to_string(), b.to_string());

    let r = thumbnails_request(&[1, 2], t.clone(), trace_name::THUMBNAIL_REQUESTED);
    assert_eq!((r.route.as_str(), r.kind), (ROUTE_MEDIA, RequestKind::Legacy));
    assert!(!r.options.user_initiated);
    assert_eq!(r.options.timeout, None);
    assert_eq!(
        r.payload.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "isuserinitiated", "mediaIds"]
    );
    assert_eq!(r.payload.str("correlationVector"), Some(t.correlation_id.as_str()));
    assert_eq!(r.payload.str("contentType"), Some("thumbnails"));
    assert_eq!(r.payload.i64_array("mediaIds"), Some(&[1i64, 2][..]));
    assert_eq!(names(&r), pair("ThumnailRequested", "HydrateThumbnails"));

    let r = photos_request(9, t.clone(), trace_name::ON_COPY);
    assert_eq!(r.payload.str("contentType"), Some("photos"));
    assert_eq!(r.payload.i64_array("mediaIds"), Some(&[9i64][..]));
    assert_eq!(names(&r), pair("OnCopy", "HydratePhoto"));

    let r = sync_request(t.clone(), trace_name::FULL_PERMISSION_REQUEST_INVOKED);
    assert_eq!(r.payload.str("contentType"), Some("mediaInfo"));
    assert_eq!(r.payload.string_array("relatedCorrelationIds"), Some(&[t.correlation_id.clone()][..]));
    assert_eq!(r.payload.string_array("subcontentTypes"), Some(&["mediaInfo".to_string()][..]));
    assert_eq!(names(&r), pair("PhotosFullPermissionRequestInvoked", "SyncAllMedia"));

    let r = delete_request(&[4], &[0], t, Duration::from_secs(60));
    assert_eq!(r.payload.keys().collect::<Vec<_>>(), ["correlationVector", "requestType", "mediaIds", "checksums"]);
    assert_eq!(r.payload.i32("requestType"), Some(0));
    assert_eq!(r.options.timeout, Some(Duration::from_secs(60)));
    assert!(!r.payload.contains_key("contentType"));
    assert_eq!(names(&r), pair("DeleteInvoked", "DeleteMedia"));
}

#[test]
fn metadata_reply_keys_in_wire_order() {
    let vs = metadata_reply("cv", 3.99, &[3, 2], &[3]);
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "contractVersion", "itemCount", "mediaIds", "mediaRequestedIds", "result"]
    );
    assert_eq!(vs.i32("itemCount"), Some(2));
    assert_eq!(vs.f64("contractVersion"), Some(3.99));
}
