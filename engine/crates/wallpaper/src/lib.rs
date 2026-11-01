//! Wallpaper: the phone's wallpaper on the Mac and the Mac's wallpaper on the phone.
//!
//! | Module | Implements |
//! |---|---|
//! | [`phone_link`] | Phone Link's contract: the `wallpaper` content processor and connect provider #4 |
//! | [`cdeh`] | the Cross Device contract: the phone wallpaper pull, the `/wallpaper` receiver, the tag-5 subscription |
//! | [`store`] | both copies of the phone's wallpaper per phone, Phone Link's sequence numbers and the phone's wallpaper permission |
//! | [`edge`] | the Mac's desktop picture as an OS edge, its size rule, and the `/wallpaper` answer |
//!
//! The UI shows one wallpaper per phone ([`Wallpaper::shown`]): Phone Link's home-screen image while
//! the phone is supported and reports its wallpaper permission, else the Cross Device copy's
//! home-screen image, else none.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod cdeh;
pub mod edge;
pub mod phone_link;
pub mod store;

use phonelink_core::config::features::WallpaperConfig;
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use phonelink_app::phone_record::load_record;
use phonelink_app::{ConnectProvider, PhoneRecord};
use phonelink_core::seams::SessionEvent;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores};
use phonelink_features::{FeatureEnv, FeatureError};
use phonelink_transport::auth::PeerEvent;
use phonelink_transport::platform::{DeviceFilter, Drm, DrmSendOutcome, Msaep, Subscription};

#[cfg(feature = "test-support")]
pub use edge::{MAX_PICTURE_HEIGHT, PictureSize, wallpaper_scaled_size};
pub use edge::{PcWallpaper, PcWallpaperSource};
pub use store::{PhoneLinkState, StoredWallpaper, WallpaperCopy};

use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;
use store::{HOME_SCREEN, LOCK_SCREEN};

/// The feature's name: its store.
pub const FEATURE: &str = "wallpaper";

/// The wallpaper the UI shows for a phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownWallpaper {
    /// The image bytes as the phone sent them.
    pub image: Vec<u8>,
    pub copy: WallpaperCopy,
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperEvent {
    /// What [`Wallpaper::shown`] gives for `phone` may have changed.
    Changed { phone: DcgClient },
}

/// A phone's store key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// The feature's shared state; tasks and registered callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    drm: Arc<Drm>,
    store: PhoneStores,
    events: broadcast::Sender<WallpaperEvent>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscription: Mutex<Option<Subscription>>,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The wallpaper feature.
pub struct Wallpaper {
    inner: Arc<Inner>,
}

impl Wallpaper {
    /// Installs the feature: the tag-5 subscription and, while enabled, the `/wallpaper` receiver; the
    /// `wallpaper` processor and connect provider #4; then the listeners and the pull of every phone
    /// that is already available and Android. `env` carries the App layer (Phone Link's contract) and
    /// the Cross Device connection and device list; `msaep` and `drm` are that connection's platform
    /// handles; `pc_wallpaper` answers `/wallpaper`. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: WallpaperConfig,
        msaep: Arc<Msaep>,
        drm: Arc<Drm>,
        pc_wallpaper: Arc<dyn PcWallpaperSource>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("wallpaper must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            drm: drm.clone(),
            store: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
            events: broadcast::channel(64).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscription: Mutex::new(None),
            weak: weak.clone(),
        });

        let session_events = env.sessions.subscribe();
        let peer_events = env.peers.subscribe();

        // Cross Device.
        let subscription = msaep.subscribe(cdeh::WALLPAPER_TAG, DeviceFilter::Any, Arc::new(cdeh::IgnoredPublications));
        *inner.subscription.lock_or_recover() = Some(subscription);
        if config.pc_wallpaper_sync_enabled {
            drm.register_receiver(cdeh::PC_WALLPAPER_PATH, Arc::new(cdeh::PcWallpaperReceiver::new(pc_wallpaper)))
                .map_err(RegistrationError::from)?;
        } else {
            info!("PC wallpaper sync off by configuration; no /wallpaper receiver");
        }

        // Phone Link.
        env.app.register_processor_factory(
            &[phone_link::CONTENT_TYPE],
            Arc::new(phone_link::WallpaperFactory(Arc::downgrade(&inner))),
        )?;
        env.app.register_connect_contributor(
            ConnectProvider::Wallpaper,
            Arc::new(phone_link::WallpaperContributor::new(Arc::downgrade(&inner))),
        )?;

        inner.spawn_listeners(session_events, peer_events);
        inner.start_pulls();
        info!("feature installed");
        Ok(Arc::new(Wallpaper { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<WallpaperEvent> {
        self.inner.events.subscribe()
    }

    /// The wallpaper the UI shows for `phone`: Phone Link's home-screen image when the phone is
    /// supported, has reported its wallpaper permission granted and the image is not empty; else the
    /// Cross Device copy's home-screen image when it is not empty; else none.
    pub fn shown(&self, phone: &DcgClient) -> Result<Option<ShownWallpaper>, FeatureError> {
        let db = self.inner.db(phone)?;
        let supported =
            self.inner.record_for(phone).is_some_and(|r| phone_link::supported(&r, &self.inner.env.app.config().app));

        if supported && store::phone_link_state(&db)?.device_permission {
            if let Some(w) =
                store::wallpaper(&db, WallpaperCopy::PhoneLink, HOME_SCREEN)?.filter(|w| !w.bytes.is_empty())
            {
                return Ok(Some(ShownWallpaper { image: w.bytes, copy: WallpaperCopy::PhoneLink }));
            }
        }
        Ok(store::wallpaper(&db, WallpaperCopy::CrossDevice, HOME_SCREEN)?
            .filter(|w| !w.bytes.is_empty())
            .map(|w| ShownWallpaper { image: w.bytes, copy: WallpaperCopy::CrossDevice }))
    }

    /// One stored row of `phone` (`wallpaper_type` `0` home screen, `1` lock screen).
    pub fn stored(
        &self,
        phone: &DcgClient,
        copy: WallpaperCopy,
        wallpaper_type: i64,
    ) -> Result<Option<StoredWallpaper>, FeatureError> {
        Ok(store::wallpaper(&*self.inner.db(phone)?, copy, wallpaper_type)?)
    }

    /// Phone Link's sequence numbers and the phone's wallpaper permission for `phone`.
    pub fn phone_link_state(&self, phone: &DcgClient) -> Result<PhoneLinkState, FeatureError> {
        Ok(store::phone_link_state(&*self.inner.db(phone)?)?)
    }
}

impl Inner {
    fn emit(&self, event: WallpaperEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    pub(crate) fn changed(&self, phone: &DcgClient) {
        self.emit(WallpaperEvent::Changed { phone: phone.clone() });
    }

    pub(crate) fn db(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, FeatureError> {
        Ok(self.store.get(&key(phone))?)
    }

    /// The selected phone's live record when `phone` is selected; otherwise its stored one.
    fn record_for(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        if self.env.app.selected_phone().is_some_and(|s| s.is_same_client(phone)) {
            return self.env.app.phone_record();
        }
        match load_record(&*self.env.edges.store, phone) {
            Ok(record) => record,
            Err(e) => {
                warn!(%phone, error = %e, "stored phone record unreadable; Phone Link's wallpaper not shown");
                None
            }
        }
    }

    fn spawn_listeners(
        &self,
        mut session_events: broadcast::Receiver<SessionEvent>,
        mut peer_events: broadcast::Receiver<PeerEvent>,
    ) {
        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = session_events.recv() => event,
                };

                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(SessionEvent::Connected(phone)) => inner.on_available(&phone),
                    Ok(SessionEvent::Disconnected { .. }) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "missed session events; phones that came up meanwhile are not pulled")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = peer_events.recv() => event,
                };

                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(PeerEvent::Added(peer)) => {
                        if inner.env.sessions.is_connected(&peer.client) && cdeh::is_android(&peer.info) {
                            inner.spawn_pull(peer.client.clone());
                        }
                    }
                    // A removed phone keeps its stored copies, as on Windows.
                    Ok(PeerEvent::Removed(_) | PeerEvent::Updated(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "missed device-list events; phones added meanwhile are not pulled")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// The phone's platform connection came up: pull it when it is an Android phone of the account.
    fn on_available(&self, phone: &DcgClient) {
        match self.env.peers.peer(phone) {
            Some(peer) if cdeh::is_android(&peer.info) => self.spawn_pull(peer.client),
            Some(_) => debug!(%phone, "available phone is not Android; its wallpaper is not pulled"),
            None => debug!(%phone, "available phone not in the account's device list; its wallpaper is not pulled"),
        }
    }

    /// The pull's first trigger: every phone of the account that is already available and Android.
    fn start_pulls(&self) {
        if !self.env.peers.is_enrolled() {
            debug!("no enrolled account; no wallpaper pull at start");
            return;
        }

        for peer in self.env.peers.peers() {
            if self.env.sessions.is_connected(&peer.client) && cdeh::is_android(&peer.info) {
                self.spawn_pull(peer.client);
            }
        }
    }

    /// One pull through CDEH's request wrapper: nothing is sent without a current account or when the
    /// phone is not in its device list; the reply is applied to the Cross Device copy. No retry. The
    /// request runs in its own task, which does not keep the feature alive.
    fn spawn_pull(&self, phone: DcgClient) {
        if !self.env.peers.is_enrolled() {
            debug!(%phone, "no current account; wallpaper pull not sent");
            return;
        }
        let Some(peer) = self.env.peers.peer(&phone) else {
            debug!(%phone, "phone not in the account's device list; wallpaper pull not sent");
            return;
        };
        let Some((phone_enabled, db, (home, lock))) = self.pull_state(&phone, &peer.client) else {
            return;
        };

        let request = cdeh::pull_request(
            &peer,
            &cdeh::pull_payload(home, lock),
            phone_enabled,
            self.env.config.platform.request_timeout,
            self.shutdown.child_token(),
        );

        let (weak, drm) = (self.weak.clone(), self.drm.clone());
        self.runtime.spawn(async move {
            let outcome = drm.send(request).await;
            let Some(inner) = weak.upgrade() else { return };
            let Some(change) = pull_reply(&phone, outcome) else {
                return;
            };
            inner.apply_pull_reply(&phone, &peer.client, &db, &change);
        });
    }

    /// What a pull reads before it is sent: the phone's enabled switch, its store and the ids of
    /// its stored Cross Device copy; `None` (logged) when one cannot be read.
    fn pull_state(&self, phone: &DcgClient, client: &DcgClient) -> Option<(bool, Arc<FeatureDb>, CrossDeviceIds)> {
        let phone_enabled = match self.env.switches.is_enabled(client) {
            Ok(enabled) => enabled,
            Err(e) => {
                warn!(%phone, error = %e, "phone switch unreadable; wallpaper pull not sent");
                return None;
            }
        };

        let db = match self.db(client) {
            Ok(db) => db,
            Err(e) => {
                warn!(%phone, error = %e, "wallpaper store not opened; wallpaper pull not sent");
                return None;
            }
        };

        match store::cross_device_ids(&db) {
            Ok(ids) => Some((phone_enabled, db, ids)),
            Err(e) => {
                warn!(%phone, error = %e, "stored wallpaper ids unreadable; wallpaper pull not sent");
                None
            }
        }
    }

    /// Stores a pull reply's change in the Cross Device copy and, once stored, announces the change.
    fn apply_pull_reply(
        &self,
        phone: &DcgClient,
        client: &DcgClient,
        db: &FeatureDb,
        change: &store::CrossDeviceChange,
    ) {
        match store::apply_cross_device(db, change) {
            Ok(()) => {
                match change {
                    store::CrossDeviceChange::DeleteAll => {
                        info!(%phone, "wallpaper pull answered without OK; the phone's pulled wallpapers deleted")
                    }
                    store::CrossDeviceChange::Upsert(rows) => info!(
                        %phone,
                        has_home = rows.iter().any(|r| r.0 == HOME_SCREEN),
                        has_lock = rows.iter().any(|r| r.0 == LOCK_SCREEN),
                        "wallpaper pull reply applied"
                    ),
                }

                self.changed(client);
            }
            Err(e) => error!(%phone, error = %e, "wallpaper pull reply not stored"),
        }
    }
}

/// The ids of the home-screen and lock-screen pictures of a phone's stored Cross Device copy.
type CrossDeviceIds = (Option<i64>, Option<i64>);

/// The change a pull's outcome carries, `None` (logged) when the pull failed or its reply does not
/// read.
fn pull_reply(phone: &DcgClient, outcome: DrmSendOutcome) -> Option<store::CrossDeviceChange> {
    let payload = match outcome {
        DrmSendOutcome::Success { payload } => payload,
        other => {
            warn!(%phone, outcome = ?other, error = other.cause(), "wallpaper pull failed; nothing changed");
            return None;
        }
    };

    match cdeh::read_pull_reply(&payload) {
        Ok(change) => Some(change),
        Err(e) => {
            warn!(%phone, error = %e, "wallpaper pull reply does not read; nothing changed");
            None
        }
    }
}
