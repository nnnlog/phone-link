//! The system-notification OS edge (macOS: `UNUserNotificationCenter`), shared by the features that
//! present phone events as system notifications.
//!
//! It covers notifications, device status (the low-battery notification), messaging (new-message
//! notifications) and photos (new-photo notifications).
//!
//! The engine decides **whether** and **what** to post — each feature keeps Phone Link's decision rules —
//! and the edge only presents. What the user does with a posted notification comes back through
//! [`NotificationRouter::route`], which hands it to the feature that owns the notification id: every
//! id starts with the owning feature's name and a `/` (`notifications/…`, `messaging/…`).
//!
//! A notification that holds an authentication code carries the [`copy`] action instead of its own
//! actions; the router handles that action itself and writes the code to the Mac's pasteboard through
//! the [`CodeCopier`], so pressing it never reaches the owning feature or the phone.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use tracing::{error, warn};

use phonelink_core::edge::EdgeError;
use phonelink_core::sync::RwLockExt;

/// A text field on an action (macOS: `UNTextInputNotificationAction`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TextInput {
    pub button_title: String,
    pub placeholder: String,
}

/// One action offered on a notification.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotificationAction {
    /// Unique within the notification; comes back in [`NotificationResponse`].
    pub id: String,
    pub title: String,
    /// `Some`: a text-input (reply) action.
    pub text_input: Option<TextInput>,
}

/// One system notification (macOS: a `UNNotificationRequest` with its content).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SystemNotification {
    /// `<feature>/<feature-chosen id>`; posting an id that is shown replaces it.
    pub id: String,
    /// Grouping (macOS: `threadIdentifier`).
    pub thread_id: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    /// `false`: deliver silently into the notification center without a banner or sound (macOS:
    /// interruption level `passive`).
    pub alert: bool,
    pub actions: Vec<NotificationAction>,
    /// Handed back unchanged with the response.
    pub user_info: HashMap<String, String>,
    /// An image shown with the notification (macOS: a `UNNotificationAttachment`), when any.
    pub image: Option<Vec<u8>>,
}

/// What the user did with a posted notification (macOS: `UNNotificationResponse`).
#[derive(Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NotificationResponse {
    /// The notification itself was clicked.
    Activated,
    /// A button action.
    Action { action_id: String },
    /// A text-input action with what the user typed.
    Reply { action_id: String, text: String },
    /// The user dismissed it from the notification center.
    Dismissed,
}

/// Shows the length of a reply's text, never the text: it is what the user typed.
impl std::fmt::Debug for NotificationResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Activated => f.write_str("Activated"),
            Self::Action { action_id } => f.debug_struct("Action").field("action_id", action_id).finish(),
            Self::Reply { action_id, text } => {
                f.debug_struct("Reply").field("action_id", action_id).field("text_len", &text.len()).finish()
            }
            Self::Dismissed => f.write_str("Dismissed"),
        }
    }
}

/// The Copy action of a notification that holds an authentication code.
pub mod copy {
    use super::{NotificationAction, SystemNotification};

    pub const ACTION_ID: &str = "copyCode";
    /// The user-info key that holds the code.
    pub const CODE_KEY: &str = "copyCode";
    pub const TITLE: &str = "Copy";

    /// Makes Copy the only action of `notification`, for `code`.
    pub fn offer(notification: &mut SystemNotification, code: &str) {
        notification.actions =
            vec![NotificationAction { id: ACTION_ID.to_string(), title: TITLE.to_string(), text_input: None }];
        notification.user_info.insert(CODE_KEY.to_string(), code.to_string());
    }

    /// `notification` offers Copy.
    pub fn offered(notification: &SystemNotification) -> bool {
        notification.actions.iter().any(|a| a.id == ACTION_ID)
    }
}

/// Writes an authentication code to the Mac's pasteboard, off the caller's thread.
pub trait CodeCopier: Send + Sync {
    fn copy_code(&self, code: String);
}

/// Posting and withdrawing system notifications.
#[uniffi::export(with_foreign)]
pub trait SystemNotifier: Send + Sync {
    /// Posts `notification`, replacing a shown one with the same id.
    fn post(&self, notification: SystemNotification) -> Result<(), EdgeError>;
    /// Withdraws shown or pending notifications by id; unknown ids are ignored.
    fn withdraw(&self, ids: Vec<String>) -> Result<(), EdgeError>;
    /// The user allows the app to post notifications (macOS: the notification authorization).
    fn is_authorized(&self) -> bool;
}

/// A feature's handler of the responses to its notifications.
pub trait NotificationResponder: Send + Sync {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse);
}

/// Routes a response to the feature named by the id's first `/`-separated part, except the Copy action,
/// which it hands to the [`CodeCopier`].
#[derive(Default)]
pub struct NotificationRouter {
    responders: RwLock<HashMap<String, Arc<dyn NotificationResponder>>>,
    copier: OnceLock<Arc<dyn CodeCopier>>,
}

/// A second notification responder for a feature that already has one.
#[derive(Debug, thiserror::Error)]
#[error("{0} already has a notification responder")]
pub struct ResponderTaken(pub String);

/// A second code copier.
#[derive(Debug, thiserror::Error)]
#[error("the notification router already has a code copier")]
pub struct CopierTaken;

impl NotificationRouter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers the responder of the ids `<feature>/…`; a feature registers once.
    pub fn register(&self, feature: &str, responder: Arc<dyn NotificationResponder>) -> Result<(), ResponderTaken> {
        let mut map = self.responders.write_or_panic();
        if map.contains_key(feature) {
            return Err(ResponderTaken(feature.to_string()));
        }
        map.insert(feature.to_string(), responder);
        Ok(())
    }

    /// Sets where the Copy action writes; set once.
    pub fn set_code_copier(&self, copier: Arc<dyn CodeCopier>) -> Result<(), CopierTaken> {
        self.copier.set(copier).map_err(|_| CopierTaken)
    }

    /// Hands `response` to the owning feature; an id no feature owns is logged and dropped. The Copy
    /// action goes to the code copier instead.
    pub fn route(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        if matches!(&response, NotificationResponse::Action { action_id } if action_id == copy::ACTION_ID) {
            self.copy(id, user_info);
            return;
        }

        let feature = id.split('/').next().unwrap_or_default();
        let responder = self.responders.read_or_panic().get(feature).cloned();
        match responder {
            Some(r) => r.on_response(id, user_info, response),
            None => warn!(id, "notification response dropped; no feature owns its id"),
        }
    }

    /// The Copy action: the code of `user_info` to the copier. The code itself is never logged.
    fn copy(&self, id: &str, user_info: &HashMap<String, String>) {
        let Some(code) = user_info.get(copy::CODE_KEY) else {
            error!(id, "copy action without its code; dropped (broken invariant)");
            return;
        };
        let Some(copier) = self.copier.get() else {
            warn!(id, "copy action dropped; no code copier is set");
            return;
        };

        copier.copy_code(code.clone());
    }
}

/// In-memory implementations for the CLI and for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::{SystemNotification, SystemNotifier};
    use phonelink_core::edge::EdgeError;
    use phonelink_core::sync::MutexExt;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// One call the notifier received.
    #[derive(Debug, Clone, PartialEq)]
    pub enum NotifierCall {
        Post(SystemNotification),
        Withdraw(Vec<String>),
    }

    /// Records every call; authorization is settable.
    #[derive(Debug)]
    pub struct RecordingNotifier {
        calls: Mutex<Vec<NotifierCall>>,
        authorized: std::sync::atomic::AtomicBool,
    }

    impl Default for RecordingNotifier {
        fn default() -> Self {
            Self { calls: Mutex::new(Vec::new()), authorized: std::sync::atomic::AtomicBool::new(true) }
        }
    }

    impl RecordingNotifier {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        pub fn set_authorized(&self, authorized: bool) {
            self.authorized.store(authorized, std::sync::atomic::Ordering::SeqCst);
        }
        pub fn calls(&self) -> Vec<NotifierCall> {
            self.calls.lock_or_panic().clone()
        }
        pub fn take(&self) -> Vec<NotifierCall> {
            std::mem::take(&mut *self.calls.lock_or_panic())
        }
        /// The notifications posted so far, in order.
        pub fn posted(&self) -> Vec<SystemNotification> {
            self.calls()
                .into_iter()
                .filter_map(|c| match c {
                    NotifierCall::Post(n) => Some(n),
                    NotifierCall::Withdraw(_) => None,
                })
                .collect()
        }
        /// Every id withdrawn so far, in order.
        pub fn withdrawn(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .flat_map(|c| match c {
                    NotifierCall::Withdraw(ids) => ids,
                    NotifierCall::Post(_) => Vec::new(),
                })
                .collect()
        }
    }

    impl SystemNotifier for RecordingNotifier {
        fn post(&self, notification: SystemNotification) -> Result<(), EdgeError> {
            self.calls.lock_or_panic().push(NotifierCall::Post(notification));
            Ok(())
        }
        fn withdraw(&self, ids: Vec<String>) -> Result<(), EdgeError> {
            self.calls.lock_or_panic().push(NotifierCall::Withdraw(ids));
            Ok(())
        }
        fn is_authorized(&self) -> bool {
            self.authorized.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests;
