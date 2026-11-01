//! The new-photo notification: a `UserNotifications` notification with the photo's thumbnail, whose
//! activation fetches the photo and asks the UI to open it (Phone Link on Windows opens it in Snipping
//! Tool's editor).
//!
//! For the first item of a `NEW_PHOTO` publication, in order: the rate limit (one attempt per second
//! over every phone, taken before the window check), the window (last-modified less than 120 s before
//! now on the PC's wall clock, a time in the future included), the app may post notifications, the
//! thumbnail fetch, silent delivery within 7 s of the phone's previous pop-up. A posted notification
//! is withdrawn after 5 minutes.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::SystemTime;

use tracing::{debug, info, warn};

use super::wire::{self, Fetch};
use super::{FEATURE, FetchError, Inner, RemoteSnippingEvent, Shown, key};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::notifier::{NotificationResponder, NotificationResponse, SystemNotification};
use phonelink_proto::remotesnipping::v1::MediaMetadata;
use phonelink_transport::auth::PeerDevice;

/// `user_info` key of the phone (`<dcgClientId>.<Environment>`).
pub(super) const USER_INFO_PHONE: &str = "phone";
/// `user_info` key of the media id.
pub(super) const USER_INFO_MEDIA_ID: &str = "mediaId";

pub(super) const TITLE: &str = "New photo";
pub(super) const BODY: &str = "Select here to view, edit, or share the photo";
/// The subtitle when the phone has no name.
pub(super) const DEVICE_LABEL: &str = "Device";

/// The id of `phone`'s notification of `media_id`: a second notification of the same photo replaces
/// the first.
pub(super) fn notification_id(phone: &DcgClient, media_id: i64) -> String {
    format!("{FEATURE}/{}/{media_id}", key(phone))
}

impl Inner {
    /// The notification of one new photo.
    pub(super) async fn notify_new_photo(self: Arc<Self>, peer: PeerDevice, item: MediaMetadata) {
        let phone = peer.client.clone();
        let media_id = item.media_id;
        let now = self.env.edges.clock.now();
        let now_ns = wire::system_time_ns(now);

        if !self.take_attempt(&phone, media_id, now) {
            return;
        }
        if !self.photo_qualifies(&phone, &item, now_ns) {
            return;
        }

        let Some(thumbnail) = self.fetch_thumbnail(&peer, media_id).await else {
            return;
        };

        let now = self.env.edges.clock.now();
        let silent = self.within_silent_window(&phone, now);
        let id = notification_id(&phone, media_id);
        let notification = new_photo_notification(&peer, &id, media_id, silent, thumbnail.clone());
        if let Err(e) = self.env.notifier.post(notification) {
            warn!(%phone, media_id, error = %e, "new-photo notification not posted");
            return;
        }
        info!(%phone, media_id, silent, "new-photo notification posted");

        if !silent {
            self.update_logged(&phone, "the last pop-up", |d| d.last_popup = Some(now));
        }

        let generation = self.record_shown(&id, &phone, media_id, thumbnail);
        self.spawn_expiry(id, generation);
    }

    /// The rate limit: records the attempt at `now`, `false` (logged) when the previous attempt over
    /// every phone is too recent.
    fn take_attempt(&self, phone: &DcgClient, media_id: i64, now: SystemTime) -> bool {
        let now_ns = wire::system_time_ns(now);
        let mut state = self.state.lock_or_panic();

        if state
            .last_attempt
            .is_some_and(|last| wire::within(now_ns, wire::system_time_ns(last), self.config.rate_limit))
        {
            debug!(%phone, media_id, "new photo within the rate limit; no notification posted");
            return false;
        }
        state.last_attempt = Some(now);
        true
    }

    /// The window and the notification authorization; `false` (logged) when the photo is not
    /// recent or notifications may not be posted.
    fn photo_qualifies(&self, phone: &DcgClient, item: &MediaMetadata, now_ns: i128) -> bool {
        let media_id = item.media_id;
        let Some(modified_ns) = wire::date_modified_ns(item) else {
            warn!(%phone, media_id, "new photo without a valid last-modified time; no notification posted");
            return false;
        };

        if !wire::within(now_ns, modified_ns, self.config.recent_window) {
            debug!(%phone, media_id, "new photo not recent; no notification posted");
            return false;
        }
        if !self.env.notifier.is_authorized() {
            info!(%phone, media_id, "notifications not authorized; no new-photo notification posted");
            return false;
        }
        true
    }

    /// The photo's thumbnail, `None` (logged) when none or an empty one came back.
    async fn fetch_thumbnail(&self, peer: &PeerDevice, media_id: i64) -> Option<Vec<u8>> {
        let fetched = match self.fetch(peer, Fetch::Thumbnail, media_id).await {
            Ok(bytes) if bytes.is_empty() => Err(FetchError::EmptyThumbnail),
            fetched => fetched,
        };
        fetched.map_err(|e| log_no_thumbnail(&peer.client, media_id, &e)).ok()
    }

    /// Whether `now` is within the silent window of the phone's previous pop-up; an unreadable store
    /// (logged) counts as no previous pop-up.
    fn within_silent_window(&self, phone: &DcgClient, now: SystemTime) -> bool {
        let last_popup = match self.data(phone) {
            Ok(d) => d.last_popup,
            Err(e) => {
                warn!(%phone, error = %e, "remote-snipping store unreadable; the last pop-up is unknown");
                None
            }
        };

        last_popup.is_some_and(|last| {
            wire::within(wire::system_time_ns(now), wire::system_time_ns(last), self.config.silent_window)
        })
    }

    /// Records a posted notification as shown; returns its generation.
    fn record_shown(&self, id: &str, phone: &DcgClient, media_id: i64, thumbnail: Vec<u8>) -> u64 {
        let mut state = self.state.lock_or_panic();
        state.next_generation += 1;
        let generation = state.next_generation;
        state.shown.insert(id.to_owned(), Shown { phone: phone.clone(), media_id, thumbnail, generation });
        generation
    }

    /// Withdraws the notification after the expiry unless it was reposted, activated or dismissed.
    fn spawn_expiry(&self, id: String, generation: u64) {
        let (weak, shutdown, expiry) = (self.weak.clone(), self.shutdown.clone(), self.config.expiry);
        self.runtime.spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(expiry) => {}
            }

            let Some(inner) = weak.upgrade() else { return };
            let expired = {
                let mut state = inner.state.lock_or_panic();
                match state.shown.get(&id) {
                    Some(shown) if shown.generation == generation => state.shown.remove(&id).is_some(),
                    _ => false,
                }
            };

            if expired {
                debug!(id, "new-photo notification expired");
                if let Err(e) = inner.env.notifier.withdraw(vec![id]) {
                    warn!(error = %e, "expired new-photo notification not withdrawn");
                }
            }
        });
    }

    /// The user clicked the notification `id`: fetch the photo, fall back to its thumbnail, and ask
    /// the UI to open the bytes. No setting is re-checked.
    async fn activate(self: Arc<Self>, id: String, user_info: HashMap<String, String>) {
        let shown = self.state.lock_or_panic().shown.remove(&id);
        let Some((phone_text, media_id)) = activated_photo(&id, shown.as_ref(), &user_info) else {
            return;
        };

        let peer = self.env.peers.peers().into_iter().find(|p| key(&p.client).to_string() == phone_text);
        let photo = match &peer {
            Some(peer) => self.fetch(peer, Fetch::Photo, media_id).await,
            None => Err(FetchError::UnknownPhone),
        };

        // The thumbnail it was shown with stands in for a photo that does not arrive.
        let (bytes, photo_failure) = match (photo, &shown) {
            (Ok(bytes), _) => (bytes, None),
            (Err(e), Some(s)) => (s.thumbnail.clone(), Some(e)),
            (Err(e), None) => {
                warn!(phone = phone_text, media_id, error = %e, "activated new photo not fetched and no thumbnail is kept; nothing opens");
                return;
            }
        };
        let phone = match (&peer, &shown) {
            (Some(p), _) => p.client.clone(),
            (None, Some(s)) => s.phone.clone(),
            (None, None) => return,
        };

        log_activation(&phone, media_id, bytes.len(), photo_failure.as_ref());
        self.emit(RemoteSnippingEvent::OpenImage { phone, media_id, bytes });
    }

    fn dismissed(&self, id: &str) {
        if self.state.lock_or_panic().shown.remove(id).is_some() {
            debug!(id, "new-photo notification dismissed");
        }
    }
}

/// The line of a new photo whose thumbnail did not arrive: no notification is posted, nothing retried.
fn log_no_thumbnail(phone: &DcgClient, media_id: i64, e: &FetchError) {
    match e {
        FetchError::NotEnrolled => {
            debug!(%phone, media_id, error = %e, "new photo's thumbnail not fetched; no notification posted");
        }
        e => warn!(%phone, media_id, error = %e, "new photo's thumbnail not fetched; no notification posted"),
    }
}

/// The line of an activation that opens an image: the photo, or its thumbnail when the photo did
/// not arrive (`photo_failure`).
fn log_activation(phone: &DcgClient, media_id: i64, image_bytes: usize, photo_failure: Option<&FetchError>) {
    match photo_failure {
        None => info!(%phone, media_id, image_bytes, "new-photo notification activated; the photo opens"),
        Some(e) => {
            warn!(%phone, media_id, image_bytes, error = %e, "activated new photo not fetched; its thumbnail opens")
        }
    }
}

/// The phone (as its key's text) and media id of an activated notification: those it was shown
/// with, else its user info; `None` (logged) when the user info lacks either.
fn activated_photo(id: &str, shown: Option<&Shown>, user_info: &HashMap<String, String>) -> Option<(String, i64)> {
    if let Some(s) = shown {
        return Some((key(&s.phone).to_string(), s.media_id));
    }

    let phone = user_info.get(USER_INFO_PHONE).cloned();
    let media_id = user_info.get(USER_INFO_MEDIA_ID).and_then(|m| m.parse::<i64>().ok());
    let (Some(phone), Some(media_id)) = (phone, media_id) else {
        warn!(id, "new-photo notification response without its phone or media id; ignored");
        return None;
    };
    Some((phone, media_id))
}

/// The new-photo notification of `media_id` from `peer`, with its thumbnail.
fn new_photo_notification(
    peer: &PeerDevice,
    id: &str,
    media_id: i64,
    silent: bool,
    thumbnail: Vec<u8>,
) -> SystemNotification {
    let phone = &peer.client;
    let subtitle =
        peer.info.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(DEVICE_LABEL).to_owned();

    SystemNotification {
        id: id.to_owned(),
        thread_id: Some(format!("{FEATURE}/{}", key(phone))),
        title: TITLE.to_owned(),
        subtitle: Some(subtitle),
        body: BODY.to_owned(),
        alert: !silent,
        actions: Vec::new(),
        user_info: HashMap::from([
            (USER_INFO_PHONE.to_owned(), key(phone).to_string()),
            (USER_INFO_MEDIA_ID.to_owned(), media_id.to_string()),
        ]),
        image: Some(thumbnail),
    }
}

/// The remote-snipping responder of [`phonelink_features::NotificationRouter`].
pub(super) struct Responder(pub(super) Weak<Inner>);

impl NotificationResponder for Responder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let Some(inner) = self.0.upgrade() else { return };
        match response {
            NotificationResponse::Activated => {
                let (runtime, id, user_info) = (inner.runtime.clone(), id.to_owned(), user_info.clone());
                runtime.spawn(async move { inner.activate(id, user_info).await });
            }
            NotificationResponse::Dismissed => inner.dismissed(id),
            NotificationResponse::Action { .. } | NotificationResponse::Reply { .. } => {
                warn!(id, "an action on a new-photo notification, which offers none; ignored");
            }
        }
    }
}
