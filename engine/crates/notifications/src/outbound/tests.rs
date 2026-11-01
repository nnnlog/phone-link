use super::*;
use serde_json::json;

#[test]
fn bodies() {
    let full = NotificationsRequest::full_sync(vec!["a".into(), "b".into()], vec![2, 1]).body("cv");
    assert_eq!(
        full.to_json(),
        json!({
            "correlationVector": {"String": "cv"},
            "contentType": {"String": "notifications"},
            "operation": {"Int32": 3},
            "notificationKeysCount": {"Int32": 2},
            "notificationKeys": {"StringArray": ["a", "b"]},
            "postTimes": {"Int64Array": [2, 1]}
        })
    );

    let dismiss = NotificationsRequest::dismiss("k").body("cv");
    assert_eq!(dismiss.keys().collect::<Vec<_>>(), ["correlationVector", "contentType", "operation", "key"]);

    let clear = NotificationsRequest::clear_all(vec!["k".into()]).body("cv");
    assert_eq!(clear.i32("operation"), Some(4));
    assert!(!clear.contains_key("postTimes"));

    let reply = NotificationsRequest::launch_action("k", 0, Some("hi")).body("cv");
    assert_eq!(reply.i32("actionIndex"), Some(0));
    assert_eq!(reply.str("inlineReplyMessage"), Some("hi"));

    let open = NotificationsRequest::open_on_phone("k", -1).body("cv");
    assert_eq!(open.keys().collect::<Vec<_>>(), ["correlationVector", "contentType", "operation", "key"]);
    assert_eq!(open.i32("operation"), Some(5));

    let open_action = NotificationsRequest::open_on_phone("k", 1).app_request_user_traced();
    assert_eq!(open_action.payload.i32("actionIndex"), Some(1));
    assert!(!open_action.options.user_initiated && open_action.options.traced_as_user_initiated);
    assert_eq!(open_action.options.trigger_id, "NotificationsAppLaunch");

    let empty_reply = NotificationsRequest::launch_action("k", 2, Some("")).body("cv");
    assert!(!empty_reply.contains_key("inlineReplyMessage"));

    let no_key = NotificationsRequest::dismiss("").body("cv");
    assert!(!no_key.contains_key("key"), "an empty key is omitted");
}
