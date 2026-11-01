use super::*;

#[test]
fn anchors_replace_and_keep_rcs_without_receive() {
    let mut a = Anchors::NONE;
    a.apply(&AnchorUpdate { base: [1, 2, 3, 4], rcs: Some([5, 6, 7]) });
    assert_eq!(a.set().len(), 7);
    a.apply(&AnchorUpdate { base: [9, -1, -1, -1], rcs: None });
    assert_eq!(a.set(), vec![("sms", 9), ("rcs_chat", 5), ("rcs_filetransfer", 6), ("rcs_conversation", 7)]);
}

#[test]
fn participant_rules() {
    assert!(valid_participant("+15550100"));
    assert!(!valid_participant(""));
    assert!(!valid_participant("bot@RCS.Google.com"));

    assert_eq!(newest_first(&[5, 9, 5]), vec![1, 2, 0]);

    let subs = vec![
        SubscriptionRow { id: 1, country_iso: String::new(), is_default_sms: false, ..Default::default() },
        SubscriptionRow { id: 2, country_iso: "kr".into(), is_default_sms: true, ..Default::default() },
    ];
    assert_eq!(message_region(subscription_for(&subs, 1), Some("US")), Some("US"), "empty ISO: the phone's");
    assert_eq!(message_region(subscription_for(&subs, 7), Some("US")), Some("kr"), "unknown: default SMS");

    assert_eq!(normalize("010-1234-5678", Some("kr")), "+821012345678");
    assert_eq!(normalize("not a number", Some("kr")), "not a number");
}
