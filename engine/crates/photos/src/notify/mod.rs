//! The new-photo notification: a `UserNotifications` notification whose activation opens the full
//! image in the default macOS viewer (Phone Link on Windows opens it in Snipping Tool).
//!
//! For each processed item batch, while the PC-side setting and the photos consent are on, the 3 most
//! recent items by taken time are considered; one whose taken and last-modified times are within
//! 120 s of each other and of now, and which carries an inline thumbnail, is notified (thumbnail as
//! the image). A notification within 7 s of the previous pop-up is delivered silently. Activating it
//! fetches the full image (waiting at most 30 s) and asks the UI to open it
//! ([`PhotosEvent::OpenImage`]).
//!
//! A phone whose new photos the remote-snipping feature notifies gets no notification from here
//! ([`NewPhotoGate`]), so one photo never shows two (Phone Link on Windows runs both independently).

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use tracing::{debug, error, info, warn};

use phonelink_core::types::random_hex;
use phonelink_features::shared::notifier::{NotificationResponder, NotificationResponse, SystemNotification};

use super::store::{self, PhotoRow};
use super::wire::MediaItem;
use super::{PhoneState, PhotosEvent, PhotosInner};
use phonelink_core::sync::MutexExt;

/// `user_info` key of the phone (`<dcgClientId>.<Environment>`).
pub const USER_INFO_PHONE: &str = "phone";
/// `user_info` key of the media id.
pub const USER_INFO_MEDIA_ID: &str = "mediaId";

/// |taken − modified| < window, now − taken < window and now − modified < window (all Unix ms).
pub fn is_new_photo(item: &MediaItem, now_ms: i64, window_ms: i64) -> bool {
    let (taken, modified, now, window) =
        (i128::from(item.taken_ms), i128::from(item.modified_ms), i128::from(now_ms), i128::from(window_ms));
    (taken - modified).abs() < window && now - taken < window && now - modified < window
}

impl PhotosInner {
    /// Runs the heuristic on one processed item batch (the batch's non-removed items).
    pub(crate) fn notify_new_photos(&self, ps: &PhoneState, items: &[MediaItem]) {
        if items.is_empty() {
            return;
        }
        if !self.new_photo_notifications_on(ps) {
            return;
        }

        let now = self.now_ms();
        let window = i64::try_from(self.config.new_photo_window.as_millis()).unwrap_or(i64::MAX);
        let silent_window = i64::try_from(self.config.new_photo_silent_window.as_millis()).unwrap_or(i64::MAX);

        let mut recent: Vec<&MediaItem> = items.iter().collect();
        // Stable: equal taken times keep the batch order.
        recent.sort_by_key(|i| std::cmp::Reverse(i.taken_ms));

        for item in recent.into_iter().take(self.config.new_photo_candidates) {
            if !is_new_photo(item, now, window) {
                continue;
            }
            let Some(thumbnail) = &item.thumbnail else {
                continue;
            };
            if !self.notifier.is_authorized() {
                debug!(phone = %ps.phone, "new-photo notification not posted; notifications are not authorized");
                continue;
            }
            self.post_new_photo(ps, item, thumbnail, now, silent_window);
        }
    }

    /// The PC-side setting and the photos consent are on, and remote snipping does not notify the
    /// phone's new photos.
    fn new_photo_notifications_on(&self, ps: &PhoneState) -> bool {
        let settings = match store::state(&ps.db) {
            Ok(s) => s,
            Err(e) => {
                error!(phone = %ps.phone, error = %e, "photos store unreadable; no new-photo notification");
                return false;
            }
        };
        if !settings.consent || !settings.new_photo_notifications {
            return false;
        }
        if self.new_photo_gate.get().is_some_and(|gate| gate.covered(&ps.phone)) {
            debug!(phone = %ps.phone, "new-photo notifications of this phone come from remote snipping; none posted here");
            return false;
        }
        true
    }

    /// Posts the notification of `item`, silently within `silent_window` of the previous pop-up.
    fn post_new_photo(&self, ps: &PhoneState, item: &MediaItem, thumbnail: &[u8], now: i64, silent_window: i64) {
        let silent = ps
            .ui
            .lock_or_panic()
            .last_popup_ms
            .is_some_and(|t| i128::from(now) - i128::from(t) < i128::from(silent_window));

        let subtitle = self
            .app
            .upgrade()
            .and_then(|app| app.phone_record())
            .filter(|r| r.phone() == ps.phone)
            .and_then(|r| r.display_name.or(r.name));

        let notification = SystemNotification {
            id: format!("{}/{}/{}/{}", store::FEATURE, ps.phone, item.id, random_hex(8)),
            thread_id: Some(format!("{}/{}", store::FEATURE, ps.phone)),
            title: "New photo".to_string(),
            subtitle,
            body: item.name.clone(),
            alert: !silent,
            actions: Vec::new(),
            user_info: HashMap::from([
                (USER_INFO_PHONE.to_string(), ps.phone.to_string()),
                (USER_INFO_MEDIA_ID.to_string(), item.id.to_string()),
            ]),
            image: Some(thumbnail.to_vec()),
        };

        match self.notifier.post(notification) {
            Ok(()) => {
                if !silent {
                    ps.ui.lock_or_panic().last_popup_ms = Some(now);
                }
                info!(phone = %ps.phone, id = item.id, silent, "new-photo notification posted");
            }
            Err(e) => warn!(phone = %ps.phone, error = %e, "new-photo notification not posted"),
        }
    }

    /// Activation of a new-photo notification: while the photos consent and the setting are on and
    /// the item is still cached, fetch its full bytes (waiting at most 30 s) and ask the UI to open
    /// them.
    pub(crate) async fn activate_new_photo(self: Arc<Self>, phone: &str, id: i64) {
        let Some(app) = self.app.upgrade() else { return };
        let Some(selected) = app.selected_phone().filter(|p| p.to_string() == phone) else {
            info!(phone, "notification activation ignored; its phone is not selected");
            return;
        };

        let ps = match self.phone_state(&selected) {
            Ok(ps) => ps,
            Err(e) => {
                error!(phone, error = %e, "photos store unavailable; notification activation ignored");
                return;
            }
        };

        let Some(item) = activation_item(&ps, phone, id) else { return };
        if !item.has_full {
            // Not a gallery trigger: no pending mark, so closing the viewer or scrolling does not
            // cancel it; named after Phone Link's toast click.
            self.request_full(&ps, &[id], super::wire::trace_name::TOAST_CLICK);
            if self.wait_for_full(&ps, id, self.config.full_bytes_wait).await.is_none() {
                warn!(phone, id, "notification activation failed; the full image did not arrive in time");
                return;
            }
        }

        self.emit(PhotosEvent::OpenImage { phone: selected, id });
    }
}

/// The cached item `id` a notification activation opens, while the photos consent and the setting are
/// on; `None` (logged) otherwise.
fn activation_item(ps: &PhoneState, phone: &str, id: i64) -> Option<PhotoRow> {
    let (settings, item) = match (store::state(&ps.db), store::item(&ps.db, id)) {
        (Ok(s), Ok(i)) => (s, i),
        (Err(e), _) | (_, Err(e)) => {
            error!(phone, error = %e, "photos store unreadable; notification activation ignored");
            return None;
        }
    };

    if !settings.consent || !settings.new_photo_notifications {
        info!(phone, "notification activation ignored; photos consent or new-photo notifications are off");
        return None;
    }
    let Some(item) = item else {
        info!(phone, id, "notification activation ignored; the item is no longer cached");
        return None;
    };
    Some(item)
}

/// The photos responder of [`phonelink_features::NotificationRouter`].
pub(crate) struct PhotosResponder {
    pub(crate) inner: Weak<PhotosInner>,
}

impl NotificationResponder for PhotosResponder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        if response != NotificationResponse::Activated {
            return;
        }

        let Some(inner) = self.inner.upgrade() else { return };
        let phone = user_info.get(USER_INFO_PHONE).cloned();
        let media_id = user_info.get(USER_INFO_MEDIA_ID).and_then(|s| s.parse::<i64>().ok());
        let (Some(phone), Some(media_id)) = (phone, media_id) else {
            warn!(id, "photos notification response without its phone or media id; ignored");
            return;
        };

        let runtime = inner.runtime.clone();
        runtime.spawn(async move { inner.activate_new_photo(&phone, media_id).await });
    }
}

#[cfg(test)]
mod tests;
