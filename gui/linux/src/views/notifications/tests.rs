use super::*;
use phonelink_engine::api::notifications::NotificationButton;

fn notification(key: &str, package: &str, app_name: Option<&str>) -> PhoneNotification {
    PhoneNotification {
        key: key.to_owned(),
        package_name: package.to_owned(),
        app_name: app_name.map(str::to_owned),
        title: Some("Title".to_owned()),
        subtitle: None,
        body: String::new(),
        conversation: Vec::new(),
        time_ms: 1_700_000_000_000,
        pinned: false,
        open_target: NotificationOpenTarget::Notifications,
        launch_supported: true,
        is_read_only_group_message: false,
        reply: None,
        actions: Vec::new(),
    }
}

fn state() -> NotificationsState {
    NotificationsState {
        capable: true,
        app_list_supported: true,
        pc_permission: true,
        device_permission: true,
        sensitive_permission: false,
        sensitive_prompt_offered: false,
        syncing: false,
        last_updated_ms: None,
        muted_all_until_ms: None,
        throttling_enabled: true,
        low_ram_device: false,
        setup_completed: true,
        post_notification_needed: false,
    }
}

fn sender(name: &str) -> NotificationLine {
    NotificationLine::Sender { name: name.to_owned() }
}

fn message(text: &str) -> NotificationLine {
    NotificationLine::Message { text: text.to_owned() }
}

#[test]
fn content_follows_the_setup_order() {
    assert_eq!(Content::of(None, false, true), Content::Loading);
    assert_eq!(Content::of(Some(&state()), true, false), Content::Failed);
    assert_eq!(
        Content::of(Some(&NotificationsState { capable: false, ..state() }), false, false),
        Content::Unavailable
    );
    let post = NotificationsState { post_notification_needed: true, pc_permission: false, ..state() };
    assert_eq!(Content::of(Some(&post), false, false), Content::NeedsPostNotificationPermission);
    let off = NotificationsState { pc_permission: false, ..state() };
    assert_eq!(Content::of(Some(&off), false, false), Content::TurnedOff);
    // This computer's switch is asked about only once the phone has given access (or cannot).
    let new_user = NotificationsState { pc_permission: false, device_permission: false, ..state() };
    assert_eq!(Content::of(Some(&new_user), false, false), Content::NeedsAccess);
    let low_ram = NotificationsState { device_permission: false, low_ram_device: true, ..state() };
    assert_eq!(Content::of(Some(&low_ram), false, false), Content::LowMemoryPhone);
    let low_ram_off = NotificationsState { pc_permission: false, ..low_ram };
    assert_eq!(Content::of(Some(&low_ram_off), false, false), Content::TurnedOff);
    assert_eq!(Content::of(Some(&NotificationsState { syncing: true, ..state() }), false, true), Content::Syncing);
    assert_eq!(Content::of(Some(&state()), false, true), Content::Empty);
    assert_eq!(Content::of(Some(&state()), false, false), Content::List);
}

#[test]
fn conversation_groups_messages_under_their_sender() {
    let lines = [message("first"), sender("Mina"), message("hi"), message("again"), sender("Jo"), sender("Al")];
    assert_eq!(
        conversation_runs(&lines),
        [
            json!({ "sender": null, "messages": ["first"] }),
            json!({ "sender": "Mina", "messages": ["hi", "again"] }),
            json!({ "sender": "Jo", "messages": [] }),
            json!({ "sender": "Al", "messages": [] }),
        ]
    );
}

#[test]
fn a_row_previews_the_body_or_the_last_message() {
    let mut item = notification("k", "com.example", None);
    item.conversation = vec![sender("Mina"), message("hi"), message("see you"), sender("Jo")];
    assert_eq!(preview(&item), "see you");
    item.body = "Body".to_owned();
    assert_eq!(preview(&item), "Body");
}

#[test]
fn items_carry_their_app_settings() {
    let mut item = notification("k", "com.example", Some(""));
    item.reply = Some(NotificationButton { name: "Reply".to_owned(), inline_reply: true, index: 2, position: 1 });
    let muted = AppNotificationSetting {
        package_name: "com.example".to_owned(),
        flags: AppNotificationFlags { feed: false, banner: false, other_bits: 0 },
        muted_until_ms: Some(42),
    };
    let json: Value = serde_json::from_str(&notifications_json(
        std::slice::from_ref(&item),
        &HashMap::from([("com.example".to_owned(), muted)]),
    ))
    .expect("the list is JSON");
    assert_eq!(json[0]["app"], "com.example");
    assert_eq!(json[0]["replyIndex"], 2);
    assert_eq!(json[0]["appOn"], false);
    assert_eq!(json[0]["appMutedUntilMs"], 42);

    let unread: Value = serde_json::from_str(&notifications_json(&[item], &HashMap::new())).expect("the list is JSON");
    assert_eq!(unread[0]["appOn"], true);
    assert_eq!(unread[0]["appMutedUntilMs"], Value::Null);
}

#[test]
fn apps_are_named_after_their_newest_named_notification() {
    let mut known = BTreeMap::from([("com.old".to_owned(), "Old".to_owned())]);
    let list = [
        notification("1", "com.chat", Some(" Chat ")),
        notification("2", "com.chat", Some("Chat (old name)")),
        notification("3", "com.bare", None),
        notification("4", "com.old", Some("")),
    ];
    assert!(remember_apps(&mut known, &list));
    assert_eq!(
        known,
        BTreeMap::from([
            ("com.bare".to_owned(), "com.bare".to_owned()),
            ("com.chat".to_owned(), "Chat".to_owned()),
            ("com.old".to_owned(), "Old".to_owned()),
        ])
    );
    assert!(!remember_apps(&mut known, &list));
}

#[test]
fn names_seen_in_this_run_win_over_stored_ones() {
    let known = BTreeMap::from([("phone".to_owned(), BTreeMap::from([("com.a".to_owned(), "A new".to_owned())]))]);
    let stored = BTreeMap::from([
        (
            "phone".to_owned(),
            BTreeMap::from([("com.a".to_owned(), "A".to_owned()), ("com.b".to_owned(), "B".to_owned())]),
        ),
        ("other".to_owned(), BTreeMap::from([("com.c".to_owned(), "C".to_owned())])),
    ]);
    let merged = merge_known_apps(&known, stored);
    assert_eq!(merged["phone"]["com.a"], "A new");
    assert_eq!(merged["phone"]["com.b"], "B");
    assert_eq!(merged["other"]["com.c"], "C");
}

#[test]
fn choices_keep_the_bits_the_phone_reported() {
    let flags = |feed, banner| AppNotificationFlags { feed, banner, other_bits: 8 };
    assert_eq!(AppChoice::of(flags(true, true)), AppChoice::BannersAndList);
    assert_eq!(AppChoice::of(flags(true, false)), AppChoice::ListOnly);
    assert_eq!(AppChoice::of(flags(false, true)), AppChoice::Off);
    assert_eq!(AppChoice::ListOnly.flags(flags(true, true)), flags(true, false));
    assert_eq!(AppChoice::Off.flags(flags(true, true)), flags(false, false));
    for choice in [AppChoice::BannersAndList, AppChoice::ListOnly, AppChoice::Off] {
        assert_eq!(AppChoice::parse(choice.name()), Some(choice));
    }
    assert_eq!(AppChoice::parse("banners"), None);
}

#[test]
fn app_rows_sort_by_name_then_package() {
    let row = |package: &str, name: &str| AppRow {
        package: package.to_owned(),
        name: name.to_owned(),
        flags: AppNotificationFlags { feed: true, banner: false, other_bits: 0 },
        muted_until_ms: None,
        pending: false,
    };
    let json = app_rows_json(vec![row("com.z", "beta"), row("com.b", "Alpha"), row("com.a", "alpha")]);
    let packages: Vec<&str> = json.as_array().into_iter().flatten().filter_map(|r| r["package"].as_str()).collect();
    assert_eq!(packages, ["com.a", "com.b", "com.z"]);
    assert_eq!(json[0]["choice"], "listOnly");
}

#[test]
fn the_help_link_carries_a_version_4_trace_id() {
    let link = post_notification_link([0xff; 16]);
    assert_eq!(link, "https://aka.ms/notification?src=1&traceId=ffffffff-ffff-4fff-bfff-ffffffffffff");
}
