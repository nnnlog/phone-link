//! Photos — the phone's recent gallery on the PC, Android only.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the message tables: inbound media-list sync (metadata, items), thumbnail fetch, full-resolution fetch, media sync request, delete |
//! | [`store`] | the local state the protocol depends on, cache maintenance (one SQLite store per phone) |
//! | `sync` | the `mediaInfo` processor: reconciliation, the pending set, completion, malformed inbound data and replies |
//! | `connect` | the contribution to the connect message |
//! | [`dispatch`] | request scheduling, the common rules of outbound requests on /media/v1 (retries, stop-all, cancellation) |
//! | `notify` | the new-photo notification and its activation |
//! | this module | gating and state (feature gates, PC-side settings, the full-access reminder), triggers, permission requests, the media sync request and delete flows, the "syncing" indicator, the trace context name of each entry point, and the UI-facing API |
//!
//! Every photos request is a legacy App request on `/media/v1` sent with default options (not
//! user-initiated) except the delete's timeout; permission requests are the shared `/permission`
//! request sent user-initiated. Cross Device's new-photo notifications are a separate feature
//! (`remote_snipping`); while it covers a phone, this feature posts no new-photo
//! notification for it ([`NewPhotoGate`]).

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod dispatch;
pub mod store;
pub mod wire;

mod connect;
mod notify;
mod sync;

use phonelink_core::config::features::PhotosConfig;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

use phonelink_app::consts::{capability as phone_capability, permission, platform};
use phonelink_app::{AppError, AppOutcome, ConnectProvider, PhoneRecord};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError};
use phonelink_features::{FeatureEnv, FeatureError};

mod access;
mod delete;
mod gallery;

#[cfg(feature = "test-support")]
pub use notify::{USER_INFO_MEDIA_ID, USER_INFO_PHONE, is_new_photo};

use phonelink_features::NewPhotoGate;
pub use wire::{capability, trace_name};

use dispatch::{ImageState, ThumbState};
use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;
use store::{PhotoRow, PhotosState, Setting};

mod state;
pub(crate) use state::PhotosInner;

/// What the UI is told to re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotosEvent {
    /// Items, thumbnails, full bytes or the displayed set changed: re-read [`Photos::gallery`].
    GalleryChanged { phone: DcgClient },
    /// Permissions, capabilities, settings, the reminder or the syncing state changed: re-read
    /// [`Photos::status`].
    StatusChanged { phone: DcgClient },
    /// The full-resolution bytes of `id` are stored.
    FullImageStored { phone: DcgClient, id: i64 },
    /// A delete finished.
    DeleteFinished { phone: DcgClient, result: DeleteResult },
    /// A new-photo notification was activated and the full image is stored: open it with the
    /// default macOS viewer (the bytes: [`Photos::full_image`]).
    OpenImage { phone: DcgClient, id: i64 },
}

/// Why a UI call could not run.
#[derive(Debug, thiserror::Error)]
pub enum PhotosError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("photo {0} is not in the cache")]
    NotCached(i64),
    #[error("deleting photos is not offered for this phone")]
    DeleteNotOffered,
    #[error("the delete action is not enabled for this phone")]
    DeleteNotEnabled,
    /// The item shows no image (neither thumbnail nor full bytes), or a delete of it is in flight.
    #[error("the delete action is not enabled for photo {0}: it shows no image, or its delete is in flight")]
    ItemDeleteNotEnabled(i64),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("App layer: {0}")]
    App(#[from] AppError),
}

/// An item's delete progress (UI state, in memory).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeleteState {
    #[default]
    None,
    /// The delete request is in flight.
    Deleting,
    /// The phone reported it deleted; it leaves the displayed gallery after 250 ms.
    Deleted,
}

/// One gallery item (without its bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoItem {
    pub id: i64,
    /// File name (the default name for save and copy).
    pub name: String,
    /// Last-modified time, Unix ms.
    pub modified_ms: i64,
    /// Date taken, Unix ms.
    pub taken_ms: i64,
    pub orientation: String,
    /// Kept as received; the PC never interprets the bytes.
    pub mime_type: String,
    pub height: i32,
    pub width: i32,
    /// Full file size, bytes.
    pub size: i64,
    pub uri: String,
    pub checksum: i64,
    pub last_seen_ms: Option<i64>,
    pub has_thumbnail: bool,
    pub has_full: bool,
    pub delete_state: DeleteState,
}

impl PhotoItem {
    fn from_row(r: PhotoRow, delete_state: DeleteState) -> Self {
        Self {
            id: r.id,
            name: r.name,
            modified_ms: r.modified_ms,
            taken_ms: r.taken_ms,
            orientation: r.orientation,
            mime_type: r.mime_type,
            height: r.height,
            width: r.width,
            size: r.size,
            uri: r.uri,
            checksum: r.checksum,
            last_seen_ms: r.last_seen_ms,
            has_thumbnail: r.has_thumbnail,
            has_full: r.has_full,
            delete_state,
        }
    }

    /// Thumbnail or full bytes cached: clicking it opens the viewer.
    pub fn is_actionable(&self) -> bool {
        self.has_thumbnail || self.has_full
    }
}

/// The feature's state for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotosStatus {
    pub phone: DcgClient,
    /// Android ∧ the phone's `Photos` capability bit: the gallery exists.
    pub enabled: bool,
    /// PC-side photos consent; off shows the consent prompt ([`Photos::set_consent`]).
    pub consent: bool,
    /// The phone's `Photos` permission; off shows "allow photo access on the phone"
    /// ([`Photos::request_photo_access`]).
    pub photos_permission: bool,
    pub photos_full_permission: bool,
    pub photos_delete_permission: bool,
    /// `PhotosCapabilities` flags ([`capability`]).
    pub capabilities: u32,
    /// PC-side "deletion allowed".
    pub deletion_allowed: bool,
    /// The delete action is offered: Android ∧ `MediaDelete` ∧ deletion allowed.
    pub delete_offered: bool,
    /// The delete action is enabled (on Android ≥ 13: a photo-delete, post-notification or
    /// auto-launch permission is held).
    pub delete_enabled: bool,
    /// PC-side "new-photo notifications".
    pub new_photo_notifications: bool,
    /// Show the full-access reminder ([`Photos::request_full_access`],
    /// [`Photos::dismiss_full_access_reminder`]).
    pub full_access_reminder: bool,
    /// The "syncing" indicator.
    pub syncing: bool,
    /// `current ≥ expected`.
    pub up_to_date: bool,
    pub current_sequence: i64,
    pub expected_sequence: i64,
    /// The last sync time, Unix ms.
    pub last_sync_ms: Option<i64>,
    /// `MediaInfoSnapshotSize`, for the "number of photos to expect" label.
    pub snapshot_size: i32,
    /// Group the gallery by month.
    pub grouping: bool,
}

/// What the delete pre-check leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeletePrompt {
    /// Ask the user to confirm, then call [`Photos::delete`].
    Confirmation,
    /// Show the "missing permission" prompt; its action is [`Photos::request_delete_permission`].
    MissingPermission,
}

/// The outcome of one delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteResult {
    pub outcome: AppOutcome,
    /// Every id the phone reported deleted (`mediaSuccessfullyDeletedIds`); those still displayed
    /// leave the displayed gallery after 250 ms.
    pub deleted: Vec<i64>,
    /// The other requested ids: they revert to "not deleted".
    pub reverted: Vec<i64>,
    /// Show the "delete failed" notice.
    pub failed_notice: bool,
}

/// The default save-as name of an item: its name's stem and extension (`.jpg` when it has none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAsName {
    pub stem: String,
    /// With the dot.
    pub extension: String,
}

/// The stem used when an item's name has none (local UI text).
pub const DEFAULT_SAVE_STEM: &str = "Photo";

/// The default save-as name for `name`.
pub fn save_as_name(name: &str) -> SaveAsName {
    let path = std::path::Path::new(name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SAVE_STEM.to_string());
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .filter(|e| !e.is_empty())
        .map_or_else(|| ".jpg".to_string(), |e| format!(".{e}"));

    SaveAsName { stem, extension }
}

/// A file action on an item that needs its full bytes; its fetch carries Phone Link's trigger name
/// of the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAction {
    Copy,
    SaveAs,
    Share,
    Open,
    OpenWith,
    DragOut,
}

impl FileAction {
    /// The `ypTriggerId` of the fetch the action starts.
    pub fn trigger(self) -> &'static str {
        match self {
            FileAction::Copy => trace_name::ON_COPY,
            FileAction::SaveAs => trace_name::ON_SAVE_AS,
            FileAction::Share => trace_name::ON_SHARE,
            FileAction::Open | FileAction::OpenWith => trace_name::ON_OPEN,
            FileAction::DragOut => trace_name::ON_DRAG,
        }
    }
}

/// The "syncing" indicator: set when the up-to-date state becomes false (and at start when not up to
/// date) for at most 2 minutes, cleared when up to date, and set while a media sync request is in
/// flight.
#[derive(Debug, Default)]
pub(crate) struct Indicator {
    pub(crate) updating: bool,
    pub(crate) generation: u64,
    pub(crate) up_to_date: Option<bool>,
}

/// UI state of a phone, in memory.
#[derive(Debug, Default)]
pub(crate) struct UiState {
    /// Ids whose thumbnail fetch a gallery trigger requested, until it arrives, is already present,
    /// gives up or is cancelled.
    pub(crate) pending_thumbnails: HashSet<i64>,
    /// The same for full-resolution fetches.
    pub(crate) pending_full: HashSet<i64>,
    /// Ids removed from the displayed gallery after a successful delete, until an item message from
    /// the phone writes their cache entry again or they leave the cache; a fetch reply stored for
    /// them does not bring them back.
    pub(crate) hidden: HashSet<i64>,
    pub(crate) deleting: HashMap<i64, DeleteState>,
    /// The previous new-photo pop-up, Unix ms.
    pub(crate) last_popup_ms: Option<i64>,
    pub(crate) indicator: Indicator,
}

/// One phone's open store, dispatchers and UI state.
pub(crate) struct PhoneState {
    pub(crate) phone: DcgClient,
    pub(crate) db: Arc<FeatureDb>,
    pub(crate) thumbs: Mutex<ThumbState>,
    pub(crate) images: Mutex<ImageState>,
    pub(crate) ui: Mutex<UiState>,
    /// Woken whenever full-resolution bytes are stored.
    pub(crate) full_stored: Notify,
}

/// The record-derived inputs of the status, to announce their changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordView {
    phone: DcgClient,
    enabled: bool,
    photos: bool,
    photos_full: bool,
    photos_delete: bool,
    post_notifications: bool,
    auto_launch: bool,
    os_major: Option<i32>,
    android_sdk_version: u32,
}

impl RecordView {
    fn of(record: &PhoneRecord) -> Self {
        Self {
            phone: record.phone(),
            enabled: is_enabled(record),
            photos: record.permission_or(permission::PHOTOS, true),
            photos_full: record.permission_or(permission::PHOTOS_FULL, false),
            photos_delete: record.permission_or(permission::PHOTOS_DELETE, false),
            post_notifications: record.permission_or(permission::POST_NOTIFICATIONS, true),
            auto_launch: record.permission_or(permission::AUTO_LAUNCH, false),
            os_major: record.android_major_version(),
            android_sdk_version: record.android_sdk_version,
        }
    }
}

/// Android ∧ the `Photos` capability bit.
fn is_enabled(record: &PhoneRecord) -> bool {
    record.platform == platform::ANDROID && record.has_capability(phone_capability::PHOTOS)
}

/// The photos feature handle: install it once; its methods are the UI's view of the gallery and the
/// photos triggers. Calls act on the selected phone.
pub struct Photos {
    inner: Arc<PhotosInner>,
}

impl Photos {
    /// Registers the `mediaInfo` processor factory, the connect provider and the `photos`
    /// notification responder, opens the selected phone's store, and starts the cache maintenance
    /// (now and every 24 h). Must run inside a tokio runtime.
    pub fn install(env: &FeatureEnv, config: PhotosConfig) -> Result<Arc<Self>, FeatureError> {
        validate_config(&config)?;
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("photos must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new(PhotosInner {
            config,
            app: Arc::downgrade(&env.app),
            clock: env.edges.clock.clone(),
            notifier: env.notifier.clone(),
            stores: PhoneStores::new(env.stores.clone(), store::FEATURE, store::MIGRATIONS),
            phones: Mutex::new(HashMap::new()),
            last_view: Mutex::new(None),
            events: broadcast::channel(256).0,
            runtime,
            shutdown: CancellationToken::new(),
            new_photo_gate: OnceLock::new(),
        });

        env.app.register_processor_factory(
            &[wire::content_type::MEDIA_INFO],
            Arc::new(sync::MediaInfoFactory { inner: inner.clone() }),
        )?;
        env.app.register_connect_contributor(
            ConnectProvider::Photos,
            Arc::new(connect::PhotosContributor { inner: inner.clone() }),
        )?;
        env.responses
            .register(store::FEATURE, Arc::new(notify::PhotosResponder { inner: Arc::downgrade(&inner) }))
            .map_err(RegistrationError::from)?;

        inner.spawn_app_listener(env.app.subscribe_events());
        if let Some(phone) = env.app.selected_phone() {
            inner.phone_state(&phone)?;
        }
        if let Some(record) = env.app.phone_record() {
            *inner.last_view.lock_or_panic() = Some(RecordView::of(&record));
        }

        inner.spawn_maintenance();
        Ok(Arc::new(Self { inner }))
    }

    /// Events that tell the UI what to re-read.
    pub fn subscribe(&self) -> broadcast::Receiver<PhotosEvent> {
        self.inner.events.subscribe()
    }

    /// Sets the gate of the new-photo notification (once): a phone it reports covered gets none from
    /// here.
    pub fn set_new_photo_gate(&self, gate: Arc<dyn NewPhotoGate>) -> Result<(), FeatureError> {
        self.inner
            .new_photo_gate
            .set(gate)
            .map_err(|_| FeatureError::Programming("the photos new-photo gate is set once".into()))
    }

    fn current(&self) -> Result<(DcgClient, Arc<PhoneState>), PhotosError> {
        let app = self.inner.app.upgrade().ok_or(PhotosError::NoPhone)?;
        let phone = app.selected_phone().ok_or(PhotosError::NoPhone)?;
        let ps = self.inner.phone_state(&phone)?;
        Ok((phone, ps))
    }

    fn row(&self, ps: &PhoneState, id: i64) -> Result<PhotoRow, PhotosError> {
        store::item(&ps.db, id)?.ok_or(PhotosError::NotCached(id))
    }

    /// A delete took the item out of the displayed gallery; its cache entry stays until the phone's
    /// data drops it, but it has no grid cell, so no grid trigger applies to it.
    fn is_hidden(&self, ps: &PhoneState, id: i64) -> bool {
        ps.ui.lock_or_panic().hidden.contains(&id)
    }

    /// The displayed items: the cache in display order without the ids a delete removed.
    fn displayed(&self, ps: &PhoneState) -> Result<Vec<PhotoItem>, PhotosError> {
        let rows = store::gallery(&ps.db)?;
        let ui = ps.ui.lock_or_panic();

        Ok(rows
            .into_iter()
            .filter(|r| !ui.hidden.contains(&r.id))
            .map(|r| {
                let state = ui.deleting.get(&r.id).copied().unwrap_or_default();
                PhotoItem::from_row(r, state)
            })
            .collect())
    }

    // --- views ------------------------------------------------------------------------------
}

fn validate_config(config: &PhotosConfig) -> Result<(), FeatureError> {
    if config.auto_hydrated_thumbnails_count < 0 {
        return Err(FeatureError::Config("AutoHydratedThumbnailsCount must not be negative".into()));
    }
    if config.thumbnail_batch == 0 || config.full_concurrency == 0 {
        return Err(FeatureError::Config("photos batch size and concurrency must be at least 1".into()));
    }
    if config.maintenance_interval.is_zero() {
        return Err(FeatureError::Config("photos maintenance interval must not be zero".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
