//! The notifications of file transfer: a share-menu send's states (connecting, preparing, sending with
//! its progress and a cancel action) and its outcome; the grant-permission notification, whose action
//! asks the phone for the files permission; and the received notification, which opens the file or
//! shows the folder.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::sync::atomic::Ordering;

use tracing::{info, warn};

use super::send::SendError;
use super::{FEATURE, FileTransferEvent, Inner, PermissionTrigger, SendPhase, key};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::files_permission::PermissionResult;
use phonelink_features::shared::notifier::{
    NotificationAction, NotificationResponder, NotificationResponse, SystemNotification,
};
use phonelink_transport::auth::PeerDevice;

/// `user_info` key of the phone (`<dcgClientId>.<Environment>`).
pub(super) const USER_INFO_PHONE: &str = "phone";
/// `user_info` key of the file a click opens.
pub(super) const USER_INFO_FILE: &str = "file";
/// `user_info` key of the folder the folder action shows.
pub(super) const USER_INFO_FOLDER: &str = "folder";

pub(super) const ACTION_CANCEL: &str = "cancel";
pub(super) const ACTION_GRANT: &str = "grant";
pub(super) const ACTION_OPEN_FOLDER: &str = "openFolder";

/// The subtitle when the phone has no name.
pub(super) const DEVICE_LABEL: &str = "Device";

pub(super) fn send_id(phone: &DcgClient) -> String {
    format!("{FEATURE}/send/{}", key(phone))
}

pub(super) fn permission_id(phone: &DcgClient) -> String {
    format!("{FEATURE}/permission/{}", key(phone))
}

fn received_id() -> String {
    format!("{FEATURE}/received/{}", phonelink_core::types::new_guid_lower())
}

fn phone_name(peer: &PeerDevice) -> String {
    peer.info.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(DEVICE_LABEL).to_owned()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "file" } else { "files" }
}

impl Inner {
    /// Posts `notification`. A send posts one per progress step, so a failure is logged when it
    /// starts.
    fn post(&self, notification: SystemNotification) {
        let id = notification.id.clone();
        match self.env.notifier.post(notification) {
            Ok(()) => self.post_failing.store(false, Ordering::Relaxed),
            Err(e) => {
                if !self.post_failing.swap(true, Ordering::Relaxed) {
                    warn!(id, error = %e, "notification not posted");
                }
            }
        }
    }

    fn base(&self, id: String, peer: &PeerDevice, title: String, body: String) -> SystemNotification {
        SystemNotification {
            id,
            thread_id: Some(format!("{FEATURE}/{}", key(&peer.client))),
            title,
            subtitle: Some(phone_name(peer)),
            body,
            alert: true,
            actions: Vec::new(),
            user_info: HashMap::from([(USER_INFO_PHONE.to_owned(), key(&peer.client).to_string())]),
            image: None,
        }
    }

    /// A share-menu send's state; only the first one pops up.
    pub(super) fn notify_send_state(&self, peer: &PeerDevice, phase: SendPhase, fraction: f64) {
        let (title, body) = match phase {
            SendPhase::Connecting => ("Connecting".to_owned(), "Connecting to your phone…".to_owned()),
            SendPhase::Preparing => ("Preparing".to_owned(), "Waiting for your phone…".to_owned()),
            SendPhase::Sending => {
                ("Sending".to_owned(), format!("{}% sent", (fraction * 100.0).floor().clamp(0.0, 100.0) as u32))
            }
        };

        let mut n = self.base(send_id(&peer.client), peer, title, body);
        n.alert = phase == SendPhase::Connecting;
        n.actions =
            vec![NotificationAction { id: ACTION_CANCEL.to_owned(), title: "Cancel".to_owned(), text_input: None }];
        self.post(n);
    }

    /// A share-menu send's outcome, replacing its state.
    pub(super) fn notify_send_end(&self, peer: &PeerDevice, result: &Result<u32, SendError>, files: usize) {
        let (title, body) = match result {
            Ok(sent) => ("Sent".to_owned(), format!("{sent} {} sent to your phone", plural(*sent as usize))),
            Err(SendError::Cancelled) => ("Cancelled".to_owned(), "The transfer was cancelled".to_owned()),
            Err(SendError::AnotherTransferInProgress) => {
                ("Couldn't send".to_owned(), "Another transfer with your phone is in progress".to_owned())
            }
            Err(SendError::PermissionRequired) => {
                // The grant-permission notification says what to do.
                if let Err(e) = self.env.notifier.withdraw(vec![send_id(&peer.client)]) {
                    warn!(phone = %peer.client, error = %e, "send notification not withdrawn");
                }
                return;
            }
            Err(e) => ("Couldn't send".to_owned(), format!("{files} {} not sent: {e}", plural(files))),
        };

        self.post(self.base(send_id(&peer.client), peer, title, body));
    }

    /// The phone needs the files permission: its action asks the phone for it.
    pub(super) fn notify_grant_permission(&self, peer: &PeerDevice) {
        let mut n = self.base(
            permission_id(&peer.client),
            peer,
            "Permission needed".to_owned(),
            "Allow your phone to receive files from this Mac".to_owned(),
        );
        n.actions = vec![NotificationAction {
            id: ACTION_GRANT.to_owned(),
            title: "Grant permission".to_owned(),
            text_input: None,
        }];
        self.post(n);
    }

    /// Files arrived: one opens on a click, several show their folder.
    pub(super) fn notify_received(&self, peer: &PeerDevice, files: &[PathBuf], folder: Option<&Path>) {
        let folder = folder.map(Path::to_path_buf).or_else(|| files[0].parent().map(Path::to_path_buf));
        let body = match files {
            [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            many => format!("{} files received", many.len()),
        };

        let mut n = self.base(received_id(), peer, "Files received".to_owned(), body);
        if let [one] = files {
            n.user_info.insert(USER_INFO_FILE.to_owned(), one.to_string_lossy().into_owned());
        }
        if let Some(folder) = folder {
            n.user_info.insert(USER_INFO_FOLDER.to_owned(), folder.to_string_lossy().into_owned());
            n.actions = vec![NotificationAction {
                id: ACTION_OPEN_FOLDER.to_owned(),
                title: "Open folder".to_owned(),
                text_input: None,
            }];
        }

        self.post(n);
    }

    fn peer_of(&self, user_info: &HashMap<String, String>) -> Option<PeerDevice> {
        let phone = user_info.get(USER_INFO_PHONE)?;
        self.env.peers.peers().into_iter().find(|p| key(&p.client).to_string() == *phone)
    }

    fn respond(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let kind = id.strip_prefix(FEATURE).and_then(|r| r.strip_prefix('/')).and_then(|r| r.split('/').next());
        match (kind, response) {
            (_, NotificationResponse::Dismissed) => {}
            (Some("send"), NotificationResponse::Action { action_id }) if action_id == ACTION_CANCEL => {
                if let Some(peer) = self.peer_of(user_info) {
                    self.cancel_send(&peer.client);
                }
            }
            (Some("permission"), NotificationResponse::Action { action_id }) if action_id == ACTION_GRANT => {
                self.grant_permission(id, user_info);
            }
            (Some("received"), NotificationResponse::Activated) => self.open_received(id, user_info),
            (Some("received"), NotificationResponse::Action { action_id }) if action_id == ACTION_OPEN_FOLDER => {
                self.reveal_received_folder(id, user_info);
            }
            // A click on a send or permission notification brings nothing up.
            (_, NotificationResponse::Activated) => {}
            (_, other) => log_unexpected_response(id, &other),
        }
    }

    /// The grant-permission action: asks the phone for the files permission.
    fn grant_permission(&self, id: &str, user_info: &HashMap<String, String>) {
        let Some(peer) = self.peer_of(user_info) else {
            warn!(id, "grant-permission action for a phone not in the account; ignored");
            return;
        };

        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                let result =
                    inner.permission(&peer, false, PermissionTrigger::UserGrantFilesPermission, None, None).await;
                inner.record_send_permission(&peer.client, &result);
                match result {
                    PermissionResult::Granted => info!(phone = %peer.client, "files permission granted"),
                    PermissionResult::Required => {
                        info!(phone = %peer.client, "files permission requested; the phone prompts")
                    }
                    other => warn!(phone = %peer.client, result = ?other, "files permission request failed"),
                }
            }
        });
    }

    /// A click on a received notification: opens its file, or reveals its folder.
    fn open_received(&self, id: &str, user_info: &HashMap<String, String>) {
        match (user_info.get(USER_INFO_FILE), user_info.get(USER_INFO_FOLDER)) {
            (Some(file), _) => self.emit(FileTransferEvent::Open { path: PathBuf::from(file) }),
            (None, Some(folder)) => self.emit(FileTransferEvent::Reveal { path: PathBuf::from(folder) }),
            (None, None) => warn!(id, "received notification without its file or folder"),
        }
    }

    /// The open-folder action of a received notification.
    fn reveal_received_folder(&self, id: &str, user_info: &HashMap<String, String>) {
        match user_info.get(USER_INFO_FOLDER) {
            Some(folder) => self.emit(FileTransferEvent::Reveal { path: PathBuf::from(folder) }),
            None => warn!(id, "received notification without its folder"),
        }
    }
}

/// A response no notification of the feature offers: its kind, and the action it names as it came.
fn log_unexpected_response(id: &str, response: &NotificationResponse) {
    let (response_kind, raw_action) = match response {
        NotificationResponse::Activated => ("activated", None),
        NotificationResponse::Action { action_id } => ("action", Some(action_id.as_str())),
        // The reply's text is the user's; only its action is logged.
        NotificationResponse::Reply { action_id, .. } => ("reply", Some(action_id.as_str())),
        NotificationResponse::Dismissed => ("dismissed", None),
    };
    warn!(id, response_kind, raw_action, "unexpected response to a notification; ignored");
}

/// The file-transfer responder of [`phonelink_features::NotificationRouter`].
pub(super) struct Responder(pub(super) Weak<Inner>);

impl NotificationResponder for Responder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        if let Some(inner) = self.0.upgrade() {
            inner.respond(id, user_info, response);
        }
    }
}
