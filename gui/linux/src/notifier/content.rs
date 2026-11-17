//! What a notification carries to the server: the app's name and icon, its body text, its actions and
//! its hints.

use std::collections::HashMap;

use phonelink_features::shared::notifier::NotificationAction;
use zbus::zvariant::Value;

pub(super) const APP_NAME: &str = "Phone Link";
pub(super) const APP_ICON: &str = "phonelink";
/// The installed .desktop file, which ties the notifications to the app's name, icon and settings.
const DESKTOP_ENTRY: &str = "phonelink-linux";
pub(super) const DEFAULT_ACTION: &str = "default";
pub(super) const INLINE_REPLY_ACTION: &str = "inline-reply";
pub(super) const BODY_MARKUP: &str = "body-markup";
pub(super) const SOUND: &str = "sound";
/// The sound theme's sound an alert plays where the server plays sounds.
pub(super) const ALERT_SOUND: &str = "message-new-instant";
pub(super) const URGENCY_LOW: u8 = 0;
pub(super) const URGENCY_NORMAL: u8 = 1;
pub(super) const URGENCY_CRITICAL: u8 = 2;

pub(super) fn supports(capabilities: &[String], capability: &str) -> bool {
    capabilities.iter().any(|c| c == capability)
}

pub(super) fn base_hints() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([("desktop-entry", Value::from(DESKTOP_ENTRY))])
}

/// An alert interrupts at normal urgency with the theme's message sound where the server plays sounds;
/// a silent delivery comes at low urgency without a sound, so that it neither interrupts nor plays.
pub(super) fn alert_hints(alert: bool, plays_sounds: bool) -> Vec<(&'static str, Value<'static>)> {
    if !alert {
        return vec![("urgency", Value::U8(URGENCY_LOW)), ("suppress-sound", Value::Bool(true))];
    }
    let mut hints = vec![("urgency", Value::U8(URGENCY_NORMAL))];
    if plays_sounds {
        hints.push(("sound-name", Value::from(ALERT_SOUND)));
    }
    hints
}

/// The body with the subtitle first: bold where the server renders markup, plain where it does not.
pub(super) fn body_text(markup: bool, subtitle: Option<&str>, body: &str) -> String {
    match subtitle.filter(|s| !s.is_empty()) {
        Some(subtitle) if markup => format!("<b>{}</b>\n{}", escape_markup(subtitle), escape_markup(body)),
        Some(subtitle) => format!("{subtitle}\n{body}"),
        None if markup => escape_markup(body),
        None => body.to_owned(),
    }
}

fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The server's action list for `actions` (the default action, then the key and label of each), and
/// the action that becomes the inline reply.
///
/// With `inline_reply` (the server takes replies), the first action that takes a reply becomes the
/// inline reply; every other action that takes a reply is left out.
pub(super) fn action_list(
    actions: &[NotificationAction],
    inline_reply: bool,
) -> (Vec<String>, Option<&NotificationAction>) {
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
