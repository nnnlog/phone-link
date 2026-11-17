//! The notifier's unit tests: the content of a notification, what the server's signals do to the shown
//! notifications, the click's activation token, and a run against a server on the session bus.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use phonelink_core::sync::MutexExt;
use phonelink_features::shared::notifier::{
    NotificationAction, NotificationResponse, SystemNotification, SystemNotifier, TextInput,
};
use zbus::Message;
use zbus::blocking::Connection;
use zbus::zvariant::Value;

use super::FreedesktopNotifier;
use super::activation::{ACTIVATION_TOKEN, TOKEN_LIFETIME, fresh_token};
use super::content::{
    ALERT_SOUND, DEFAULT_ACTION, INLINE_REPLY_ACTION, SOUND, URGENCY_LOW, URGENCY_NORMAL, action_list, alert_hints,
    body_text,
};
use super::freedesktop::Respond;
use super::server::{NOTIFICATIONS, NOTIFICATIONS_PATH, Server};
use super::shown::{CLOSED_BY_USER, CLOSED_EXPIRED, Delivery, Posted, Shown};
use super::take_activation_token;
use super::watch::ServerSignal;

fn action(id: &str, reply: bool) -> NotificationAction {
    NotificationAction {
        id: id.to_owned(),
        title: id.to_uppercase(),
        text_input: reply.then(|| TextInput { button_title: "Send".to_owned(), placeholder: "Reply".to_owned() }),
    }
}

#[test]
fn reply_action_becomes_the_inline_reply() {
    let actions = [action("mark", false), action("reply", true)];
    let (list, reply) = action_list(&actions, true);
    assert_eq!(list, ["default", "", "mark", "MARK", "inline-reply", "REPLY"]);
    assert_eq!(reply.map(|r| r.id.as_str()), Some("reply"));
}

#[test]
fn reply_action_is_left_out_without_inline_reply() {
    let actions = [action("mark", false), action("reply", true)];
    let (list, reply) = action_list(&actions, false);
    assert_eq!(list, ["default", "", "mark", "MARK"]);
    assert!(reply.is_none());
}

#[test]
fn body_is_escaped_with_the_subtitle_first_where_markup_renders() {
    assert_eq!(body_text(true, Some("A&B"), "<hi>"), "<b>A&amp;B</b>\n&lt;hi&gt;");
    assert_eq!(body_text(false, Some("A&B"), "<hi>"), "A&B\n<hi>");
    assert_eq!(body_text(false, None, "<hi>"), "<hi>");
}

#[test]
fn alerts_sound_where_the_server_plays_sounds_and_silent_ones_never() {
    let hints = |alert, sound| -> Vec<(&str, String)> {
        alert_hints(alert, sound).into_iter().map(|(key, value)| (key, format!("{value:?}"))).collect()
    };
    let urgency = |level: u8| format!("{:?}", Value::U8(level));
    assert_eq!(
        hints(false, true),
        [("urgency", urgency(URGENCY_LOW)), ("suppress-sound", format!("{:?}", Value::Bool(true)))]
    );
    assert_eq!(
        hints(true, true),
        [("urgency", urgency(URGENCY_NORMAL)), ("sound-name", format!("{:?}", Value::from(ALERT_SOUND)))]
    );
    assert_eq!(hints(true, false), [("urgency", urgency(URGENCY_NORMAL))]);
}

fn posted(id: &str, reply_action: Option<&str>) -> Posted {
    Posted {
        id: id.to_owned(),
        user_info: HashMap::from([("k".to_owned(), "v".to_owned())]),
        reply_action: reply_action.map(str::to_owned),
        answered: false,
        local: None,
    }
}

fn shown_with(server_id: u32, posted: Posted) -> Shown {
    let mut shown = Shown::default();
    shown.insert(server_id, None, posted);
    shown
}

/// The engine responses (id and response) and tokens of `signals`, applied in order.
fn run(shown: &mut Shown, signals: Vec<ServerSignal>) -> (Vec<(String, NotificationResponse)>, Vec<String>) {
    let (mut responses, mut tokens) = (Vec::new(), Vec::new());
    for signal in signals {
        let handled = shown.handle(signal);
        tokens.extend(handled.token);
        match handled.delivery {
            Some(Delivery::Engine { id, user_info, response }) => {
                assert_eq!(user_info.get("k").map(String::as_str), Some("v"), "the user info comes back");
                responses.push((id, response));
            }
            Some(Delivery::Local { action, .. }) => {
                responses.push((format!("local:{action}"), NotificationResponse::Activated))
            }
            None => {}
        }
    }
    (responses, tokens)
}

#[test]
fn a_click_is_not_undone_by_the_close_that_follows_it() {
    let mut shown = shown_with(7, posted("messaging/1", None));
    let (responses, tokens) = run(
        &mut shown,
        vec![
            ServerSignal::ActivationToken { server_id: 7, token: "token-7".to_owned() },
            ServerSignal::ActionInvoked { server_id: 7, action: "default".to_owned() },
            ServerSignal::Closed { server_id: 7, reason: CLOSED_BY_USER },
        ],
    );
    assert_eq!(responses, [("messaging/1".to_owned(), NotificationResponse::Activated)]);
    assert_eq!(tokens, ["token-7"]);
    assert!(shown.server_ids.is_empty() && shown.by_server_id.is_empty() && shown.tokens.is_empty());
}

#[test]
fn buttons_and_replies_answer_without_a_token() {
    let mut shown = shown_with(3, posted("notifications/a", Some("reply")));
    let (responses, tokens) = run(
        &mut shown,
        vec![
            ServerSignal::ActivationToken { server_id: 3, token: "token-3".to_owned() },
            ServerSignal::ActionInvoked { server_id: 3, action: "copyCode".to_owned() },
            ServerSignal::Closed { server_id: 3, reason: CLOSED_BY_USER },
        ],
    );
    assert_eq!(
        responses,
        [("notifications/a".to_owned(), NotificationResponse::Action { action_id: "copyCode".to_owned() })]
    );
    assert!(tokens.is_empty());

    let mut shown = shown_with(4, posted("messaging/2", Some("reply")));
    let (responses, tokens) = run(
        &mut shown,
        vec![
            ServerSignal::ActionInvoked { server_id: 4, action: INLINE_REPLY_ACTION.to_owned() },
            ServerSignal::Replied { server_id: 4, text: "on my way".to_owned() },
            ServerSignal::Closed { server_id: 4, reason: CLOSED_BY_USER },
        ],
    );
    let reply = NotificationResponse::Reply { action_id: "reply".to_owned(), text: "on my way".to_owned() };
    assert_eq!(responses, [("messaging/2".to_owned(), reply)]);
    assert!(tokens.is_empty());
}

#[test]
fn only_a_users_close_of_an_unanswered_notification_is_a_dismissal() {
    let mut shown = shown_with(5, posted("photos/1", None));
    let (responses, _) = run(&mut shown, vec![ServerSignal::Closed { server_id: 5, reason: CLOSED_BY_USER }]);
    assert_eq!(responses, [("photos/1".to_owned(), NotificationResponse::Dismissed)]);

    for reason in [3, 4] {
        let mut shown = shown_with(5, posted("photos/1", None));
        let (responses, _) = run(&mut shown, vec![ServerSignal::Closed { server_id: 5, reason }]);
        assert!(responses.is_empty());
        assert!(shown.server_ids.is_empty(), "closed for reason {reason}, it is forgotten");
    }
}

#[test]
fn an_expired_notification_stays_known_for_withdrawing_and_replacing() {
    let mut shown = shown_with(9, posted("notifications/b", None));
    let (responses, _) = run(&mut shown, vec![ServerSignal::Closed { server_id: 9, reason: CLOSED_EXPIRED }]);
    assert!(responses.is_empty());
    assert_eq!(shown.server_ids.get("notifications/b"), Some(&9));

    // Replaced under a new server id: the old one is forgotten.
    shown.insert(12, Some(9), posted("notifications/b", None));
    assert_eq!(shown.server_ids.get("notifications/b"), Some(&12));
    assert!(!shown.by_server_id.contains_key(&9));
    let (responses, _) = run(&mut shown, vec![ServerSignal::Closed { server_id: 9, reason: CLOSED_BY_USER }]);
    assert!(responses.is_empty(), "a close of the replaced id answers nothing");
    assert_eq!(shown.server_ids.get("notifications/b"), Some(&12));
}

#[test]
fn signals_for_other_ids_and_tokens_without_an_answer_do_nothing() {
    let mut shown = shown_with(1, posted("messaging/3", None));
    let (responses, tokens) = run(
        &mut shown,
        vec![
            ServerSignal::ActivationToken { server_id: 2, token: "other".to_owned() },
            ServerSignal::ActionInvoked { server_id: 2, action: "default".to_owned() },
            ServerSignal::Replied { server_id: 1, text: "no reply action".to_owned() },
        ],
    );
    assert!(responses.is_empty() && tokens.is_empty());
    assert!(shown.tokens.is_empty());
}

#[test]
fn a_local_notification_answers_its_handler_and_is_never_dismissed() {
    let mut local = posted("screen_mirroring/dragNotAccepted/1", None);
    local.local = Some(Arc::new(|_: &str| {}));
    let mut shown = shown_with(6, local);
    let (responses, tokens) = run(
        &mut shown,
        vec![
            ServerSignal::ActivationToken { server_id: 6, token: "token-6".to_owned() },
            ServerSignal::ActionInvoked { server_id: 6, action: "default".to_owned() },
            ServerSignal::Closed { server_id: 6, reason: CLOSED_BY_USER },
        ],
    );
    assert_eq!(responses, [("local:default".to_owned(), NotificationResponse::Activated)]);
    assert_eq!(tokens, ["token-6"]);
}

#[test]
fn a_server_change_forgets_what_it_showed() {
    let server = Server::default();
    server.shown().insert(1, None, posted("messaging/4", None));
    server.capabilities.lock_or_panic().1 = Some(vec![SOUND.to_owned()]);
    server.changed();
    assert!(server.shown().server_ids.is_empty());
    assert_eq!(*server.capabilities.lock_or_panic(), (1, None));
}

#[test]
fn signals_are_read_by_member() {
    let message = |member: &str, body: &(u32, &str)| {
        Message::signal(NOTIFICATIONS_PATH, NOTIFICATIONS, member).unwrap().build(body).unwrap()
    };
    assert_eq!(
        ServerSignal::parse("ActionInvoked", &message("ActionInvoked", &(4, "default"))),
        Some(ServerSignal::ActionInvoked { server_id: 4, action: "default".to_owned() })
    );
    assert_eq!(
        ServerSignal::parse("ActivationToken", &message("ActivationToken", &(4, "t"))),
        Some(ServerSignal::ActivationToken { server_id: 4, token: "t".to_owned() })
    );
    assert_eq!(
        ServerSignal::parse("NotificationReplied", &message("NotificationReplied", &(4, "hi"))),
        Some(ServerSignal::Replied { server_id: 4, text: "hi".to_owned() })
    );
    let closed = Message::signal(NOTIFICATIONS_PATH, NOTIFICATIONS, "NotificationClosed")
        .unwrap()
        .build(&(4u32, CLOSED_BY_USER))
        .unwrap();
    assert_eq!(
        ServerSignal::parse("NotificationClosed", &closed),
        Some(ServerSignal::Closed { server_id: 4, reason: CLOSED_BY_USER })
    );
    assert_eq!(ServerSignal::parse("NotificationClosed", &message("NotificationClosed", &(4, "x"))), None);
    assert_eq!(ServerSignal::parse("Other", &message("Other", &(4, "x"))), None);
}

#[test]
fn a_click_token_is_taken_once_while_it_is_fresh() {
    let clicked = Instant::now();
    *ACTIVATION_TOKEN.lock_or_panic() = Some(("stale".to_owned(), clicked));
    assert_eq!(fresh_token(clicked + TOKEN_LIFETIME * 2), None);
    assert!(ACTIVATION_TOKEN.lock_or_panic().is_none(), "a stale token is dropped");

    *ACTIVATION_TOKEN.lock_or_panic() = Some(("fresh".to_owned(), Instant::now()));
    assert_eq!(take_activation_token().as_deref(), Some("fresh"));
    assert_eq!(take_activation_token(), None, "a token is taken once");
}

/// What the fake server was asked to show: its id and the hints that concern sound.
#[derive(Debug, Clone, PartialEq)]
struct Asked {
    id: u32,
    urgency: Option<u8>,
    suppress_sound: Option<bool>,
    sound_name: Option<String>,
}

/// A notification server for a test, on the session bus.
struct FakeServer {
    capabilities: Vec<String>,
    asked: Arc<Mutex<Vec<Asked>>>,
    open: HashSet<u32>,
    next: u32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeServer {
    fn get_capabilities(&self) -> Vec<String> {
        self.capabilities.clone()
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the method's arguments are the Desktop Notifications Specification's"
    )]
    fn notify(
        &mut self,
        _app_name: String,
        replaces_id: u32,
        _app_icon: String,
        _summary: String,
        _body: String,
        _actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let id = if self.open.contains(&replaces_id) {
            replaces_id
        } else {
            self.next += 1;
            self.next
        };
        self.open.insert(id);
        let hint = |key: &str| hints.get(key).map(|value| Value::from(value.clone()));
        self.asked.lock_or_panic().push(Asked {
            id,
            urgency: hint("urgency").and_then(|value| u8::try_from(&value).ok()),
            suppress_sound: hint("suppress-sound").and_then(|value| bool::try_from(&value).ok()),
            sound_name: hint("sound-name").and_then(|value| String::try_from(value).ok()),
        });
        id
    }

    fn close_notification(&mut self, id: u32) -> zbus::fdo::Result<()> {
        if self.open.remove(&id) { Ok(()) } else { Err(zbus::fdo::Error::Failed("no such notification".to_owned())) }
    }
}

fn start_server(capabilities: &[&str], asked: &Arc<Mutex<Vec<Asked>>>) -> Connection {
    let server = FakeServer {
        capabilities: capabilities.iter().map(|c| (*c).to_owned()).collect(),
        asked: asked.clone(),
        open: HashSet::new(),
        next: 0,
    };
    zbus::blocking::connection::Builder::session()
        .unwrap()
        .serve_at(NOTIFICATIONS_PATH, server)
        .unwrap()
        .name(NOTIFICATIONS)
        .unwrap()
        .build()
        .unwrap()
}

fn emit<B: serde::Serialize + zbus::zvariant::DynamicType>(server: &Connection, member: &str, body: &B) {
    server.emit_signal(None::<()>, NOTIFICATIONS_PATH, NOTIFICATIONS, member, body).unwrap();
}

fn notification(id: &str, alert: bool) -> SystemNotification {
    SystemNotification {
        id: id.to_owned(),
        thread_id: None,
        title: "Title".to_owned(),
        subtitle: None,
        body: "Body".to_owned(),
        alert,
        actions: vec![action("copyCode", false)],
        user_info: HashMap::new(),
        image: None,
    }
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "needs a D-Bus session bus of its own (run it under dbus-run-session)"]
fn notifications_go_through_whichever_server_runs() {
    let (responses, received) = mpsc::channel();
    let respond: Respond = Box::new(move |id, _, response| responses.send((id, response)).unwrap());
    let notifier = FreedesktopNotifier::responding(respond, Arc::default()).unwrap();
    let no_more = |what: &str| {
        assert!(received.recv_timeout(Duration::from_millis(300)).is_err(), "{what}");
    };
    assert!(notifier.post(notification("messaging/1", true)).is_err(), "no server runs yet");

    // A server that starts later is used, with its capabilities.
    let asked = Arc::new(Mutex::new(Vec::new()));
    let server = start_server(&[SOUND], &asked);
    notifier.post(notification("messaging/1", true)).unwrap();
    notifier.post(notification("photos/1", false)).unwrap();
    let sound = Some(ALERT_SOUND.to_owned());
    assert_eq!(
        *asked.lock_or_panic(),
        [
            Asked { id: 1, urgency: Some(URGENCY_NORMAL), suppress_sound: None, sound_name: sound },
            Asked { id: 2, urgency: Some(URGENCY_LOW), suppress_sound: Some(true), sound_name: None },
        ]
    );

    // A click, with the close that follows it: one response, and the click's token.
    emit(&server, "ActivationToken", &(1u32, "token-1"));
    emit(&server, "ActionInvoked", &(1u32, DEFAULT_ACTION));
    emit(&server, "NotificationClosed", &(1u32, CLOSED_BY_USER));
    let (id, response) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((id.as_str(), response), ("messaging/1", NotificationResponse::Activated));
    no_more("the close after the click is no dismissal");
    assert_eq!(fresh_token(Instant::now()).as_deref(), Some("token-1"));

    // An expired notification can still be withdrawn; one the server forgot is forgotten.
    emit(&server, "NotificationClosed", &(2u32, CLOSED_EXPIRED));
    no_more("an expiry answers nothing");
    assert_eq!(notifier.server_id("photos/1"), Some(2));
    let fake = server.object_server().interface::<_, FakeServer>(NOTIFICATIONS_PATH).unwrap();
    fake.get_mut().open.remove(&2);
    notifier.post(notification("calls/1", true)).unwrap();
    notifier.withdraw(vec!["photos/1".to_owned(), "calls/1".to_owned(), "unknown/1".to_owned()]).unwrap();
    assert_eq!(notifier.server_id("photos/1"), None);
    assert!(!fake.get().open.contains(&3), "the rest are closed after a failure");

    // A user's dismissal of an unanswered notification is one.
    notifier.post(notification("messaging/2", true)).unwrap();
    emit(&server, "NotificationClosed", &(4u32, CLOSED_BY_USER));
    let (id, response) = received.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((id.as_str(), response), ("messaging/2", NotificationResponse::Dismissed));

    // A restarted server: what the old one showed is forgotten, and the new one's capabilities count.
    notifier.post(notification("messaging/3", true)).unwrap();
    drop(fake);
    drop(server);
    wait_until("the old server is forgotten", || notifier.server_id("messaging/3").is_none());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let _server = start_server(&[], &asked);
    notifier.post(notification("messaging/3", true)).unwrap();
    assert_eq!(
        *asked.lock_or_panic(),
        [Asked { id: 1, urgency: Some(URGENCY_NORMAL), suppress_sound: None, sound_name: None }]
    );
}
