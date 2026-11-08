use super::*;
use phonelink_core::types::Environment;
use std::collections::HashMap;

fn phone(id: &str) -> DcgClient {
    DcgClient::new(id, Environment::Prod)
}

fn notification(id: &str, body: &str) -> SystemNotification {
    SystemNotification {
        id: id.to_string(),
        thread_id: None,
        title: "1234-5678".into(),
        subtitle: None,
        body: body.to_string(),
        alert: true,
        actions: Vec::new(),
        user_info: HashMap::new(),
        image: None,
    }
}

const BODY: &str = "[Web발신] [서비스] 인증번호 [123456]를 입력해주세요.";

#[test]
fn an_offer_completes_a_kept_notification() {
    let mut pairing = CodePairing::default();
    pairing.keep_posted(&phone("a"), notification("messaging/a/sms/1", BODY), 0);

    assert!(pairing.offer(&phone("b"), "123456", 10).is_none(), "another phone's offer is kept, not paired");
    assert!(pairing.offer(&phone("a"), "12345", 10).is_none(), "the code must be a whole digit run");

    let found = pairing.offer(&phone("a"), "123456", 20).expect("the kept notification");
    assert_eq!(found.id, "messaging/a/sms/1");
    assert!(pairing.offer(&phone("a"), "123456", 30).is_none(), "the notification is no longer kept");
}

#[test]
fn a_kept_offer_completes_a_later_notification_once() {
    let mut pairing = CodePairing::default();
    assert!(pairing.offer(&phone("a"), "123456", 0).is_none());

    assert_eq!(pairing.take_offer(&phone("b"), BODY, 10), None);
    assert_eq!(pairing.take_offer(&phone("a"), "no code here", 10), None);
    assert_eq!(pairing.take_offer(&phone("a"), BODY, 10).as_deref(), Some("123456"));
    assert_eq!(pairing.take_offer(&phone("a"), BODY, 20), None, "the offer is used up");
}

#[test]
fn both_sides_expire_after_the_window() {
    let mut pairing = CodePairing::default();
    pairing.keep_posted(&phone("a"), notification("messaging/a/sms/1", BODY), 0);
    assert!(pairing.offer(&phone("a"), "123456", KEEP_MS + 1).is_none());

    assert_eq!(pairing.take_offer(&phone("a"), BODY, KEEP_MS * 2 + 2), None);
}

#[test]
fn only_a_body_with_a_possible_code_and_no_copy_is_kept() {
    let mut pairing = CodePairing::default();
    pairing.keep_posted(&phone("a"), notification("messaging/a/sms/1", "see you at 12"), 0);
    assert!(pairing.offer(&phone("a"), "12", 1).is_none());

    let mut with_copy = notification("messaging/a/sms/2", BODY);
    copy::offer(&mut with_copy, "123456");
    pairing.keep_posted(&phone("a"), with_copy, 0);
    assert!(pairing.offer(&phone("a"), "123456", 1).is_none());
}

#[test]
fn a_forgotten_notification_is_not_completed() {
    let mut pairing = CodePairing::default();
    pairing.keep_posted(&phone("a"), notification("messaging/a/sms/1", BODY), 0);

    pairing.forget("messaging/a/sms/1");

    assert!(pairing.offer(&phone("a"), "123456", 1).is_none());
}
