//! Photos for the UI ([`crate::features::photos`]).
//!
//! It covers the gallery and its thumbnails with the fetch triggers the grid and the viewer report,
//! the file actions' full images, delete, the settings and the phone-side permission prompts.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::photos::{
    DeletePrompt, DeleteResult, DeleteState, FileAction, PhotoItem, Photos, PhotosError, PhotosEvent, PhotosStatus,
};

use super::{FeatureApiError, RequestOutcome, core_of, features_of};

impl From<PhotosError> for FeatureApiError {
    fn from(e: PhotosError) -> Self {
        match e {
            PhotosError::NoPhone => FeatureApiError::NoPhone,
            e @ (PhotosError::NotCached(_)
            | PhotosError::DeleteNotOffered
            | PhotosError::DeleteNotEnabled
            | PhotosError::ItemDeleteNotEnabled(_)) => FeatureApiError::NotAvailable { reason: e.to_string() },
            PhotosError::Store(e) => e.into(),
            PhotosError::App(e) => e.into(),
        }
    }
}

/// Where a photo's delete stands: none asked, deleting, or deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhotoDeleteState {
    None,
    Deleting,
    Deleted,
}

impl From<DeleteState> for PhotoDeleteState {
    fn from(s: DeleteState) -> Self {
        match s {
            DeleteState::None => PhotoDeleteState::None,
            DeleteState::Deleting => PhotoDeleteState::Deleting,
            DeleteState::Deleted => PhotoDeleteState::Deleted,
        }
    }
}

/// One gallery item (its bytes come from `thumbnail` / `full_image`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Photo {
    pub id: i64,
    pub name: String,
    pub taken_ms: i64,
    pub modified_ms: i64,
    pub mime_type: String,
    pub width: i32,
    pub height: i32,
    pub size: i64,
    pub has_thumbnail: bool,
    pub has_full: bool,
    pub delete_state: PhotoDeleteState,
}

impl From<&PhotoItem> for Photo {
    fn from(p: &PhotoItem) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            taken_ms: p.taken_ms,
            modified_ms: p.modified_ms,
            mime_type: p.mime_type.clone(),
            width: p.width,
            height: p.height,
            size: p.size,
            has_thumbnail: p.has_thumbnail,
            has_full: p.has_full,
            delete_state: p.delete_state.into(),
        }
    }
}

/// Photos' state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotosState {
    /// The phone has the photos feature.
    pub enabled: bool,
    /// "Show photos on this Mac".
    pub consent: bool,
    /// The phone allows photos; off shows the permission prompt.
    pub photos_permission: bool,
    /// Show the full-access reminder (the phone shares only some photos).
    pub full_access_reminder: bool,
    /// "Allow deleting photos".
    pub deletion_allowed: bool,
    /// Delete is offered and enabled.
    pub delete_available: bool,
    /// "New-photo notifications".
    pub new_photo_notifications: bool,
    pub syncing: bool,
    pub up_to_date: bool,
    pub last_sync_ms: Option<i64>,
}

impl From<&PhotosStatus> for PhotosState {
    fn from(s: &PhotosStatus) -> Self {
        Self {
            enabled: s.enabled,
            consent: s.consent,
            photos_permission: s.photos_permission,
            full_access_reminder: s.full_access_reminder,
            deletion_allowed: s.deletion_allowed,
            delete_available: s.delete_offered && s.delete_enabled,
            new_photo_notifications: s.new_photo_notifications,
            syncing: s.syncing,
            up_to_date: s.up_to_date,
            last_sync_ms: s.last_sync_ms,
        }
    }
}

/// A file action that needs the full image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhotoAction {
    Copy,
    SaveAs,
    Share,
    Open,
    OpenWith,
    DragOut,
}

impl From<PhotoAction> for FileAction {
    fn from(a: PhotoAction) -> Self {
        match a {
            PhotoAction::Copy => FileAction::Copy,
            PhotoAction::SaveAs => FileAction::SaveAs,
            PhotoAction::Share => FileAction::Share,
            PhotoAction::Open => FileAction::Open,
            PhotoAction::OpenWith => FileAction::OpenWith,
            PhotoAction::DragOut => FileAction::DragOut,
        }
    }
}

/// What deleting a photo needs first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhotoDeleteCheck {
    /// Ask the user to confirm, then call `delete`.
    Confirm,
    /// The phone has not allowed deleting; offer `request_delete_permission`.
    MissingPermission,
}

/// The outcome of a delete.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotoDeleteResult {
    pub outcome: RequestOutcome,
    pub deleted: Vec<i64>,
    pub reverted: Vec<i64>,
    /// Show the "delete failed" notice.
    pub failed: bool,
}

impl From<&DeleteResult> for PhotoDeleteResult {
    fn from(r: &DeleteResult) -> Self {
        Self {
            outcome: (&r.outcome).into(),
            deleted: r.deleted.clone(),
            reverted: r.reverted.clone(),
            failed: r.failed_notice,
        }
    }
}

/// The default name of a saved photo.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhotoFileName {
    pub stem: String,
    /// With the dot.
    pub extension: String,
}

/// A change to photos: re-read the gallery or the state, or a full image, delete or open to handle.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhotosChange {
    GalleryChanged,
    StateChanged,
    /// The full image of `id` is stored.
    FullImageStored {
        id: i64,
    },
    DeleteFinished {
        result: PhotoDeleteResult,
    },
    /// The user clicked a new-photo notification: open the photo in the default viewer.
    OpenImage {
        id: i64,
    },
}

impl From<&PhotosEvent> for PhotosChange {
    fn from(e: &PhotosEvent) -> Self {
        match e {
            PhotosEvent::GalleryChanged { .. } => PhotosChange::GalleryChanged,
            PhotosEvent::StatusChanged { .. } => PhotosChange::StateChanged,
            PhotosEvent::FullImageStored { id, .. } => PhotosChange::FullImageStored { id: *id },
            PhotosEvent::DeleteFinished { result, .. } => PhotosChange::DeleteFinished { result: result.into() },
            PhotosEvent::OpenImage { id, .. } => PhotosChange::OpenImage { id: *id },
        }
    }
}

/// Photos of the selected phone.
#[derive(uniffi::Object)]
pub struct PhotosApi {
    engine: Arc<Engine>,
}

impl PhotosApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<Arc<Photos>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        Ok(features_of(&core)?.photos.clone())
    }

    fn run<T>(&self, f: impl FnOnce(&Photos) -> Result<T, PhotosError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.photos.clone();
        Ok(f(&feature)?)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PhotosApi {
    /// The selected phone's photos setting, the phone's photo permissions, and the sync state.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<PhotosState, FeatureApiError> {
        self.run(|f| f.status()).map(|s| (&s).into())
    }

    /// The gallery: taken time descending, then last-modified descending.
    pub fn gallery(&self) -> Result<Vec<Photo>, FeatureApiError> {
        self.run(|f| f.gallery()).map(|v| v.iter().map(Into::into).collect())
    }

    /// The stored thumbnail of photo `id`; `None` until the phone has sent it (no fetch).
    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>, FeatureApiError> {
        self.run(|f| f.thumbnail(id))
    }

    /// The cached full image (no fetch).
    pub fn full_image(&self, id: i64) -> Result<Option<Vec<u8>>, FeatureApiError> {
        self.run(|f| f.full_image(id))
    }

    /// The default file name for saving photo `id`, split into stem and extension.
    ///
    /// A part the photo's name lacks is "Photo" or ".jpg".
    pub fn save_as_name(&self, id: i64) -> Result<PhotoFileName, FeatureApiError> {
        self.run(|f| f.save_as_name(id)).map(|n| PhotoFileName { stem: n.stem, extension: n.extension })
    }

    /// A grid cell appeared: its thumbnail is fetched when missing.
    pub fn item_visible(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_item_visible(id))
    }

    /// The grid laid out with these cells visible.
    pub fn grid_layout(&self, visible: Vec<i64>) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_grid_layout(&visible))
    }

    /// A grid cell disappeared: its queued fetch is cancelled.
    pub fn item_hidden(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_item_hidden(id))
    }

    /// The gallery was left: every queued fetch is cancelled.
    pub fn gallery_left(&self) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_gallery_left())
    }

    /// The user opened an item; `true` when the viewer should open (its image is cached).
    pub fn open_item(&self, id: i64) -> Result<bool, FeatureApiError> {
        self.run(|f| f.open_item(id))
    }

    /// The viewer shows `id`: its neighbours are prefetched.
    pub fn viewer_selection(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_viewer_selection(id))
    }

    /// `id` was on screen in the viewer.
    pub fn viewer_item_shown(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_viewer_item_shown(id))
    }

    /// The viewer closed: the full images fetched for it and still pending are cancelled.
    pub fn viewer_closed(&self) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_viewer_closed())
    }

    /// An item's context menu opened: its full image is fetched ahead of an action.
    pub fn context_menu_opened(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.on_context_menu_opened(id))
    }

    /// The full image for a file action, fetched when not cached (waits at most 30 s); `None` when
    /// it did not arrive.
    pub async fn full_image_for_action(
        &self,
        id: i64,
        action: PhotoAction,
    ) -> Result<Option<Vec<u8>>, FeatureApiError> {
        Ok(self.feature()?.full_image_for_action(id, action.into()).await?)
    }

    /// Asks the phone to allow photos.
    pub async fn request_photo_access(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_photo_access().await?.into())
    }

    /// The full-access reminder's action.
    pub async fn request_full_access(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_full_access().await?.into())
    }

    /// Hides the full-access reminder until the phone next connects.
    pub fn dismiss_full_access_reminder(&self) -> Result<(), FeatureApiError> {
        self.run(|f| f.dismiss_full_access_reminder())
    }

    /// Asks the phone to allow deleting photos.
    pub async fn request_delete_permission(&self) -> Result<RequestOutcome, FeatureApiError> {
        Ok(self.feature()?.request_delete_permission().await?.into())
    }

    /// What deleting `id` needs first.
    pub async fn prepare_delete(&self, id: i64) -> Result<PhotoDeleteCheck, FeatureApiError> {
        Ok(match self.feature()?.prepare_delete(id).await? {
            DeletePrompt::Confirmation => PhotoDeleteCheck::Confirm,
            DeletePrompt::MissingPermission => PhotoDeleteCheck::MissingPermission,
        })
    }

    /// Deletes `id` on the phone (after the user confirmed).
    pub async fn delete(&self, id: i64) -> Result<PhotoDeleteResult, FeatureApiError> {
        Ok((&self.feature()?.delete(id).await?).into())
    }

    /// "Show photos on this Mac".
    pub fn set_consent(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_consent(on))
    }

    /// "Allow deleting photos".
    pub fn set_deletion_allowed(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_deletion_allowed(on))
    }

    /// "New-photo notifications".
    pub fn set_new_photo_notifications(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_new_photo_notifications(on))
    }
}
