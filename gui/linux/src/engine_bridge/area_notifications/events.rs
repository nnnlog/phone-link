//! The notifications feature's changes: the list, the state and the apps' settings read again, an
//! action of this window finished, and a desktop notification's click.

use std::pin::Pin;

use cxx_qt_lib::QString;
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::notifications::{NotificationOpenTarget, NotificationsChange};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::translate;
use crate::user_text;

impl EngineBridge {
    pub(in crate::engine_bridge) fn notifications_changed(mut self: Pin<&mut Self>, change: NotificationsChange) {
        match change {
            NotificationsChange::ListChanged => self.reload_notifications(),
            NotificationsChange::StateChanged => self.reload_notification_state(),
            // The list hides an app whose switch went off; a row's menu and the settings pane read the
            // apps' settings.
            NotificationsChange::SettingsChanged => {
                self.as_mut().reload_notifications();
                self.bump(Revision::Settings);
            }
            NotificationsChange::ActionFinished { key, reply, outcome } => self.action_finished(key, reply, outcome),
            NotificationsChange::OpenRequested { key, target } => match target {
                NotificationOpenTarget::Notifications => self.show_notification(QString::from(key.as_str())),
                NotificationOpenTarget::Messaging => self.show_section(QString::from("messages")),
                NotificationOpenTarget::Calls => self.show_section(QString::from("calls")),
            },
        }
    }

    fn action_finished(mut self: Pin<&mut Self>, key: String, reply: bool, outcome: Option<RequestOutcome>) {
        let pending = &mut self.as_mut().notifications_mut().pending_actions;
        let action = (key, reply);
        // The engine logs the action's outcome where it reports it.
        match pending.get_mut(&action) {
            None => return,
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                pending.remove(&action);
            }
        }

        let text = match outcome {
            None => Some(translate("Phone Link couldn't reach your phone within 10 minutes.").to_owned()),
            Some(outcome) => user_text::outcome_failure(&outcome),
        };
        if let Some(text) = text {
            let title = if reply { "Couldn't Send Reply" } else { "Couldn't Complete Action" };
            self.show_failure(Failure::new(title, text));
        }
    }
}
