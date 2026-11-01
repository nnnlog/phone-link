use super::*;

fn row(group: &str, t: i32, value: &str) -> SettingRow {
    SettingRow {
        group_id: group.into(),
        key: "com.a".into(),
        setting_type: t,
        value: value.into(),
        mute_expiration_ticks: 0,
    }
}

#[test]
fn flags_text_is_enum_try_parse() {
    assert_eq!(parse_flags_text("3"), Some(AppFlags::ALL));
    assert_eq!(parse_flags_text(" 2 "), Some(AppFlags::BANNER));
    assert_eq!(parse_flags_text("AllNotifications"), Some(AppFlags::ALL));
    assert_eq!(parse_flags_text("ShowNotificationsInFeed, ShowNotificationsInBanner"), Some(AppFlags::ALL));
    assert_eq!(parse_flags_text("allnotifications"), None, "names are case-sensitive");
    assert_eq!(parse_flags_text("-1"), None);
    assert_eq!(parse_flags_text(""), None);
}

#[test]
fn rows_read_and_combine() {
    assert_eq!(read_row(None), AppFlags::ALL);
    assert_eq!(read_row(Some(&row(setting::SYNC_GROUP, 11, "1"))), AppFlags::FEED);
    assert_eq!(read_row(Some(&row(setting::SYNC_GROUP, 11, "0"))), AppFlags::NONE);
    assert_eq!(read_row(Some(&row(setting::LOCAL_GROUP, 5, "2"))), AppFlags::BANNER);
    assert_eq!(read_row(Some(&row(setting::LOCAL_GROUP, 5, "junk"))), AppFlags::ALL);
    assert_eq!(read_row(Some(&row(setting::LOCAL_GROUP, 7, "0"))), AppFlags::ALL);

    let sync_off = row(setting::SYNC_GROUP, 11, "0");
    let local_banner_off = row(setting::LOCAL_GROUP, 5, "0");
    // No local row: All (whatever the sync row says), unless quiet-by-default.
    assert_eq!(effective_flags(Some(&sync_off), None, false, AppFlags::ALL), AppFlags::ALL);
    assert_eq!(effective_flags(None, Some(&local_banner_off), false, AppFlags::ALL), AppFlags::FEED);
    assert_eq!(effective_flags(Some(&sync_off), Some(&local_banner_off), false, AppFlags::ALL), AppFlags::NONE);
    // Quiet by default: the banner bit from the default preference, only with the feed bit.
    assert_eq!(effective_flags(None, None, true, AppFlags::FEED), AppFlags::FEED);
    assert_eq!(effective_flags(None, None, true, AppFlags::ALL), AppFlags::ALL);
    assert_eq!(effective_flags(Some(&sync_off), None, true, AppFlags::ALL), AppFlags::NONE);
}

#[test]
fn request_bodies() {
    let item = SyncSettingItem::new("com.a", AppFlags::ALL);
    let vs = setting_changed_body(&item, "0123456789abcdef");
    assert_eq!(
        vs.to_json(),
        serde_json::json!({
            "settingKey": {"String": "com.a"},
            "settingValue": {"String": "1"},
            "settingType": {"Int32": 11},
            "settingGroupId": {"String": "NotificationSyncSetting"},
            "settingMuteExpirationTimestamp": {"Int64": 0},
            "triggerType": {"Int32": 26},
            "contentType": {"String": "settings"},
            "correlationVector": {"String": "0123456789abcdef"}
        })
    );

    let vs = batch_body(&[item, SyncSettingItem::new("com.b", AppFlags::BANNER)], "cv");
    assert_eq!(
        vs.str("settings"),
        Some(
            r#"{"settings":[{"settingKey":"com.a","settingValue":"1","settingType":11,"settingGroupId":"NotificationSyncSetting","settingMuteExpirationTimestamp":0},{"settingKey":"com.b","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting","settingMuteExpirationTimestamp":0}]}"#
        )
    );
    assert_eq!(vs.i32("triggerType"), Some(27));

    let vs = sync_all_body("cv");
    assert_eq!(vs.keys().collect::<Vec<_>>(), ["triggerType", "contentType", "correlationVector"]);
}

#[test]
fn responses() {
    let obj = r#"{"settingKey":"com.a","settingValue":"0","settingType":11,"settingGroupId":"NotificationSyncSetting","settingMuteExpirationTimestamp":5}"#;
    let rows = parse_response(&ValueSet::new().with("savedSetting", obj).with("syncSetting", "{}"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].mute_expiration_ticks, 5);

    // savedSetting that is not a JSON object: syncSetting is read instead.
    let sync = r#"{"settings":[{"settingKey":"a","settingValue":"1","settingType":11,"settingGroupId":"g"},
            {"settingKey":"b","settingValue":"1","settingGroupId":"g"},
            {"settingKey":"c","settingValue":"1","settingType":"11","settingGroupId":"g"},
            {"settingKey":"d","settingValue":"1","settingType":11,"settingGroupId":"g"}]}"#;
    let rows = parse_response(&ValueSet::new().with("savedSetting", "nope").with("syncSetting", sync));
    assert_eq!(rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["a"], "b skipped, c ends the parse");

    assert!(parse_response(&ValueSet::new().with("syncSetting", "[1]")).is_empty());
    assert!(parse_response(&ValueSet::new().with("syncSetting", r#"{"x":1}"#)).is_empty());
    assert!(parse_response(&ValueSet::new()).is_empty());
}
