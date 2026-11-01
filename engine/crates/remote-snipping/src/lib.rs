//! New-photo notifications (remote snipping), a Cross Device Experience Host feature on the CDEH
//! identity's connection.
//!
//! It covers the phone publishes MSAEP tag 6 when a photo is taken, the PC fetches its thumbnail
//! with `/DeviceResourceManager` `GET` by type `MEDIA` and posts a notification, and clicking it
//! fetches the photo and asks the UI to open it.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the `MEDIA` and `PERMISSIONS` requests, their replies, the tag-6 and tag-3 publications |
//! | [`store`] | the per-phone state (capability, setting, permission, last pop-up, capability answer) |
//! | `notify` | the notification: rate limit, window, thumbnail, silent pop-up, expiry, activation |
//! | this module | the option, the subscriptions, the capability request and its triggers, tag 3, the settings and the permission request, the gate of the photos feature's own new-photo notification |
//!
//! The photos feature posts its own new-photo notification from the gallery sync. Phone Link on
//! Windows runs both independently, so one photo can show two; here the photos notification is
//! posted only for a phone this feature does not cover ([`RemoteSnipping::covers`]).

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod store;
pub mod wire;

mod notify;

use phonelink_core::config::features::RemoteSnippingConfig;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use phonelink_core::seams::SessionEvent;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::NewPhotoGate;
use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::{FeatureEnv, FeatureError, PhoneSwitchChanged};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_proto::remotesnipping::v1::MediaMetadata;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use phonelink_transport::platform::{
    DeviceFilter, Drm, DrmAddress, DrmSendOutcome, DrmSendRequest, Msaep, Publication, PublicationObserver,
    Subscription,
};

pub use store::{CapabilityAnswer, FEATURE, PhoneData};
pub use wire::PermissionOutcome;

use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;
use wire::Fetch;

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteSnippingEvent {
    /// What [`RemoteSnipping::settings`] shows for `phone` may have changed.
    Changed { phone: DcgClient },
    /// The user clicked a new-photo notification: open `bytes` (the photo, else its thumbnail).
    OpenImage { phone: DcgClient, media_id: i64, bytes: Vec<u8> },
}

/// The feature's settings for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhoneSettings {
    /// The phone supports new-photo notifications: the toggle is shown.
    pub applicable: bool,
    /// "Get new photo notifications" (default on).
    pub enabled: bool,
    /// The phone has not reported photo access: the "Allow permissions" link is shown.
    pub needs_permission: bool,
    /// The permission request runs ("Finishing up").
    pub requesting_permission: bool,
    /// This feature covers the phone's new-photo notifications, so the photos feature posts none.
    pub covers_new_photo_notifications: bool,
}

/// An operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum RemoteSnippingError {
    /// The phone is not in the account's device list.
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// Why a thumbnail or photo fetch brought no image.
#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("no current account")]
    NotEnrolled,
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    #[error("the request failed: {0:?}")]
    Request(DrmSendOutcome),
    #[error("the reply does not parse: {0}")]
    Reply(#[source] prost::DecodeError),
    /// The phone answered without the image; `status` is the reply's.
    #[error("the reply carries no image (status {status})")]
    NoImage { status: i32 },
    #[error("the thumbnail is empty")]
    EmptyThumbnail,
}

/// A phone's map and store key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// A notification on screen.
#[derive(Debug, Clone)]
struct Shown {
    phone: DcgClient,
    media_id: i64,
    /// Kept for the activation's fallback when the photo does not arrive.
    thumbnail: Vec<u8>,
    /// Tells a repost of the same id from the expiry of an earlier one.
    generation: u64,
}

#[derive(Debug, Default)]
struct State {
    /// The last notification attempt (wall clock), over every phone.
    last_attempt: Option<SystemTime>,
    /// Phones whose new-photo publication was handled since the feature started.
    published: HashSet<DcgClient>,
    /// By notification id.
    shown: HashMap<String, Shown>,
    next_generation: u64,
    requesting: HashSet<DcgClient>,
}

/// The feature's shared state; tasks and callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    config: RemoteSnippingConfig,
    drm: Arc<Drm>,
    store: PhoneStores,
    state: Mutex<State>,
    /// Serializes read-modify-write of a phone's row.
    data_lock: Mutex<()>,
    events: broadcast::Sender<RemoteSnippingEvent>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscriptions: Mutex<Vec<Subscription>>,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The new-photo notifications feature.
pub struct RemoteSnipping {
    inner: Arc<Inner>,
}

impl RemoteSnipping {
    /// Installs the feature on the CDEH identity's `env`, `msaep` and `drm`: the notification responder;
    /// then, while the option is on, the tag-6 and tag-3 subscribers, the trigger loop and the
    /// capability request of every phone that is available now. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: RemoteSnippingConfig,
        msaep: Arc<Msaep>,
        drm: Arc<Drm>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current().map_err(|_| {
            FeatureError::Programming("remote snipping must be installed inside a tokio runtime".into())
        })?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            drm,
            store: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
            state: Mutex::new(State::default()),
            data_lock: Mutex::new(()),
            events: broadcast::channel(64).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscriptions: Mutex::new(Vec::new()),
            weak: weak.clone(),
        });

        env.responses
            .register(FEATURE, Arc::new(notify::Responder(Arc::downgrade(&inner))))
            .map_err(RegistrationError::from)?;

        if !inner.config.enabled {
            info!("feature off by configuration; no subscription, no request");
            return Ok(Arc::new(Self { inner }));
        }

        let triggers = Triggers {
            peers: env.peers.subscribe(),
            sessions: env.sessions.subscribe(),
            switches: env.switches.subscribe(),
        };
        let new_photo =
            msaep.subscribe(wire::NEW_PHOTO_TAG, DeviceFilter::Any, Arc::new(NewPhotoObserver(Arc::downgrade(&inner))));
        let permissions = msaep.subscribe(
            wire::PERMISSIONS_TAG,
            DeviceFilter::Any,
            Arc::new(PermissionObserver(Arc::downgrade(&inner))),
        );

        inner.subscriptions.lock_or_panic().extend([new_photo, permissions]);
        inner.spawn_loop(triggers);
        inner.start_for_available_phones();
        info!("feature installed");
        Ok(Arc::new(Self { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<RemoteSnippingEvent> {
        self.inner.events.subscribe()
    }

    /// The gate the photos feature's new-photo notification consults.
    pub fn gate(&self) -> Arc<dyn NewPhotoGate> {
        Arc::new(Gate(Arc::downgrade(&self.inner)))
    }

    /// Whether this feature covers `phone`'s new-photo notifications (the photos feature then posts
    /// none): unless the option is off, the phone is disabled, or the phone answered the capability
    /// request without new-media notifications and has published no new photo since the feature
    /// started. A phone that never answered is covered.
    pub fn covers(&self, phone: &DcgClient) -> bool {
        self.inner.covers(phone)
    }

    /// `phone`'s settings.
    pub fn settings(&self, phone: &DcgClient) -> Result<PhoneSettings, RemoteSnippingError> {
        let data = self.inner.data(phone)?;
        let requesting = self.inner.state.lock_or_panic().requesting.contains(&key(phone));
        Ok(PhoneSettings {
            applicable: data.capable,
            enabled: data.enabled,
            needs_permission: !data.has_permissions,
            requesting_permission: requesting,
            covers_new_photo_notifications: self.inner.covers(phone),
        })
    }

    /// "Get new photo notifications" for `phone`. Turning it off stops later notifications; those
    /// shown stay.
    pub fn set_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), RemoteSnippingError> {
        let changed = self.inner.update(phone, |d| d.enabled = enabled)?;
        if changed {
            info!(%phone, enabled, "new-photo notifications setting changed");
            self.inner.changed(phone);
        }
        Ok(())
    }

    /// The "Allow permissions" link: asks the phone for photo access (`SYNC` by type `PERMISSIONS`).
    pub async fn request_permission(&self, phone: &DcgClient) -> Result<PermissionOutcome, RemoteSnippingError> {
        let peer = self.inner.env.peers.peer(phone).ok_or(RemoteSnippingError::UnknownPhone)?;
        Ok(self.inner.clone().request_permission(peer).await)
    }
}

/// The trigger streams, subscribed before any state is read.
struct Triggers {
    peers: broadcast::Receiver<PeerEvent>,
    sessions: broadcast::Receiver<SessionEvent>,
    switches: broadcast::Receiver<PhoneSwitchChanged>,
}

impl Inner {
    fn emit(&self, event: RemoteSnippingEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn changed(&self, phone: &DcgClient) {
        self.emit(RemoteSnippingEvent::Changed { phone: phone.clone() });
    }

    fn data(&self, phone: &DcgClient) -> Result<PhoneData, StoreError> {
        store::load(&*self.store.get(&key(phone))?)
    }

    /// Applies `f` to `phone`'s row; `true` when it changed.
    fn update(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneData)) -> Result<bool, StoreError> {
        let _guard = self.data_lock.lock_or_panic();
        let db = self.store.get(&key(phone))?;
        let before = store::load(&db)?;
        let mut after = before;
        f(&mut after);
        if after != before {
            store::save(&db, after)?;
        }
        Ok(after != before)
    }

    /// [`Inner::update`] for the background paths: a store failure is logged; a change of what the
    /// settings show is announced.
    fn update_logged(&self, phone: &DcgClient, what: &str, f: impl FnOnce(&mut PhoneData)) {
        match self.update(phone, f) {
            Ok(true) => self.changed(phone),
            Ok(false) => {}
            Err(e) => error!(%phone, error = %e, what, "remote-snipping state not stored"),
        }
    }

    fn phone_enabled(&self, phone: &DcgClient) -> Result<bool, StoreError> {
        self.env.switches.is_enabled(phone)
    }

    fn covers(&self, phone: &DcgClient) -> bool {
        if !self.config.enabled {
            return false;
        }

        match self.phone_enabled(phone) {
            Ok(true) => {}
            Ok(false) => return false,
            Err(e) => {
                warn!(%phone, error = %e, "phone switch unreadable; new-photo notifications counted as covered");
                return true;
            }
        }

        match self.data(phone) {
            Ok(PhoneData { capability_answer: Some(CapabilityAnswer::NotCapable), .. }) => {
                self.state.lock_or_panic().published.contains(&key(phone))
            }
            Ok(_) => true,
            Err(e) => {
                warn!(%phone, error = %e, "remote-snipping store unreadable; new-photo notifications counted as covered");
                true
            }
        }
    }

    // --------------------------------------------------------------------------------------------
    // Triggers and the capability request
    // --------------------------------------------------------------------------------------------

    fn spawn_loop(&self, mut t: Triggers) {
        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = t.peers.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PeerEvent::Added(peer)) => inner.run_manager(&peer, true, "phone added"),
                            Ok(PeerEvent::Removed(phone)) => inner.phone_removed(&phone),
                            // Only availability and the enabled flag re-run a manager.
                            Ok(PeerEvent::Updated(_)) => {}
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(skipped = n, "missed device-list events; resyncing the available phones");
                                inner.start_for_available_phones();
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = t.sessions.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(SessionEvent::Connected(phone)) => match inner.env.peers.peer(&phone) {
                                Some(peer) => inner.run_manager(&peer, false, "phone available"),
                                None => debug!(%phone, "available partner not in the account's device list"),
                            },
                            // A change to unavailable does nothing.
                            Ok(SessionEvent::Disconnected { .. }) => {}
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(skipped = n, "missed session events; resyncing the available phones");
                                inner.start_for_available_phones();
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = t.switches.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PhoneSwitchChanged { phone, .. }) => {
                                inner.changed(&phone);
                                let available = inner.env.sessions.is_connected(&phone);
                                match inner.env.peers.peer(&phone) {
                                    Some(peer) if available => inner.run_manager(&peer, false, "phone switch changed"),
                                    _ => {}
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(skipped = n, "missed phone-switch events; resyncing the available phones");
                                inner.start_for_available_phones();
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    /// Every phone of the account that is available now, without a forced session start.
    fn start_for_available_phones(&self) {
        for peer in self.env.peers.peers() {
            if self.env.sessions.is_connected(&peer.client) {
                self.run_manager(&peer, false, "available at start");
            }
        }
    }

    /// A phone's manager: an enabled Android phone gets the capability request.
    fn run_manager(&self, peer: &PeerDevice, force_session_start: bool, trigger: &'static str) {
        let phone = &peer.client;
        if !self.env.peers.is_enrolled() {
            debug!(%phone, trigger, "no current account; no capability request");
            return;
        }
        if !self.capability_switch_on(phone, trigger) {
            return;
        }
        if !peer.info.is_android() {
            debug!(%phone, trigger, "not an Android phone; no capability request");
            return;
        }

        info!(%phone, trigger, force_session_start, "asking the phone for its new-media capability");
        let (weak, peer) = (self.weak.clone(), peer.clone());
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            inner.request_capability(&peer, force_session_start).await;
        });
    }

    /// The phone's enabled switch for a capability request, `false` (logged) when it is off or
    /// unreadable.
    fn capability_switch_on(&self, phone: &DcgClient, trigger: &'static str) -> bool {
        match self.phone_enabled(phone) {
            Ok(true) => true,
            Ok(false) => {
                debug!(%phone, trigger, "phone disabled; no capability request");
                false
            }
            Err(e) => {
                warn!(%phone, trigger, error = %e, "phone switch unreadable; no capability request");
                false
            }
        }
    }

    /// Sends the capability request and records its outcome.
    async fn request_capability(&self, peer: &PeerDevice, force_session_start: bool) {
        let outcome = self
            .send(
                peer,
                ProtoDeviceResourceType::Media,
                ProtoDeviceResourceRequestType::Get,
                wire::capabilities_request(),
                force_session_start,
                wire::trace_name::CAPABILITIES,
            )
            .await;

        self.capability_outcome(&peer.client, outcome);
    }

    fn capability_outcome(&self, phone: &DcgClient, outcome: DrmSendOutcome) {
        match outcome {
            DrmSendOutcome::Success { payload } => self.capability_reply(phone, &payload),
            DrmSendOutcome::UnsupportedResourceType => {
                info!(%phone, "the phone has no MEDIA handler; it does not notify new photos");
                self.update_logged(phone, "the capability answer", |d| {
                    d.capability_answer = Some(CapabilityAnswer::NotCapable);
                });
            }
            other => {
                warn!(%phone, outcome = ?other, error = other.cause(), "capability request failed; state kept, no retry")
            }
        }
    }

    /// Records the capability reply's answer; a reply that does not parse keeps the state.
    fn capability_reply(&self, phone: &DcgClient, payload: &[u8]) {
        match wire::parse_media_reply(payload) {
            Ok(reply) => {
                info!(%phone, capable = reply.new_media_notifications, has_permissions = reply.has_permissions, status = reply.status, "capability request answered");
                let capable = reply.new_media_notifications;
                self.update_logged(phone, "the capability reply", |d| {
                    d.capable = capable;
                    d.has_permissions = reply.has_permissions;
                    d.capability_answer =
                        Some(if capable { CapabilityAnswer::Capable } else { CapabilityAnswer::NotCapable });
                });
            }
            Err(e) => warn!(%phone, error = %e, "capability reply does not parse; state kept"),
        }
    }

    /// One request through the CDEH request wrapper, addressed by type, bounded by the platform
    /// request timeout, cancelled only by the feature's shutdown.
    async fn send(
        &self,
        peer: &PeerDevice,
        resource_type: ProtoDeviceResourceType,
        request_type: ProtoDeviceResourceRequestType,
        payload: Vec<u8>,
        force_session_start: bool,
        names: (&str, &str),
    ) -> DrmSendOutcome {
        // The wrapper refuses a disabled phone; an unreadable switch counts as off.
        let phone_enabled = self.phone_enabled(&peer.client).unwrap_or_else(|e| {
            warn!(phone = %peer.client, error = %e, "phone switch unreadable; the request is refused");
            false
        });

        self.drm
            .send(DrmSendRequest {
                target: peer.client.clone(),
                address: DrmAddress::Type(resource_type),
                request_type,
                payload,
                client_version: peer.info.client_version.clone(),
                phone_enabled,
                force_session_start,
                timeout: self.env.config.platform.request_timeout,
                trace: TraceContext::new_root(phonelink_core::types::trace_state(names.0, names.1)),
                cancel: self.shutdown.child_token(),
            })
            .await
    }

    /// A thumbnail or photo fetch (forced session start); every reply that parses records the
    /// phone's `has_permissions`. The caller logs a failure with what it does instead.
    async fn fetch(&self, peer: &PeerDevice, fetch: Fetch, media_id: i64) -> Result<Vec<u8>, FetchError> {
        let phone = &peer.client;
        if !self.env.peers.is_enrolled() {
            return Err(FetchError::NotEnrolled);
        }

        let outcome = self
            .send(
                peer,
                ProtoDeviceResourceType::Media,
                ProtoDeviceResourceRequestType::Get,
                wire::fetch_request(fetch, media_id),
                true,
                fetch.trace_names(),
            )
            .await;

        let DrmSendOutcome::Success { payload } = outcome else {
            return Err(FetchError::Request(outcome));
        };
        self.fetched_bytes(phone, fetch, &payload)
    }

    /// The image of a fetch reply; a reply that parses records the phone's `has_permissions`.
    fn fetched_bytes(&self, phone: &DcgClient, fetch: Fetch, payload: &[u8]) -> Result<Vec<u8>, FetchError> {
        let reply = wire::parse_media_reply(payload).map_err(FetchError::Reply)?;
        let (has_permissions, status) = (reply.has_permissions, reply.status);
        self.update_logged(phone, "has_permissions of a fetch reply", |d| d.has_permissions = has_permissions);
        reply.bytes(fetch).ok_or(FetchError::NoImage { status })
    }

    fn phone_removed(&self, phone: &DcgClient) {
        let ids: Vec<String> = {
            let mut state = self.state.lock_or_panic();
            let k = key(phone);
            state.published.remove(&k);
            state.requesting.remove(&k);
            let ids: Vec<String> =
                state.shown.iter().filter(|(_, s)| key(&s.phone) == k).map(|(id, _)| id.clone()).collect();
            for id in &ids {
                state.shown.remove(id);
            }
            ids
        };

        if !ids.is_empty() {
            info!(%phone, count = ids.len(), "phone left the account; its new-photo notifications are withdrawn");
            if let Err(e) = self.env.notifier.withdraw(ids) {
                warn!(%phone, error = %e, "new-photo notifications not withdrawn");
            }
        }
        self.changed(phone);
    }

    // --------------------------------------------------------------------------------------------
    // Publications
    // --------------------------------------------------------------------------------------------

    /// Tag 6. Runs on the router's task: checks and parses, then spawns the notification.
    fn on_new_photo(&self, publication: &Publication) {
        let Some(peer) = self.env.peers.peer(&publication.source) else {
            warn!(partner = %publication.source, "new-photo publication from a device not in the account; dropped");
            return;
        };

        let phone = &peer.client;
        if !self.new_photos_on(phone) {
            return;
        }
        let Some(item) = new_photo_item(phone, publication) else {
            return;
        };

        self.state.lock_or_panic().published.insert(key(phone));
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                inner.notify_new_photo(peer, item).await;
            }
        });
    }

    /// Whether the phone's new-photo notifications are on and the phone enabled; `false` (logged)
    /// otherwise or when either is unreadable.
    fn new_photos_on(&self, phone: &DcgClient) -> bool {
        let enabled = match self.data(phone) {
            Ok(d) => d.enabled,
            Err(e) => {
                warn!(%phone, error = %e, "remote-snipping store unreadable; new-photo publication dropped");
                return false;
            }
        };

        match self.phone_enabled(phone) {
            Ok(true) if enabled => true,
            Ok(_) => {
                debug!(%phone, enabled, "new-photo notifications off or phone disabled; publication dropped");
                false
            }
            Err(e) => {
                warn!(%phone, error = %e, "phone switch unreadable; new-photo publication dropped");
                false
            }
        }
    }

    /// Tag 3: `PHOTOS` sets the phone's `has_permissions`; nothing clears it.
    fn on_permissions(&self, publication: &Publication) {
        match wire::publication_grants_photos(&publication.payload.additional) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => {
                debug!(partner = %publication.source, error = %e, "permission publication does not parse; ignored");
                return;
            }
        }

        let Some(peer) = self.env.peers.peer(&publication.source) else {
            warn!(partner = %publication.source, "permission publication from a device not in the account; dropped");
            return;
        };

        info!(phone = %peer.client, "the phone granted photo access");
        self.update_logged(&peer.client, "a permission publication", |d| d.has_permissions = true);
    }

    // --------------------------------------------------------------------------------------------
    // Permission request
    // --------------------------------------------------------------------------------------------

    /// The permission request; its result goes to the app, which logs a failure.
    async fn request_permission(self: Arc<Self>, peer: PeerDevice) -> PermissionOutcome {
        let phone = peer.client.clone();
        if !self.env.peers.is_enrolled() {
            return PermissionOutcome::Failed("no current account".into());
        }

        self.state.lock_or_panic().requesting.insert(key(&phone));
        self.changed(&phone);

        let outcome = self
            .send(
                &peer,
                ProtoDeviceResourceType::Permissions,
                ProtoDeviceResourceRequestType::Sync,
                wire::photos_permission_request(),
                true,
                wire::trace_name::PERMISSION,
            )
            .await;
        let result = match outcome {
            DrmSendOutcome::Success { payload } => wire::permission_outcome(&payload),
            other => PermissionOutcome::Failed(format!("{other:?}")),
        };

        if !matches!(result, PermissionOutcome::Failed(_)) {
            info!(%phone, ?result, "photo permission request answered");
        }
        if result == PermissionOutcome::Granted {
            self.update_logged(&phone, "a granted permission", |d| d.has_permissions = true);
        }

        self.state.lock_or_panic().requesting.remove(&key(&phone));
        self.changed(&phone);
        result
    }
}

/// The photo a tag-6 publication announces, `None` (logged) for another change or one that does not
/// parse.
fn new_photo_item(phone: &DcgClient, publication: &Publication) -> Option<MediaMetadata> {
    match wire::parse_media_change(&publication.payload.additional) {
        Ok(wire::MediaChange::NewPhoto(item)) => Some(item),
        Ok(wire::MediaChange::NewPhotoWithoutItem) => {
            warn!(%phone, "NEW_PHOTO publication without an item; dropped");
            None
        }
        Ok(wire::MediaChange::Other(change_type)) => {
            debug!(%phone, change_type, "media change other than NEW_PHOTO; ignored");
            None
        }
        Err(e) => {
            warn!(%phone, error = %e, "new-photo publication does not parse; dropped");
            None
        }
    }
}

struct NewPhotoObserver(Weak<Inner>);

impl PublicationObserver for NewPhotoObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_new_photo(publication);
        }
    }
}

struct PermissionObserver(Weak<Inner>);

impl PublicationObserver for PermissionObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_permissions(publication);
        }
    }
}

struct Gate(Weak<Inner>);

impl NewPhotoGate for Gate {
    fn covered(&self, phone: &DcgClient) -> bool {
        self.0.upgrade().is_some_and(|inner| inner.covers(phone))
    }
}
