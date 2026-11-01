//! Phone files for the UI and the file manager ([`crate::features::phone_files`]).
//!
//! It covers the selected phone's state and switch, the permission request, the refresh, and the
//! tree — items, children, changes, thumbnails, downloads, uploads, new folders, renames, moves and
//! deletes.
//!
//! The file manager names the
//! phone by its domain (the phone's id); a call for another phone than the selected one fails with
//! `NotRunning`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::api::engine::Engine;
use crate::features::phone_files::{
    DeleteError, FileTimes, Item, ItemChanges, OpError, PermissionOrigin, PhoneFiles, PhoneFilesError, PhoneFilesEvent,
    PhoneFilesStatus, SyncState, TransferError, UploadError,
};
use crate::features::shared::files_permission::PermissionResult;
use crate::types::DcgClient;

use super::{FeatureApiError, core_of, features_of, selected_phone};
use crate::sync::MutexExt;

impl From<PhoneFilesError> for FeatureApiError {
    fn from(e: PhoneFilesError) -> Self {
        match e {
            PhoneFilesError::UnknownPhone | PhoneFilesError::NotRunning => {
                FeatureApiError::NotAvailable { reason: e.to_string() }
            }
            PhoneFilesError::Store(e) => e.into(),
        }
    }
}

/// Where the latest metadata sync stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneFilesSync {
    Idle,
    Preparing,
    Syncing,
    Succeeded,
    /// The phone could not be reached.
    ConnectionFailed,
    Failed {
        message: String,
    },
}

impl From<SyncState> for PhoneFilesSync {
    fn from(s: SyncState) -> Self {
        match s {
            SyncState::Idle => Self::Idle,
            SyncState::Preparing => Self::Preparing,
            SyncState::Syncing => Self::Syncing,
            SyncState::Succeeded => Self::Succeeded,
            SyncState::ConnectionFailed => Self::ConnectionFailed,
            SyncState::Failed(message) => Self::Failed { message },
        }
    }
}

/// The selected phone's phone-files state.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneFilesState {
    /// The file manager's domain of the phone (its id).
    pub domain: String,
    /// The phone's name, the domain's display name.
    pub phone_name: String,
    /// The phone serves phone files: show the switch.
    pub capable: bool,
    /// The user's switch.
    pub enabled: bool,
    /// The domain exists.
    pub running: bool,
    pub permission_granted: bool,
    /// The phone is connected.
    pub available: bool,
    pub sync: PhoneFilesSync,
    pub initial_sync_done: bool,
    pub storage_free: i64,
    pub storage_total: i64,
    /// Bytes per gigabyte as the phone counts them.
    pub gigabyte_bytes: i64,
    /// The location of the phone's recycle bin in the domain (`\`-separated), when it has one.
    pub recycle_bin: Option<String>,
    /// Link to Windows is turned off on the phone.
    pub ltw_off: bool,
    /// The user chose not to be told where deleted files go.
    pub recycle_notice_off: bool,
}

/// One entry of the phone's tree.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneFileItem {
    /// `""` for the root.
    pub identifier: String,
    /// The parent folder's identifier (`""` = the root).
    pub parent: String,
    pub name: String,
    pub is_directory: bool,
    pub size: i64,
    /// Unix seconds.
    pub created: Option<i64>,
    pub modified: Option<i64>,
    /// `phone_files.v1.FileType`: 1 image, 2 video, 3 audio, 4 document, 5 playlist, 6 subtitle, 7 none.
    pub file_type: i32,
    pub content_version: String,
    pub metadata_version: String,
    pub has_thumbnail: bool,
    /// A folder the tree adds between the root and a phone item.
    pub synthetic: bool,
}

impl From<Item> for PhoneFileItem {
    fn from(i: Item) -> Self {
        Self {
            identifier: i.identifier,
            parent: i.parent,
            name: i.name,
            is_directory: i.is_directory,
            size: i.size,
            created: i.created,
            modified: i.modified,
            file_type: i.file_type,
            content_version: i.content_version,
            metadata_version: i.metadata_version,
            has_thumbnail: i.has_thumbnail,
            synthetic: i.synthetic,
        }
    }
}

/// What changed after a generation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneFileChanges {
    pub updated: Vec<PhoneFileItem>,
    pub removed: Vec<String>,
    pub generation: i64,
}

impl From<ItemChanges> for PhoneFileChanges {
    fn from(c: ItemChanges) -> Self {
        Self { updated: c.updated.into_iter().map(Into::into).collect(), removed: c.removed, generation: c.generation }
    }
}

/// A downloaded file.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneFileContent {
    /// A temporary file; the caller moves or deletes it.
    pub path: String,
    pub item: PhoneFileItem,
}

/// Why a tree operation failed, in the terms the file manager reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneFileFailure {
    /// The domain is not the selected phone's, or phone files is not running for it.
    NotRunning,
    UnknownItem,
    /// An item with the name already exists.
    Exists,
    /// The name stays on the Mac: it is not sent to the phone.
    Ignored,
    NotSupported,
    /// The Mac's tree is behind the phone; a refresh corrects it.
    NotInSync,
    PermissionRequired,
    /// The phone or its connection cannot be reached.
    PhoneUnavailable,
    LtwOff,
    Metered,
    PcLocked,
    TimedOut,
    FileNotFound,
    Cancelled,
    /// The same file is already being downloaded.
    InProgress,
    /// Some of a folder's items were not deleted.
    Partial,
    Failed,
}

/// Why a phone-files operation failed: a [`PhoneFileFailure`] and a message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum PhoneFileError {
    #[error("{message}")]
    Failure { failure: PhoneFileFailure, message: String },
}

fn failure(failure: PhoneFileFailure, message: impl ToString) -> PhoneFileError {
    PhoneFileError::Failure { failure, message: message.to_string() }
}

impl From<FeatureApiError> for PhoneFileError {
    fn from(e: FeatureApiError) -> Self {
        failure(PhoneFileFailure::NotRunning, e)
    }
}

impl From<PhoneFilesError> for PhoneFileError {
    fn from(e: PhoneFilesError) -> Self {
        match e {
            PhoneFilesError::Store(_) => failure(PhoneFileFailure::Failed, e),
            _ => failure(PhoneFileFailure::NotRunning, e),
        }
    }
}

impl From<TransferError> for PhoneFileError {
    fn from(e: TransferError) -> Self {
        use crate::features::phone_files::Status;
        let kind = match &e {
            TransferError::NoContextHandle
            | TransferError::InvalidContextHandle
            | TransferError::FailedConnectionLost
            | TransferError::FailedPlatformError => PhoneFileFailure::PhoneUnavailable,
            TransferError::ChannelCreationTimedOut => PhoneFileFailure::TimedOut,
            TransferError::UserSessionLocked => PhoneFileFailure::PcLocked,
            TransferError::MeteredSyncDisabled => PhoneFileFailure::Metered,
            TransferError::PermissionRequired | TransferError::Status(Status::NoPermission) => {
                PhoneFileFailure::PermissionRequired
            }
            TransferError::LtwOff => PhoneFileFailure::LtwOff,
            TransferError::FileNotFound | TransferError::Status(Status::NotFound) => PhoneFileFailure::FileNotFound,
            TransferError::UserCancelled => PhoneFileFailure::Cancelled,
            TransferError::InProgress => PhoneFileFailure::InProgress,
            TransferError::Status(Status::NotSupported) => PhoneFileFailure::NotSupported,
            _ => PhoneFileFailure::Failed,
        };

        failure(kind, e)
    }
}

impl From<UploadError> for PhoneFileError {
    fn from(e: UploadError) -> Self {
        let kind = match &e {
            UploadError::NotRunning => PhoneFileFailure::NotRunning,
            UploadError::UnknownFolder => PhoneFileFailure::UnknownItem,
            UploadError::Ignored => PhoneFileFailure::Ignored,
            UploadError::Exists => PhoneFileFailure::Exists,
            UploadError::Cancelled => PhoneFileFailure::Cancelled,
            UploadError::Source(_)
            | UploadError::NotAFile
            | UploadError::Transfer(_)
            | UploadError::Store(_)
            | UploadError::Tree(_) => PhoneFileFailure::Failed,
        };

        failure(kind, e)
    }
}

impl From<OpError> for PhoneFileError {
    fn from(e: OpError) -> Self {
        let kind = match &e {
            OpError::NotRunning => PhoneFileFailure::NotRunning,
            OpError::UnknownItem | OpError::UnknownFolder => PhoneFileFailure::UnknownItem,
            OpError::Exists => PhoneFileFailure::Exists,
            OpError::Ignored => PhoneFileFailure::Ignored,
            OpError::NotSupported => PhoneFileFailure::NotSupported,
            OpError::NotInSync => PhoneFileFailure::NotInSync,
            OpError::Phone(_) | OpError::Store(_) | OpError::Tree(_) | OpError::Item(_) => PhoneFileFailure::Failed,
        };

        failure(kind, e)
    }
}

impl From<DeleteError> for PhoneFileError {
    fn from(e: DeleteError) -> Self {
        let kind = match &e {
            DeleteError::NotRunning => PhoneFileFailure::NotRunning,
            DeleteError::UnknownItem => PhoneFileFailure::UnknownItem,
            DeleteError::NotSupported => PhoneFileFailure::NotSupported,
            DeleteError::NotInSync => PhoneFileFailure::NotInSync,
            DeleteError::Partial { .. } => PhoneFileFailure::Partial,
            DeleteError::Phone(_) | DeleteError::Store(_) => PhoneFileFailure::Failed,
        };

        failure(kind, e)
    }
}

/// How the files permission request ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneFilesPermissionResult {
    Granted,
    /// The phone shows its prompt.
    Required,
    Failed {
        reason: String,
    },
}

/// Where the user asked for the permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneFilesPermissionOrigin {
    /// The prompt shown when the feature needs the permission.
    Dialog,
    /// The status in the settings.
    StatusUi,
}

/// A downloaded file's progress.
#[uniffi::export(with_foreign)]
pub trait PhoneFileProgress: Send + Sync {
    /// Called as the download goes on: `done` of `total` bytes have arrived.
    fn progress(&self, done: u64, total: u64);
}

/// A change to phone files: re-read the state, or a phone's tree came, went, or changed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneFilesChange {
    /// Re-read `state`.
    StateChanged,
    /// The domain of the phone `domain` appeared (`running`) or went away.
    Running { domain: String, running: bool },
    /// The tree of `domain` changed: ask for the changes.
    ItemsChanged { domain: String },
    /// Every delete of a batch succeeded: tell the user deleted files go to the phone's recycle bin.
    RecycleBinNotice { domain: String },
}

impl From<&PhoneFilesEvent> for PhoneFilesChange {
    fn from(e: &PhoneFilesEvent) -> Self {
        match e {
            PhoneFilesEvent::StatusChanged { .. } => Self::StateChanged,
            PhoneFilesEvent::Running { phone, running } => {
                Self::Running { domain: phone.id.clone(), running: *running }
            }
            PhoneFilesEvent::ItemsChanged { phone } => Self::ItemsChanged { domain: phone.id.clone() },
            PhoneFilesEvent::RecycleBinNotice { phone } => Self::RecycleBinNotice { domain: phone.id.clone() },
        }
    }
}

/// Phone files with the selected phone.
#[derive(uniffi::Object)]
pub struct PhoneFilesApi {
    engine: Arc<Engine>,
    /// Running downloads and uploads by the caller's request id.
    requests: Mutex<HashMap<String, CancellationToken>>,
}

impl PhoneFilesApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine, requests: Mutex::default() })
    }

    fn feature(&self) -> Result<(Arc<PhoneFiles>, DcgClient), FeatureApiError> {
        let core = core_of(&self.engine)?;
        let phone = selected_phone(&core)?;
        Ok((features_of(&core)?.phone_files.clone(), phone))
    }

    /// The feature and the phone of `domain`, which must be the selected phone.
    fn domain(&self, domain: &str) -> Result<(Arc<PhoneFiles>, DcgClient), PhoneFileError> {
        let (feature, phone) = self.feature()?;
        if !phone.id.eq_ignore_ascii_case(domain) {
            return Err(failure(PhoneFileFailure::NotRunning, "the domain is not the selected phone's"));
        }
        Ok((feature, phone))
    }

    fn begin(&self, request_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        self.requests.lock_or_panic().insert(request_id.to_owned(), token.clone());
        token
    }

    fn end(&self, request_id: &str) {
        self.requests.lock_or_panic().remove(request_id);
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl PhoneFilesApi {
    /// The selected phone's phone-files state: domain, switch, permission, sync and storage.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<PhoneFilesState, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        let name = feature.phone_name(&phone).unwrap_or_default();
        let s: PhoneFilesStatus = feature.status(&phone)?;

        Ok(PhoneFilesState {
            domain: phone.id.clone(),
            phone_name: name,
            capable: s.capable,
            enabled: s.enabled,
            running: s.running,
            permission_granted: s.permission_granted,
            available: s.available,
            sync: s.sync.into(),
            initial_sync_done: s.initial_sync_done,
            storage_free: s.storage_free,
            storage_total: s.storage_total,
            gigabyte_bytes: s.gigabyte_bytes,
            recycle_bin: s.recycle_bin,
            ltw_off: s.ltw_off,
            recycle_notice_off: s.recycle_notice_off,
        })
    }

    /// The switch: on turns it off for the account's other phones.
    pub fn set_enabled(&self, on: bool) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.set_enabled(&phone, on)?)
    }

    /// Do not tell the user again where deleted files go.
    pub fn set_recycle_notice_off(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.set_recycle_notice_off(&phone)?)
    }

    /// Asks the phone for the files permission (the phone prompts).
    pub async fn request_permission(
        &self,
        origin: PhoneFilesPermissionOrigin,
    ) -> Result<PhoneFilesPermissionResult, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        let origin = match origin {
            PhoneFilesPermissionOrigin::Dialog => PermissionOrigin::Dialog,
            PhoneFilesPermissionOrigin::StatusUi => PermissionOrigin::StatusUi,
        };
        Ok(match feature.request_permission(&phone, origin).await? {
            PermissionResult::Granted => PhoneFilesPermissionResult::Granted,
            PermissionResult::Required => PhoneFilesPermissionResult::Required,
            other => PhoneFilesPermissionResult::Failed { reason: format!("{other:?}") },
        })
    }

    /// The user's refresh.
    pub fn refresh(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.refresh(&phone)?)
    }

    /// The file or folder `identifier` in the tree of `domain`.
    ///
    /// Fails with `UnknownItem` when the tree holds no such item.
    pub fn item(&self, domain: String, identifier: String) -> Result<PhoneFileItem, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        feature
            .item(&phone, &identifier)?
            .map(Into::into)
            .ok_or_else(|| failure(PhoneFileFailure::UnknownItem, "no such item".to_owned()))
    }

    /// The entries directly inside the folder `identifier`.
    pub fn children(&self, domain: String, identifier: String) -> Result<Vec<PhoneFileItem>, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature
            .children(&phone, &identifier)?
            .ok_or_else(|| failure(PhoneFileFailure::UnknownItem, "no such folder".to_owned()))?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// The tree's current generation.
    pub fn generation(&self, domain: String) -> Result<i64, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.generation(&phone)?)
    }

    /// What changed after `generation`; `None` when it is too old (read everything again).
    pub fn changes(&self, domain: String, generation: i64) -> Result<Option<PhoneFileChanges>, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.changes(&phone, generation)?.map(Into::into))
    }

    /// The file's thumbnail (JPEG); `None` when it has none.
    pub async fn thumbnail(&self, domain: String, identifier: String) -> Result<Option<Vec<u8>>, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.thumbnail(&phone, &identifier).await?)
    }

    /// Downloads the file into a temporary file. `request_id` names the download for [`Self::cancel`].
    pub async fn download(
        &self,
        domain: String,
        identifier: String,
        request_id: String,
        progress: Arc<dyn PhoneFileProgress>,
    ) -> Result<PhoneFileContent, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;

        let cancel = self.begin(&request_id);
        let report = Arc::new(move |done, total| progress.progress(done, total));
        let result = feature.download(&phone, &identifier, true, report, cancel).await;
        self.end(&request_id);

        let (path, item) = result?;
        Ok(PhoneFileContent { path: path.to_string_lossy().into_owned(), item: item.into() })
    }

    /// Copies the local file at `source` to the phone as `name` inside the folder `parent`; times are
    /// Unix milliseconds (`None`: the file's own). `request_id` names the upload for [`Self::cancel`].
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub async fn upload(
        &self,
        domain: String,
        parent: String,
        name: String,
        source: String,
        created_ms: Option<i64>,
        modified_ms: Option<i64>,
        request_id: String,
    ) -> Result<PhoneFileItem, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        let times = match (created_ms, modified_ms) {
            (Some(c), Some(m)) => Some(FileTimes { created_ms: c, modified_ms: m, accessed_ms: m }),
            (None, Some(m)) => Some(FileTimes { created_ms: m, modified_ms: m, accessed_ms: m }),
            _ => None,
        };

        let cancel = self.begin(&request_id);
        let result = feature.upload(&phone, &parent, &name, PathBuf::from(source), times, cancel).await;
        self.end(&request_id);

        Ok(result?.into())
    }

    /// Cancels the download or upload `request_id`, if it runs.
    pub fn cancel(&self, request_id: String) {
        if let Some(t) = self.requests.lock_or_panic().get(&request_id) {
            t.cancel();
        }
    }

    /// Creates the folder `name` inside the folder `parent` on the phone; returns the new folder.
    pub async fn create_folder(
        &self,
        domain: String,
        parent: String,
        name: String,
    ) -> Result<PhoneFileItem, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.create_folder(&phone, &parent, &name).await?.into())
    }

    /// Renames the item to `name` and moves it into the folder `parent`.
    pub async fn rename(
        &self,
        domain: String,
        identifier: String,
        parent: String,
        name: String,
    ) -> Result<PhoneFileItem, PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.rename(&phone, &identifier, &parent, &name).await?.into())
    }

    /// Deletes the item (a folder with everything in it).
    pub async fn delete(&self, domain: String, identifier: String) -> Result<(), PhoneFileError> {
        let (feature, phone) = self.domain(&domain)?;
        Ok(feature.delete(&phone, &identifier).await?)
    }
}
