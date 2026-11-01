use super::*;
use serde_json::json;

fn item(v: serde_json::Value) -> Result<NotificationItem, ItemError> {
    NotificationItem::parse(&v.to_string())
}

fn base() -> serde_json::Value {
    json!({"key": "0|com.example|1|null|10", "packageName": "com.example", "postTime": 1_760_781_600_000i64})
}

fn with(extra: serde_json::Value) -> NotificationItem {
    let mut v = base();
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    item(v).unwrap()
}

#[test]
fn identity_rules() {
    let i = item(base()).unwrap();
    assert_eq!(i.key, "0|com.example|1|null|10");
    assert_eq!(i.package_name, "com.example");
    assert_eq!(i.post_time, 1_760_781_600_000);

    assert_eq!(NotificationItem::parse("not json"), Err(ItemError::NotJson));
    assert_eq!(NotificationItem::parse("[1]"), Err(ItemError::NotAnObject));

    for missing in ["key", "packageName", "postTime"] {
        let mut v = base();
        v.as_object_mut().unwrap().remove(missing);
        assert_eq!(item(v), Err(ItemError::MissingOrMistyped(missing)), "{missing}");
    }

    let mut v = base();
    v["key"] = json!(5);
    assert_eq!(item(v), Err(ItemError::MissingOrMistyped("key")));

    let mut v = base();
    v["postTime"] = json!(1.5);
    assert_eq!(item(v), Err(ItemError::MissingOrMistyped("postTime")));

    let mut v = base();
    v["postTime"] = json!(MAX_UNIX_MS + 1);
    assert_eq!(item(v), Err(ItemError::PostTimeOutOfRange));
}

#[test]
fn keys_read_while_stored() {
    for k in ["text", "title", "template"] {
        let mut v = base();
        v[k] = serde_json::Value::Null;
        assert_eq!(item(v), Err(ItemError::NullWhileStored(k)), "{k}");
    }

    let mut v = base();
    v["template"] = json!("a$BigTextStyle");
    v["bigText"] = serde_json::Value::Null;
    assert_eq!(item(v.clone()), Err(ItemError::NullWhileStored("bigText")));
    v["template"] = json!("a$BigTextStyle$InboxStyle");
    assert!(item(v).is_ok(), "bigText is not read under an Inbox template");

    let mut v = base();
    v["publisherWebsite"] = json!(["x"]);
    assert_eq!(item(v.clone()), Err(ItemError::PublisherWebsiteNotAString));
    v["publisherWebsite"] = json!("https://example.com");
    assert!(item(v).is_ok());

    // A key read only later reads as absent.
    let i = with(json!({"title": 5, "notificationClass": "2", "importance": 4.5}));
    assert_eq!((i.title(), i.is_media_style(), i.banner_importance()), (None, false, 3));
}

#[test]
fn platform_selects_the_parser_only_with_version() {
    let mut v = base();
    v["platform"] = json!(1);
    assert!(item(v.clone()).is_ok(), "platform without version: Android");
    v["version"] = json!(2);
    assert_eq!(item(v.clone()), Err(ItemError::NotAndroid));
    v["platform"] = json!(0);
    assert!(item(v.clone()).is_ok());
    v["platform"] = json!(7);
    assert_eq!(item(v.clone()), Err(ItemError::NotAndroid));
    v["platform"] = json!("0");
    assert_eq!(item(v), Err(ItemError::NotAndroid));
}

#[test]
fn classification_and_flags() {
    let i = item(base()).unwrap();
    assert_eq!(i.banner_importance(), 3);
    assert!(!i.only_alert_once() && !i.is_media_style() && !i.is_messaging());

    let i = with(json!({"importance": 4, "flags": 8 | 2, "notificationClass": 2, "featureFlags": 2}));
    assert_eq!(i.banner_importance(), 4);
    assert!(i.only_alert_once() && i.is_media_style() && i.launch_supported());

    let i = with(json!({"importance": "4", "notificationClass": 1}));
    assert_eq!(i.banner_importance(), 3, "a mistyped importance reads as absent");
    assert!(i.is_messaging());

    assert_eq!(with(json!({"timestamp": 0})).event_time_ms(), 1_760_781_600_000);
    assert_eq!(with(json!({"timestamp": 1_760_781_700_000i64})).event_time_ms(), 1_760_781_700_000);
}

#[test]
fn multi_line_content_and_dedupe_description() {
    assert!(
        with(json!({"template": "android.app.Notification$InboxStyle", "textLines": ["a"]})).has_multi_line_content()
    );
    assert!(!with(json!({"template": "x$InboxStyle", "textLines": []})).has_multi_line_content());
    assert!(with(json!({"template": "x$MessagingStyle", "messages": ["m"]})).has_multi_line_content());
    assert!(!with(json!({"textLines": ["a"]})).has_multi_line_content(), "no template");
    assert_eq!(with(json!({"template": "x$BigTextStyle", "text": "t", "bigText": "b"})).dedupe_description(), "b");
    assert_eq!(with(json!({"template": "x$BigTextStyle", "text": "t"})).dedupe_description(), "");
    assert_eq!(with(json!({"text": "t"})).dedupe_description(), "t");
    assert_eq!(item(base()).unwrap().dedupe_description(), "");
}

#[test]
fn actions_for_list_and_banner() {
    let i = with(json!({"notificationActions": [
        {"actionName": "Reply", "isActionInlineReply": true, "actionIndex": 0},
        {"actionName": "Mark read", "isActionInlineReply": false, "actionIndex": 1},
        {"actionName": "No index", "isActionInlineReply": false},
        {"actionName": "Reply all", "isActionInlineReply": true, "actionIndex": 2},
        {"actionName": "Late", "isActionInlineReply": true, "actionIndex": 5},
        {"actionName": "B", "isActionInlineReply": false, "actionIndex": 6},
        {"actionName": "C", "isActionInlineReply": false, "actionIndex": 7},
        {"actionName": "D", "isActionInlineReply": false, "actionIndex": 8},
        {"actionName": "E", "isActionInlineReply": false, "actionIndex": 9},
        {"actionName": "Neg", "isActionInlineReply": true, "actionIndex": -1},
        {"actionName": "Answer", "isActionInlineReply": true, "actionIndex": 0},
    ]}));

    assert_eq!(i.list_reply().unwrap().name, "Reply");
    let names: Vec<_> = i.list_actions().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Mark read", "B", "C", "D"]);

    // One banner action per reply entry with `actionIndex < 3`, even when two share an index.
    let banner: Vec<_> = i.banner_reply_actions().into_iter().map(|a| (a.index, a.position, a.name)).collect();
    assert_eq!(banner, [(0, 0, "Reply".to_string()), (2, 3, "Reply all".to_string()), (0, 10, "Answer".to_string())]);
}

#[test]
fn display_templates() {
    let d = with(json!({"title": "T", "text": "x", "template": "a$BigTextStyle", "bigText": "big", "bigTitle": "BT"}))
        .display();
    assert_eq!((d.title.as_deref(), d.body.as_str()), (Some("BT"), "big"));

    let d = with(json!({"text": "x", "template": "a$InboxStyle", "textLines": ["l1", "l2"]})).display();
    assert_eq!(d.body, "l1\nl2");

    // MessagingStyle runs; the title equals a sender heading: the first heading replaces it.
    let d = with(json!({"title": "Bob", "text": "x", "template": "a$MessagingStyle",
            "senderNames": ["Ann", "Ann", "Bob"], "messages": ["1", "2", "3"]}))
    .display();
    assert_eq!(d.title.as_deref(), Some("Ann"));
    assert_eq!(
        d.conversation,
        [
            ConversationLine::Message("1".into()),
            ConversationLine::Message("2".into()),
            ConversationLine::Sender("Bob".into()),
            ConversationLine::Message("3".into())
        ]
    );

    // A blank sender: the messages alone. Unequal lengths: the textLines fallback.
    let d = with(json!({"template": "$MessagingStyle", "senderNames": ["A", " "], "messages": ["1", "2"]})).display();
    assert_eq!(d.conversation, [ConversationLine::Message("1".into()), ConversationLine::Message("2".into())]);

    let d = with(json!({"text": "x", "template": "$MessagingStyle", "senderNames": ["A"], "messages": ["1", "2"],
            "textLines": ["t"]}))
    .display();
    assert!(d.conversation.is_empty());
    assert_eq!(d.body, "t");

    // Cut to the last 7.
    let many: Vec<String> = (0..9).map(|n| n.to_string()).collect();
    let d = with(json!({"template": "$MessagingStyle", "senderNames": vec!["S"; 9], "messages": many})).display();
    assert_eq!(d.conversation.len(), 8);
    assert_eq!(d.conversation[1], ConversationLine::Message("2".into()));
}
