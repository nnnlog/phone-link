//! The notifications the app posts itself, which the app answers and the engine never sees.

use std::collections::HashMap;
use std::sync::Arc;

use phonelink_engine::edge::EdgeError;
use zbus::zvariant::Value;

use super::FreedesktopNotifier;
use super::content::{BODY_MARKUP, DEFAULT_ACTION, URGENCY_CRITICAL, URGENCY_NORMAL, base_hints, body_text, supports};
use super::shown::Posted;

/// What the app does with an action on a notification it posted itself (not the engine's): the action
/// key, `default` for a click on the notification.
pub(crate) type LocalHandler = Arc<dyn Fn(&str) + Send + Sync>;

/// A notification the app posts itself, such as a mirrored app taking none of the files dropped on it.
pub(crate) struct LocalNotification<'a> {
    pub(crate) id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
    /// (key, label) of each button.
    pub(crate) actions: &'a [(&'a str, &'a str)],
    /// Stays until it is answered (urgency critical).
    pub(crate) critical: bool,
}

impl FreedesktopNotifier {
    /// Posts a notification the app answers itself: `handler` gets each action key (`default` for a
    /// click); the engine never sees it.
    pub(crate) fn post_local(
        &self,
        notification: &LocalNotification<'_>,
        handler: LocalHandler,
    ) -> Result<(), EdgeError> {
        let capabilities = self.capabilities()?;
        let mut actions = vec![DEFAULT_ACTION.to_owned(), String::new()];
        for (key, label) in notification.actions {
            actions.extend([(*key).to_owned(), (*label).to_owned()]);
        }
        let mut hints = base_hints();
        hints.insert("urgency", Value::U8(if notification.critical { URGENCY_CRITICAL } else { URGENCY_NORMAL }));
        if notification.critical {
            hints.insert("resident", Value::Bool(true));
        }

        let replaces = self.server_id(notification.id);
        let body = body_text(supports(&capabilities, BODY_MARKUP), None, notification.body);
        let server_id = self.notify(replaces, notification.title, &body, actions, hints)?;
        let posted = Posted {
            id: notification.id.to_owned(),
            user_info: HashMap::new(),
            reply_action: None,
            answered: false,
            local: Some(handler),
        };
        self.remember(server_id, replaces, posted);
        Ok(())
    }
}
