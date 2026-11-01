use super::*;

fn contacts_batch(extra: ValueSet) -> ValueSet {
    let mut b = ValueSet::new()
        .with("itemCount", 2i32)
        .with("contactIds", vec![1i64, 2])
        .with("lastUpdatedTimes", vec![1_700_000_000_000i64, 1_700_000_000_001]);
    for (k, v) in extra.iter() {
        b.insert(k, v.clone());
    }
    b
}

fn item(batch_key: &str, batch: ValueSet) -> ValueSet {
    ValueSet::new().with(batch_key, batch)
}

#[test]
fn metadata_defaults_and_types() {
    let m = parse_metadata(&ValueSet::new().with("metadata", true)).unwrap();
    assert!(m.ids.iter().all(Vec::is_empty));
    assert_eq!(m.contact_capabilities, None);
    assert_eq!(m.sequence_numbers, None);

    let m = parse_metadata(
        &ValueSet::new()
            .with("contactIds", vec![5i64, 6])
            .with("contactChecksums", vec![50i64, 60, 70])
            .with("phoneNumberIds", vec![9i64])
            .with("contactSettings", ValueSet::new())
            .with("sequenceNumbers", ValueSet::new().with("contact", 4i64)),
    )
    .unwrap();
    assert_eq!(m.ids[0], [5, 6]);
    assert_eq!(m.checksums[0], [50, 60], "a longer checksum array is cut to the ids");
    assert_eq!(m.checksums[1], [0], "an absent checksum array reads 0");
    assert_eq!(m.contact_capabilities, Some(0), "contactSettings without contactCapabilities is None (0)");
    assert_eq!(m.sequence_numbers.unwrap().0, [4, -1, -1, -1, -1, -1]);

    // Ignored when not an Object.
    let m = parse_metadata(&ValueSet::new().with("contactSettings", 3i32).with("sequenceNumbers", 1i64)).unwrap();
    assert_eq!((m.contact_capabilities, m.sequence_numbers), (None, None));

    // Refused.
    for bad in [
        ValueSet::new().with("contactIds", vec![1i32]),
        ValueSet::new().with("contactIds", vec![1i64, 2]).with("contactChecksums", vec![1i64]),
        ValueSet::new().with("contactUrlChecksums", "x"),
        ValueSet::new().with("contactSettings", ValueSet::new().with("contactCapabilities", 1i64)),
        ValueSet::new().with("sequenceNumbers", ValueSet::new().with("contacturl", 1i32)),
    ] {
        assert!(parse_metadata(&bad).is_err(), "{bad:?}");
    }

    // Checksums without ids are not read.
    assert!(parse_metadata(&ValueSet::new().with("contactChecksums", vec![1i64])).is_ok());
}

#[test]
fn batch_count_rules() {
    // itemCount 0 skips the batch without reading it.
    let i = parse_item(&item("contacts", ValueSet::new().with("itemCount", 0i32))).unwrap();
    assert_eq!(i.contacts, None);

    // A batch that is not an Object is ignored.
    let i = parse_item(&ValueSet::new().with("contacts", 3i32)).unwrap();
    assert_eq!(i.contacts, None);

    // Missing or mistyped itemCount, missing required array.
    assert_eq!(
        parse_item(&item("contacts", ValueSet::new())).unwrap_err().reason,
        "contactsV2 batch without itemCount"
    );
    assert!(parse_item(&item("contacts", ValueSet::new().with("itemCount", 1i64))).is_err());
    assert_eq!(
        parse_item(&item("contacts", ValueSet::new().with("itemCount", 1i32).with("contactIds", vec![1i64])))
            .unwrap_err()
            .reason,
        "contactsV2 batch without a required array"
    );

    // A present optional array shorter than itemCount.
    assert_eq!(
        parse_item(&item("contacts", contacts_batch(ValueSet::new().with("displayNames", vec!["a"]))))
            .unwrap_err()
            .reason,
        "contactsV2 batch with fewer entries than itemCount"
    );

    // Mistyped optional array.
    assert!(parse_item(&item("contacts", contacts_batch(ValueSet::new().with("nicknames", vec![1i32, 2])))).is_err());

    // Every present array longer than itemCount: n is their minimum.
    let b = ValueSet::new()
        .with("itemCount", 1i32)
        .with("phoneNumberIds", vec![1i64, 2])
        .with("contactIds", vec![10i64, 20, 30])
        .with("checksums", vec![0i64, 0])
        .with("phoneNumbers", vec!["a", "b"])
        .with("displayNumbers", vec!["", "B"])
        .with("types", vec![2i32, 3])
        .with("labels", vec!["x", "y"])
        .with("actions", vec![1i32, 0]);
    let i = parse_item(&item("phoneNumbers", b)).unwrap();
    assert_eq!(i.phone_numbers.as_ref().unwrap().processed, [1, 2]);

    // One absent optional array bounds n by itemCount.
    let b = ValueSet::new()
        .with("itemCount", 1i32)
        .with("phoneNumberIds", vec![1i64, 2])
        .with("contactIds", vec![10i64, 20]);
    let i = parse_item(&item("phoneNumbers", b)).unwrap();
    let pn = i.phone_numbers.unwrap();
    assert_eq!(pn.processed, [1]);
    assert_eq!(
        pn.upserts[0],
        PhoneNumberRecord {
            id: 1,
            contact_id: 10,
            checksum: 0,
            number: String::new(),
            display_number: String::new(),
            kind: 0,
            label: String::new()
        }
    );
}

#[test]
fn actions_split_deletes_and_upserts() {
    let b = ValueSet::new()
        .with("itemCount", 4i32)
        .with("contactUrlIds", vec![1i64, 2, 3, 4])
        .with("contactIds", vec![9i64, 9, 9, 9])
        .with("actions", vec![2i32, -1, 0, 7]);

    let urls = parse_item(&item("contactUrls", b)).unwrap().contact_urls.unwrap();
    assert_eq!(urls.deletes, [1]);
    assert_eq!(urls.upserts.iter().map(|u| u.id).collect::<Vec<_>>(), [2, 3, 4]);
    assert_eq!(urls.processed, [1, 2, 3, 4]);
}

#[test]
fn thumbnails_are_sliced_over_the_non_deleted_entries() {
    let b = ValueSet::new()
        .with("itemCount", 4i32)
        .with("contactIds", vec![1i64, 2, 3, 4])
        .with("lastUpdatedTimes", vec![0i64, 0, 0, 0])
        .with("thumbnailSizes", vec![2i64, 5, 0, 3])
        .with("actions", vec![0i32, 2, 0, 1])
        .with("thumbnails", vec![1u8, 2, 3, 4, 5, 9]);

    let c = parse_item(&item("contacts", b)).unwrap().contacts.unwrap();
    let thumbs: Vec<_> = c.upserts.iter().map(|r| (r.id, r.thumbnail.clone())).collect();
    // Entry 2 is deleted: its size takes no bytes; entry 4's bytes follow entry 1's.
    assert_eq!(thumbs, [(1, Some(vec![1, 2])), (3, None), (4, Some(vec![3, 4, 5]))]);
    assert_eq!(c.deletes, [2]);
}

#[test]
fn thumbnail_and_time_violations_are_refused() {
    let too_short =
        contacts_batch(ValueSet::new().with("thumbnailSizes", vec![2i64, 2]).with("thumbnails", vec![1u8, 2, 3]));
    assert_eq!(
        parse_item(&item("contacts", too_short)).unwrap_err().reason,
        "contactsV2 thumbnails shorter than thumbnailSizes"
    );

    let missing = contacts_batch(ValueSet::new().with("thumbnailSizes", vec![0i64, 1]));
    assert!(parse_item(&item("contacts", missing)).is_err(), "a positive size needs the blob");

    let huge = contacts_batch(
        ValueSet::new().with("thumbnailSizes", vec![i64::from(i32::MAX) + 1, 0]).with("thumbnails", vec![0u8]),
    );
    assert_eq!(parse_item(&item("contacts", huge)).unwrap_err().reason, "contactsV2 thumbnail size out of range");

    let mistyped_blob = contacts_batch(ValueSet::new().with("thumbnails", vec!["x"]));
    assert!(parse_item(&item("contacts", mistyped_blob)).is_err());

    let late = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactIds", vec![1i64])
        .with("lastUpdatedTimes", vec![MAX_UNIX_MS + 1]);
    assert_eq!(parse_item(&item("contacts", late)).unwrap_err().reason, "contactsV2 lastUpdatedTimes out of range");

    // A deleted entry's time is not read.
    let deleted = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactIds", vec![1i64])
        .with("lastUpdatedTimes", vec![i64::MAX])
        .with("actions", vec![2i32]);
    assert!(parse_item(&item("contacts", deleted)).is_ok());

    let early = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactIds", vec![1i64])
        .with("lastUpdatedTimes", vec![MIN_UNIX_MS]);
    assert!(parse_item(&item("contacts", early)).is_ok());
}

#[test]
fn negative_item_count_with_an_absent_array_processes_nothing() {
    let b = ValueSet::new().with("itemCount", -3i32).with("contactDateIds", vec![1i64]).with("contactIds", vec![2i64]);
    let d = parse_item(&item("contactDates", b)).unwrap().contact_dates.unwrap();
    assert!(d.processed.is_empty());
}

#[test]
fn item_sequence_numbers() {
    let i = parse_item(&ValueSet::new().with("sequenceNumbers", ValueSet::new().with("phonenumber", 8i64))).unwrap();
    assert_eq!(i.sequence_numbers.unwrap().get(RecordType::PhoneNumber), 8);
    assert!(!i.has_entries());
    assert!(parse_item(&ValueSet::new().with("sequenceNumbers", ValueSet::new().with("contact", "1"))).is_err());
}
