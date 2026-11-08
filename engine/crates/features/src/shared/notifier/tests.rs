use super::*;
use phonelink_core::sync::MutexExt;
use std::sync::Mutex;

struct Collect(Mutex<Vec<(String, NotificationResponse)>>);

impl NotificationResponder for Collect {
    fn on_response(&self, id: &str, _user_info: &HashMap<String, String>, response: NotificationResponse) {
        self.0.lock_or_panic().push((id.to_string(), response));
    }
}

#[test]
fn responses_go_to_the_owning_feature() {
    let router = NotificationRouter::new();
    let a = Arc::new(Collect(Mutex::new(Vec::new())));

    router.register("messaging", a.clone()).unwrap();
    assert!(router.register("messaging", a.clone()).is_err());

    router.route("messaging/p/42", &HashMap::new(), NotificationResponse::Activated);
    router.route("photos/p/1", &HashMap::new(), NotificationResponse::Dismissed);

    assert_eq!(*a.0.lock_or_panic(), vec![("messaging/p/42".to_string(), NotificationResponse::Activated)]);
}

struct Copies(Mutex<Vec<String>>);

impl CodeCopier for Copies {
    fn copy_code(&self, code: String) {
        self.0.lock_or_panic().push(code);
    }
}

fn copy_info(code: Option<&str>) -> HashMap<String, String> {
    code.map(|c| HashMap::from([(copy::CODE_KEY.to_string(), c.to_string())])).unwrap_or_default()
}

#[test]
fn the_copy_action_goes_to_the_copier_not_the_feature() {
    let router = NotificationRouter::new();
    let feature = Arc::new(Collect(Mutex::new(Vec::new())));
    let copies = Arc::new(Copies(Mutex::new(Vec::new())));
    router.register("messaging", feature.clone()).unwrap();
    router.set_code_copier(copies.clone()).unwrap();
    assert!(router.set_code_copier(copies.clone()).is_err());

    let copy = NotificationResponse::Action { action_id: copy::ACTION_ID.to_string() };
    router.route("messaging/p/42", &copy_info(Some("123456")), copy.clone());
    router.route("messaging/p/43", &copy_info(None), copy);
    router.route("messaging/p/44", &copy_info(None), NotificationResponse::Action { action_id: "other".into() });

    assert_eq!(*copies.0.lock_or_panic(), vec!["123456".to_string()]);
    assert_eq!(
        *feature.0.lock_or_panic(),
        vec![("messaging/p/44".to_string(), NotificationResponse::Action { action_id: "other".into() })]
    );
}

#[test]
fn offering_copy_replaces_the_actions_and_keeps_the_code() {
    let mut n = SystemNotification {
        id: "messaging/p/1".into(),
        thread_id: None,
        title: "t".into(),
        subtitle: None,
        body: "b".into(),
        alert: true,
        actions: vec![NotificationAction { id: "reply".into(), title: "Reply".into(), text_input: None }],
        user_info: HashMap::from([("phone".to_string(), "p".to_string())]),
        image: None,
    };
    assert!(!copy::offered(&n));

    copy::offer(&mut n, "123456");

    assert!(copy::offered(&n));
    assert_eq!(n.actions.len(), 1);
    assert_eq!(n.user_info.get(copy::CODE_KEY).map(String::as_str), Some("123456"));
    assert_eq!(n.user_info.get("phone").map(String::as_str), Some("p"));
}
