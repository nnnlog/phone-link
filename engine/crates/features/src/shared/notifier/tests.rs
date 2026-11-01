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
