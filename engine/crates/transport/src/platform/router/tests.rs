use super::*;

#[test]
fn rejection_table() {
    use PlatformRequestStatus::*;
    // (`_rejectedReason`, the status it maps to, why).
    let cases: &[(&str, Option<PlatformRequestStatus>, &str)] = &[
        ("None", None, ""),
        ("Unspecified", Some(NoResponse), ""),
        ("NotAllowed", Some(RemoteSyncNotAllowed), ""),
        ("VersionTooLow", Some(RemoteVersionTooLow), ""),
        ("VersionTooHigh", Some(RemoteVersionTooHigh), ""),
        ("AllSyncDisabled", Some(RemoteAllSyncDisabled), ""),
        ("MeteredSyncDisabled", Some(RemoteMeteredSyncDisabled), ""),
        ("NotSubscribedToRoute", Some(RemoteNotSubscribedToRoute), ""),
        // Names that do not parse: treated as VersionTooHigh.
        ("SomethingNew", Some(RemoteVersionTooHigh), ""),
        ("notsubscribedtoroute", Some(RemoteVersionTooHigh), "case-sensitive"),
        ("", Some(RemoteVersionTooHigh), ""),
        ("   ", Some(RemoteVersionTooHigh), ""),
        ("NotAllowed,", Some(RemoteVersionTooHigh), "a trailing separator"),
        // Numbers parse to their value, defined or not.
        ("7", Some(RemoteNotSubscribedToRoute), ""),
        (" +07 \t", Some(RemoteNotSubscribedToRoute), ""),
        ("1", None, "1 is None: not a rejection"),
        ("0", Some(NoResponse), ""),
        ("99", Some(NoResponse), ""),
        ("-1", Some(NoResponse), ""),
        ("7\0\0", Some(RemoteNotSubscribedToRoute), "trailing NULs"),
        ("2147483648", Some(RemoteVersionTooHigh), "overflow"),
        ("-2147483648", Some(NoResponse), ""),
        ("7x", Some(RemoteVersionTooHigh), ""),
        ("- 7", Some(RemoteVersionTooHigh), ""),
        // Names are trimmed and OR-ed.
        (" VersionTooHigh ", Some(RemoteVersionTooHigh), ""),
        ("NotAllowed, VersionTooLow", Some(RemoteVersionTooLow), "2 | 3 = 3"),
        ("NotAllowed,VersionTooHigh", Some(RemoteMeteredSyncDisabled), "2 | 4 = 6"),
        ("Unspecified,None", None, "0 | 1 = None"),
        ("7,None", Some(RemoteVersionTooHigh), ""),
    ];

    for (reason, expected, why) in cases {
        assert_eq!(rejection_status(reason), *expected, "{reason:?} {why}");
    }
}

#[test]
fn header_orders() {
    let h = HeaderMap::from_pairs([("_route", "/x"), ("ms-content-type", "text/json")]).unwrap();
    let r = request_id_first(&h, "7");
    assert_eq!(
        r.iter().collect::<Vec<_>>(),
        vec![("_requestId", "7"), ("_route", "/x"), ("ms-content-type", "text/json")]
    );

    let own = HeaderMap::from_pairs([("result", "0"), ("_route", "/ignored"), ("_rejectionVersion", "9")]).unwrap();
    let resp = response_headers("abc", &own);
    assert_eq!(
        resp.iter().collect::<Vec<_>>(),
        vec![
            ("_originalRequestId", "abc"),
            ("_route", "/internal/response"),
            ("result", "0"),
            ("_rejectionVersion", "1")
        ]
    );
}
