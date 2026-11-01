use super::*;

const RECENT_FLAGS: u32 = 0x1 | 0x2 | 0x800;

fn metadata(extra: ValueSet) -> ValueSet {
    let mut vs = ValueSet::new().with("correlationVector", "cv").with("metadata", 1i32);
    for (k, v) in extra.iter() {
        vs.insert(k, v.clone());
    }
    vs
}

#[test]
fn metadata_checksums_must_cover_the_ids() {
    let m = parse_metadata(
        &metadata(ValueSet::new().with("phoneAppsIds", vec![1i64, 2]).with("checksums", vec![5i64, 6, 7])),
        0,
    )
    .unwrap();
    assert_eq!(m.apps, Some(IdList { ids: vec![1, 2], checksums: vec![5, 6] }));

    assert_eq!(
        parse_metadata(&metadata(ValueSet::new().with("phoneAppsIds", vec![1i64, 2]).with("checksums", vec![5i64])), 0),
        Err(Malformed::reason("checksums missing or shorter than phoneAppsIds"))
    );

    // An empty list needs no checksums; a mistyped id list is refused.
    assert!(parse_metadata(&metadata(ValueSet::new().with("phoneAppsIds", Vec::<i64>::new())), 0).is_ok());
    assert_eq!(
        parse_metadata(&metadata(ValueSet::new().with("phoneAppsIds", vec![1i32])), 0),
        Err(Malformed::reason("phoneAppsIds is not an Int64Array"))
    );
    assert_eq!(
        parse_metadata(&ValueSet::new().with("metadata", 1i32), 0),
        Err(Malformed::reason("metadata without a correlationVector"))
    );
}

#[test]
fn metadata_reads_the_recent_part_only_once_the_settings_allow_it() {
    let recent = ValueSet::new()
        .with("recentAppsIds", vec![9i64])
        .with("recentAppsChecksums", vec![90i64])
        .with("recentAppsSequenceNumber", 70i64);
    let m = parse_metadata(&metadata(recent.clone()), 0).unwrap();
    assert!(!m.recent_supported && m.recent.is_none() && m.recent_sequence.is_none());

    let with_settings =
        recent.with("phoneAppsSettings", ValueSet::new().with("phoneAppsCapabilities", RECENT_FLAGS as i32));
    let m = parse_metadata(&metadata(with_settings), 0).unwrap();
    assert_eq!(m.settings, Some(RECENT_FLAGS));
    assert!(m.recent_supported);
    assert_eq!(m.recent_sequence, Some(70));

    // Settings without an Int32 capability report 0, which turns recent apps off.
    let off = metadata(ValueSet::new().with("phoneAppsSettings", ValueSet::new().with("phoneAppsCapabilities", "x")));
    let m = parse_metadata(&off, RECENT_FLAGS).unwrap();
    assert_eq!(m.settings, Some(0));
    assert!(!m.recent_supported);
}

#[test]
fn metadata_sequence_numbers_of_another_type_are_ignored() {
    let m = parse_metadata(
        &metadata(ValueSet::new().with("phoneAppsIds", Vec::<i64>::new()).with("sequenceNumber", 4i32)),
        0,
    )
    .unwrap();
    assert_eq!(m.apps_sequence, None);

    // Without `phoneAppsIds` the sequence number is not read.
    let m = parse_metadata(&metadata(ValueSet::new().with("sequenceNumber", 4i64)), 0).unwrap();
    assert_eq!(m.apps_sequence, None);
}

fn items(n: usize) -> ValueSet {
    let s = |p: &str| (0..n).map(|i| format!("{p}{i}")).collect::<Vec<_>>();
    ValueSet::new()
        .with("itemCount", n as i32)
        .with("phoneAppsIds", (0..n as i64).collect::<Vec<_>>())
        .with("appPackageNames", s("pkg"))
        .with("appNames", s("App "))
        .with("appVersions", s("1."))
        .with("phoneAppsLastUpdatedTimes", vec![7i64; n])
        .with("checksums", vec![3i64; n])
        .with("iconSizes", vec![0i64; n])
        .with("content", Vec::<u8>::new())
}

#[test]
fn icons_are_sliced_from_content_in_order() {
    let vs = items(3).with("iconSizes", vec![2i64, 0, 3]).with("content", vec![1u8, 2, 3, 4, 5, 6]);
    let b = parse_items(&vs, 0).unwrap().apps.unwrap();
    let icons: Vec<_> = b.items.iter().map(|i| i.icon.clone()).collect();
    assert_eq!(icons, [Some(vec![1, 2]), None, Some(vec![3, 4, 5])]);
    assert!(!b.with_actions);

    assert_eq!(
        parse_items(&items(1).with("iconSizes", vec![2i64]).with("content", vec![1u8]), 0),
        Err(Malformed::reason("icon sizes exceed content"))
    );
    assert_eq!(
        parse_items(&items(2).with("iconSizes", vec![-1i64, 1]).with("content", vec![1u8]), 0),
        Err(Malformed::reason("negative icon size"))
    );
    assert_eq!(
        parse_items(&items(1).with("content", ValueSet::new()), 0),
        Err(Malformed::reason("content missing or not a UInt8Array"))
    );
}

#[test]
fn item_arrays_must_be_parallel() {
    assert_eq!(
        parse_items(&items(2).with("appNames", vec!["a"]), 0),
        Err(Malformed::reason("appNames missing or not parallel"))
    );

    let mut vs = items(2);
    vs.remove("appVersions");
    assert_eq!(parse_items(&vs, 0), Err(Malformed::reason("appVersions missing or not parallel")));
    assert_eq!(parse_items(&items(2).with("actions", vec![2i32]), 0), Err(Malformed::reason("actions not parallel")));

    // Empty or mistyped actions read as none.
    assert!(!parse_items(&items(2).with("actions", Vec::<i32>::new()), 0).unwrap().apps.unwrap().with_actions);
    assert!(!parse_items(&items(2).with("actions", vec![2i64, 2]), 0).unwrap().apps.unwrap().with_actions);
    let b = parse_items(&items(2).with("actions", vec![0i32, 2]), 0).unwrap().apps.unwrap();
    assert!(b.with_actions);
    assert_eq!(b.items.iter().map(|i| i.delete).collect::<Vec<_>>(), [false, true]);

    assert_eq!(
        parse_items(&items(1).with("sequenceNumber", 3i32), 0),
        Err(Malformed::reason("sequenceNumber is not an Int64"))
    );
    assert_eq!(parse_items(&items(1).with("sequenceNumber", 3i64), 0).unwrap().apps.unwrap().sequence, Some(3));

    // No `phoneAppsIds`: no phone-app part, whatever else is there.
    assert_eq!(parse_items(&ValueSet::new().with("sequenceNumber", 3i32), 0).unwrap().apps, None);
}

fn recent(n: usize) -> ValueSet {
    let s = |p: &str| (0..n).map(|i| format!("{p}{i}")).collect::<Vec<_>>();
    ValueSet::new()
        .with("recentAppsItemCount", n as i32)
        .with("recentAppsIds", (100..100 + n as i64).collect::<Vec<_>>())
        .with("recentAppsPackageNames", s("pkg"))
        .with("recentAppsChecksums", vec![1i64; n])
        .with("recentAppsDescriptions", vec![String::new(); n])
        .with("recentAppsIntentActions", vec![MAIN_ACTION.to_string(); n])
        .with("recentAppsTaskIds", vec![42i64; n])
        .with("recentAppsRanks", (0..n as i32).collect::<Vec<_>>())
}

#[test]
fn recent_entries_are_read_only_with_recent_apps_supported() {
    assert_eq!(parse_items(&recent(1), 0).unwrap().recent, None);

    let r = parse_items(&recent(2).with("recentAppsWebDocumentUris", vec!["", "https://x"]), RECENT_FLAGS)
        .unwrap()
        .recent
        .unwrap();
    assert_eq!(r.entries[0].web_uri, None);
    assert_eq!(r.entries[1].web_uri.as_deref(), Some("https://x"));

    assert_eq!(
        parse_items(&recent(2).with("recentAppsWindowsDocumentUris", vec!["a"]), RECENT_FLAGS),
        Err(Malformed::reason("recentAppsWindowsDocumentUris not parallel"))
    );
    assert_eq!(
        parse_items(&recent(2).with("recentAppsRanks", vec![1i64, 2]), RECENT_FLAGS),
        Err(Malformed::reason("recentAppsRanks missing or not parallel"))
    );
    assert_eq!(
        parse_items(&recent(1).with("recentAppsSequenceNumber", 1i32), RECENT_FLAGS),
        Err(Malformed::reason("recentAppsSequenceNumber is not an Int64"))
    );
}

#[test]
fn the_batch_sizes_are_required_int32s() {
    let mut vs = items(1);
    vs.remove("itemCount");
    assert_eq!(parse_items(&vs, 0), Err(Malformed::reason("itemCount missing or not an Int32")));
    assert_eq!(
        parse_items(&items(1).with("itemCount", 1i64), 0),
        Err(Malformed::reason("itemCount missing or not an Int32"))
    );

    let mut vs = recent(1);
    vs.remove("recentAppsItemCount");
    assert_eq!(parse_items(&vs, RECENT_FLAGS), Err(Malformed::reason("recentAppsItemCount missing or not an Int32")));

    // Not read while recent apps are not supported.
    assert_eq!(parse_items(&vs, 0).unwrap().recent, None);

    // The count is not checked against the ids.
    assert_eq!(parse_items(&items(2).with("itemCount", 7i32), 0).unwrap().apps.unwrap().items.len(), 2);
}

#[test]
fn desktop_uris_follow_the_http_and_office_rules() {
    assert_eq!(desktop_uri_candidate("com.a", "ms-word:ofe|u|x", false).as_deref(), Some("ms-word:ofe|u|x"));
    assert_eq!(desktop_uri_candidate("com.a", "HTTPS://host/doc.docx", true), None, "not the Office app");
    assert_eq!(desktop_uri_candidate(OFFICE_PACKAGE, "https://host/doc.docx", false), None, "rewrite off");
    assert_eq!(
        desktop_uri_candidate("COM.MICROSOFT.OFFICE.OFFICEHUBROW", "https://h/a.DOCX", true).as_deref(),
        Some("ms-word:ofv|u|https://h/a.DOCX")
    );
    assert_eq!(
        desktop_uri_candidate(OFFICE_PACKAGE, "http://h/a.xlsx", true).as_deref(),
        Some("ms-excel:ofv|u|http://h/a.xlsx")
    );
    assert_eq!(
        desktop_uri_candidate(OFFICE_PACKAGE, "https://h/a.pptx", true).as_deref(),
        Some("ms-powerpoint:ofv|u|https://h/a.pptx")
    );
    assert_eq!(desktop_uri_candidate(OFFICE_PACKAGE, "https://h/a.pdf", true), None);
    assert_eq!(desktop_uri_candidate("com.a", "ht", false).as_deref(), Some("ht"));
}

#[test]
fn an_absolute_uri_starts_with_a_scheme() {
    for uri in ["ms-word:ofe|u|x", "file:///a", "a:", "x+y-z.1:rest", "MS-EXCEL:ofv|u|h"] {
        assert!(is_absolute_uri(uri), "{uri}");
    }
    for uri in ["ht", "docs/report.docx", ":x", "1a:x", "a b:x", "/a:b", "\\\\host\\share"] {
        assert!(!is_absolute_uri(uri), "{uri}");
    }
}

#[test]
fn the_metadata_reply_names_the_requested_ids() {
    let r = metadata_reply("cv", 3.99, &[1002], Some(&[2001]));
    assert_eq!(
        r.keys().collect::<Vec<_>>(),
        ["correlationVector", "contentType", "contractVersion", "itemCount", "phoneAppsIds", "result", "recentAppsIds"]
    );
    assert_eq!(r.i32("itemCount"), Some(2));

    let r = metadata_reply("cv", 3.99, &[], None);
    assert_eq!(r.i64_array("phoneAppsIds"), Some(&[][..]));
    assert!(!r.contains_key("recentAppsIds"));

    assert_eq!(rejection_answer("bad").str("message"), Some("bad 80004005"));
}
