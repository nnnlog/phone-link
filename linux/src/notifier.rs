use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::thread;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::edge::EdgeError;
use phonelink_features::shared::notifier::{
    NotificationAction, NotificationResponse, SystemNotification, SystemNotifier,
};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";
const APP_NAME: &str = "Phone Link";
const APP_ICON: &str = "smartphone";
const DEFAULT_ACTION: &str = "default";
const INLINE_REPLY_ACTION: &str = "inline-reply";
const CLOSED_BY_USER: u32 = 2;
const URGENCY_LOW: u8 = 0;
const URGENCY_NORMAL: u8 = 1;

struct Posted {
    id: String,
    user_info: HashMap<String, String>,
    reply_action: Option<String>,
    answered: bool,
}

#[derive(Default)]
struct Shown {
    by_server_id: HashMap<u32, Posted>,
    server_ids: HashMap<String, u32>,
}

impl Shown {
    fn remove(&mut self, server_id: u32) -> Option<Posted> {
        let posted = self.by_server_id.remove(&server_id)?;
        self.server_ids.remove(&posted.id);
        Some(posted)
    }
}

pub struct FreedesktopNotifier {
    bus: Connection,
    shown: Arc<Mutex<Shown>>,
    engine: Arc<OnceLock<Weak<Engine>>>,
    capabilities: Vec<String>,
}

impl FreedesktopNotifier {
    pub fn connect() -> zbus::Result<Self> {
        let bus = Connection::session()?;
        let capabilities: Vec<String> =
            Proxy::new(&bus, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS)?.call("GetCapabilities", &())?;
        let notifier = Self { bus, shown: Arc::default(), engine: Arc::default(), capabilities };
        notifier.watch_responses()?;
        Ok(notifier)
    }

    fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    pub fn deliver_responses_to(&self, engine: &Arc<Engine>) {
        let _ = self.engine.set(Arc::downgrade(engine));
    }

    fn proxy(&self) -> zbus::Result<Proxy<'static>> {
        Proxy::new(&self.bus, NOTIFICATIONS, NOTIFICATIONS_PATH, NOTIFICATIONS)
    }

    fn watch_responses(&self) -> zbus::Result<()> {
        let proxy = self.proxy()?;
        let invoked = proxy.receive_signal("ActionInvoked")?;
        let replied = proxy.receive_signal("NotificationReplied")?;
        let closed = proxy.receive_signal("NotificationClosed")?;

        let (shown, engine) = (self.shown.clone(), self.engine.clone());
        thread::spawn(move || {
            for message in invoked {
                let Ok((server_id, action)) = message.body().deserialize::<(u32, String)>() else { continue };
                respond(&shown, &engine, server_id, |_| match action.as_str() {
                    DEFAULT_ACTION => Some(NotificationResponse::Activated),
                    INLINE_REPLY_ACTION => None,
                    _ => Some(NotificationResponse::Action { action_id: action }),
                });
            }
        });

        let (shown, engine) = (self.shown.clone(), self.engine.clone());
        thread::spawn(move || {
            for message in replied {
                let Ok((server_id, text)) = message.body().deserialize::<(u32, String)>() else { continue };
                respond(&shown, &engine, server_id, |posted| {
                    let action_id = posted.reply_action.clone()?;
                    Some(NotificationResponse::Reply { action_id, text })
                });
            }
        });

        let (shown, engine) = (self.shown.clone(), self.engine.clone());
        thread::spawn(move || {
            for message in closed {
                let Ok((server_id, reason)) = message.body().deserialize::<(u32, u32)>() else { continue };
                let posted = shown.lock().unwrap_or_else(PoisonError::into_inner).remove(server_id);
                if let Some(posted) = posted.filter(|p| reason == CLOSED_BY_USER && !p.answered) {
                    deliver(&engine, posted.id, posted.user_info, NotificationResponse::Dismissed);
                }
            }
        });

        Ok(())
    }
}

fn respond(
    shown: &Mutex<Shown>,
    engine: &OnceLock<Weak<Engine>>,
    server_id: u32,
    response_for: impl FnOnce(&Posted) -> Option<NotificationResponse>,
) {
    let mut shown = shown.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(posted) = shown.by_server_id.get_mut(&server_id) else { return };
    let Some(response) = response_for(posted) else { return };
    posted.answered = true;
    let (id, user_info) = (posted.id.clone(), posted.user_info.clone());
    drop(shown);
    deliver(engine, id, user_info, response);
}

fn deliver(
    engine: &OnceLock<Weak<Engine>>,
    id: String,
    user_info: HashMap<String, String>,
    response: NotificationResponse,
) {
    let Some(engine) = engine.get().and_then(Weak::upgrade) else { return };
    if let Err(e) = engine.notification_response(id, user_info, response) {
        eprintln!("a notification response was not delivered: {e}");
    }
}

fn notifier_err(e: zbus::Error) -> EdgeError {
    EdgeError::Unexpected { message: format!("notifications: {e}") }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn body_of(n: &SystemNotification) -> String {
    match n.subtitle.as_deref().filter(|s| !s.is_empty()) {
        Some(subtitle) => format!("<b>{}</b>\n{}", escape_markup(subtitle), escape_markup(&n.body)),
        None => escape_markup(&n.body),
    }
}

fn action_list(actions: &[NotificationAction], inline_reply: bool) -> (Vec<String>, Option<&NotificationAction>) {
    let reply = actions.iter().find(|a| a.text_input.is_some()).filter(|_| inline_reply);
    let mut list = vec![DEFAULT_ACTION.to_owned(), String::new()];
    for action in actions {
        let key = match (&action.text_input, reply) {
            (Some(_), Some(r)) if r.id == action.id => INLINE_REPLY_ACTION,
            (Some(_), _) => continue,
            (None, _) => action.id.as_str(),
        };
        list.extend([key.to_owned(), action.title.clone()]);
    }
    (list, reply)
}

impl SystemNotifier for FreedesktopNotifier {
    fn post(&self, n: SystemNotification) -> Result<(), EdgeError> {
        let (actions, reply) = action_list(&n.actions, self.supports(INLINE_REPLY_ACTION));
        let mut hints: HashMap<&str, Value> = HashMap::new();
        hints.insert("urgency", Value::U8(if n.alert { URGENCY_NORMAL } else { URGENCY_LOW }));
        if let Some(input) = reply.and_then(|r| r.text_input.as_ref()) {
            hints.insert("x-kde-reply-placeholder-text", Value::from(input.placeholder.clone()));
            hints.insert("x-kde-reply-submit-button-text", Value::from(input.button_title.clone()));
        }

        let replaces = self.shown.lock().unwrap_or_else(PoisonError::into_inner).server_ids.get(&n.id).copied();
        let server_id: u32 = self
            .proxy()
            .and_then(|proxy| {
                proxy.call(
                    "Notify",
                    &(APP_NAME, replaces.unwrap_or(0), APP_ICON, n.title.as_str(), body_of(&n), actions, hints, -1i32),
                )
            })
            .map_err(notifier_err)?;

        let mut shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = replaces {
            shown.remove(previous);
        }
        shown.server_ids.insert(n.id.clone(), server_id);
        shown.by_server_id.insert(
            server_id,
            Posted { id: n.id, user_info: n.user_info, reply_action: reply.map(|r| r.id.clone()), answered: false },
        );
        Ok(())
    }

    fn withdraw(&self, ids: Vec<String>) -> Result<(), EdgeError> {
        let server_ids: Vec<u32> = {
            let shown = self.shown.lock().unwrap_or_else(PoisonError::into_inner);
            ids.iter().filter_map(|id| shown.server_ids.get(id).copied()).collect()
        };
        let proxy = self.proxy().map_err(notifier_err)?;
        for server_id in server_ids {
            proxy.call::<_, _, ()>("CloseNotification", &(server_id,)).map_err(notifier_err)?;
        }
        Ok(())
    }

    fn is_authorized(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phonelink_features::shared::notifier::TextInput;

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
    fn body_is_escaped_with_the_subtitle_first() {
        let n = SystemNotification {
            id: "messaging/1".to_owned(),
            thread_id: None,
            title: "Kim".to_owned(),
            subtitle: Some("A&B".to_owned()),
            body: "<hi>".to_owned(),
            alert: true,
            actions: vec![],
            user_info: HashMap::new(),
            image: None,
        };
        assert_eq!(body_of(&n), "<b>A&amp;B</b>\n&lt;hi&gt;");
    }
}
