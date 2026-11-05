//! The messages of phone files.
//!
//! It covers the `/DeviceResourceManager` requests of type `FILES_METADATA` and `PERMISSIONS` and
//! how their replies are read, the tag-4 publication, the requests and replies on the three
//! `NamedStreams` channels, and the upload manifest.

use std::collections::HashMap;

use prost::Message;

use phonelink_core::types::new_guid_lower;
use phonelink_proto::permissions::v1::PermissionsResponseMessage;
use phonelink_proto::phone_files::v1::{
    BatchedDeleteRequest, CreateFolderRequest, FileMetadata, FilesConfigurationSyncRequest, FilesDeleteRequest,
    FilesDeleteResponse, FilesGetRequest, FilesGetResponse, FilesResponsePayload, FilesResponseStatus,
    FilesStreamsMessage, FilesSyncRequest, FilesSyncResponse, FilesSyncType, FilesUpdateRequest, FilesUpdateResponse,
    GetFilesMetadataSyncRequest, GetThumbnailRequest, PrefetchThumbnailRequest, RenameRequest, StreamMessageStatus,
    files_get_request, files_sync_request, files_update_request,
};
use phonelink_transport::platform::ClientVersion;
use phonelink_transport::platform::version::client_version_passes;

/// The MSAEP tag of the phone's files publication (configuration, sync updates, storage), which also
/// delivers the context handle the feature's channels use.
pub const FILES_TAG: i16 = 4;
/// The MSAEP tag of the phone's permission publication.
pub const PERMISSIONS_TAG: i16 = 3;

/// `NamedStreams` scenario of file content (download and upload).
pub const CONTENT_SCENARIO: &str = "fileTransfer_1EAA053C-0354-4DF8-AA05-888312CCC57E";
/// `NamedStreams` scenario of thumbnails.
pub const THUMBNAIL_SCENARIO: &str = "thumbnailTransfer_2FAB064D-0465-5EF9-BB16-999423DDD68F";
/// `NamedStreams` scenario of metadata requests. The id holds letters that are not hex; it is sent as
/// it is.
pub const REQUEST_SCENARIO: &str = "filesRequestTransfer_3GAC175E-1576-6FA0-CC27-000534EEE79G";

/// Channel message targets.
pub mod target {
    pub const FILES_REQUEST: &str = "/filesRequest";
    pub const FILES_REQUEST_RESPONSE: &str = "/filesRequestResponse";
    pub const GET_THUMBNAIL_REQUEST: &str = "/getThumbnailRequest";
    pub const GET_THUMBNAIL_RESPONSE: &str = "/getThumbnailResponse";
    pub const PREFETCH_THUMBNAILS_REQUEST: &str = "/prefetchThumbnailsRequest";
    pub const PREFETCH_THUMBNAILS_RESPONSE: &str = "/prefetchThumbnailsResponse";
    pub const THUMBNAIL_ERROR_RESPONSE: &str = "/thumbnailErrorResponse";
    pub const CREATE_FILE_RESPONSE: &str = "/com.microsoft.mmx.agents.files.transport.create_file_response";
    pub const FILES_STREAM_ERROR: &str = "/com.microsoft.mmx.agents.files.transport.files_stream_error";

    /// Every reply target of the thumbnail channel.
    pub const THUMBNAIL_REPLIES: [&str; 3] =
        [GET_THUMBNAIL_RESPONSE, PREFETCH_THUMBNAILS_RESPONSE, THUMBNAIL_ERROR_RESPONSE];
    /// The messages the content channel receives.
    pub const CONTENT_REPLIES: [&str; 2] = [CREATE_FILE_RESPONSE, FILES_STREAM_ERROR];
}

/// Upload manifest metadata keys.
pub mod manifest_key {
    pub const PATH: &str = "fileTransferPath";
    pub const CREATED: &str = "fileTransferCreationTimestamp";
    pub const MODIFIED: &str = "fileTransferModifiedTimestamp";
    pub const ACCESSED: &str = "fileTransferAccessedTimestamp";
}

/// The trace-state names (`ypScenarioId`, `ypTriggerId`).
pub mod trace_name {
    pub const CONFIG_REQUEST: &str = "CDEH.PhoneFiles.DeviceConfigRequest";
    pub const REQUEST_PERMISSION: &str = "CDEH.PhoneFiles.RequestPermission";
    pub const CREATE_DIRECTORY: &str = "CDEH.PhoneFiles.CreateDirectory";
    pub const RENAME: &str = "CDEH.PhoneFiles.RenameFileScenarioId";
    pub const DELETE: &str = "CDEH.PhoneFiles.DeleteFileScenarioId";
    pub const DELETE_BATCH: &str = "CDEH.PhoneFiles.DeleteBatchScenarioId";
    pub const GET_THUMBNAIL: &str = "CDEH.PhoneFiles.GetThumbnailScenarioId";
    /// A full refresh that sends a full request.
    pub const FULL_SYNC_TO_LOCAL_DB: &str = "CDEH.PhoneFiles.FullSyncToLocalDB";
    /// A full refresh that sends an incremental request (the sequence is known).
    pub const INCREMENTAL_SYNC_TO_LOCAL_DB: &str = "CDEH.PhoneFiles.IncrementalSyncToLocalDB";
    pub const INCREMENTAL_SYNC: &str = "CDEH.PhoneFiles.IncrementalSync";
    /// A download for the file manager.
    pub const FETCH_DATA: &str = "CDEH.PhoneFiles.FetchData";
    pub const OS_CALLBACK: &str = "OSCallback";
    /// The trigger of a single thumbnail's `GET`.
    pub const THUMBNAIL_REQUESTED: &str = "ThumbnailRequested";
    pub const PREFETCH_THUMBNAILS: &str = "CDEH.PhoneFiles.PrefetchThumbnailsScenarioId";
}

/// What made the PC send a request; it names the request's trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    DeviceInitialization,
    FilesServiceStarted,
    DeviceBecameAvailable,
    DeviceUpdated,
    DeviceReconnectedSync,
    FilesSyncPublished,
    RefreshClicked,
    SequenceNumberExpired,
    GetContentStream,
    SetContentStream,
    PermissionDialogClicked,
    PermissionStatusUiClicked,
    GetThumbnail,
    PrefetchThumbnails,
}

impl Trigger {
    pub fn wire(self) -> &'static str {
        match self {
            Self::DeviceInitialization => "DeviceInitialization",
            Self::FilesServiceStarted => "FilesServiceStarted",
            Self::DeviceBecameAvailable => "DeviceBecameAvailable",
            Self::DeviceUpdated => "DeviceUpdated",
            Self::DeviceReconnectedSync => "DeviceReconnectedSync",
            Self::FilesSyncPublished => "FilesSyncPublished",
            Self::RefreshClicked => "RefreshClicked",
            Self::SequenceNumberExpired => "SequenceNumberExpired",
            Self::GetContentStream => "GetContentStream",
            Self::SetContentStream => "SetContentStream",
            Self::PermissionDialogClicked => "PermissionDialogClicked",
            Self::PermissionStatusUiClicked => "PermissionStatusUIClicked",
            Self::GetThumbnail => trace_name::GET_THUMBNAIL,
            Self::PrefetchThumbnails => trace_name::PREFETCH_THUMBNAILS,
        }
    }
}

/// A `FilesResponseStatus` as the phone sent it. Every value but `SUCCESS` is a failure, except that an
/// item of a batch delete answered `DirectoryNotEmpty` counts as deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    NoPermission,
    NotFound,
    ChecksumMismatch,
    IoError,
    NotSupported,
    RequestNotSet,
    FeatureDisabled,
    PlatformError,
    Cancelled,
    SyncFailed,
    BatchRestartRequired,
    DirectoryNotEmpty,
    /// `UNSPECIFIED` or a value outside the known ones.
    Other(i32),
}

impl Status {
    pub fn from_wire(v: i32) -> Self {
        match FilesResponseStatus::try_from(v) {
            Ok(FilesResponseStatus::Success) => Self::Success,
            Ok(FilesResponseStatus::NoPermission) => Self::NoPermission,
            Ok(FilesResponseStatus::NotFound) => Self::NotFound,
            Ok(FilesResponseStatus::ChecksumMismatch) => Self::ChecksumMismatch,
            Ok(FilesResponseStatus::IoError) => Self::IoError,
            Ok(FilesResponseStatus::NotSupported) => Self::NotSupported,
            Ok(FilesResponseStatus::RequestNotSet) => Self::RequestNotSet,
            Ok(FilesResponseStatus::FeatureDisabled) => Self::FeatureDisabled,
            Ok(FilesResponseStatus::PlatformError) => Self::PlatformError,
            Ok(FilesResponseStatus::Cancelled) => Self::Cancelled,
            Ok(FilesResponseStatus::SyncFailed) => Self::SyncFailed,
            Ok(FilesResponseStatus::BatchRestartRequired) => Self::BatchRestartRequired,
            Ok(FilesResponseStatus::DirectoryNotEmpty) => Self::DirectoryNotEmpty,
            Ok(FilesResponseStatus::Unspecified) | Err(_) => Self::Other(v),
        }
    }
}

// ------------------------------------------------------------------------------------------------
// `/DeviceResourceManager` payloads
// ------------------------------------------------------------------------------------------------

/// `FilesSyncRequest { config_sync_request {} }` (bytes `0a 00`).
pub fn config_sync_request() -> Vec<u8> {
    FilesSyncRequest { request: Some(files_sync_request::Request::ConfigSyncRequest(FilesConfigurationSyncRequest {})) }
        .encode_to_vec()
}

/// The status of a config-sync reply; an empty body is `IO_ERROR`.
pub fn config_sync_status(payload: &[u8]) -> Status {
    if payload.is_empty() {
        return Status::IoError;
    }
    match FilesSyncResponse::decode(payload) {
        Ok(r) => Status::from_wire(r.status),
        Err(_) => Status::IoError,
    }
}

/// A tag-3 publication's `PermissionsResponseMessage`.
pub fn parse_permissions(additional: &[u8]) -> Result<PermissionsResponseMessage, prost::DecodeError> {
    PermissionsResponseMessage::decode(additional)
}

/// A tag-4 publication's `FilesResponsePayload`.
pub fn parse_publication(additional: &[u8]) -> Result<FilesResponsePayload, prost::DecodeError> {
    FilesResponsePayload::decode(additional)
}

/// A metadata sync request: full (`seq_num = -1`) or incremental from `seq_num`.
///
/// Every optional field the PC sets is encoded, also when `false` or `0`; `no_engagement` is set
/// only by an incremental sync of its own (a full refresh leaves it out).
pub fn metadata_sync_request(
    full: bool,
    seq_num: i64,
    user_initiated: bool,
    no_engagement: Option<bool>,
) -> FilesGetRequest {
    FilesGetRequest {
        request: Some(files_get_request::Request::FilesMetadataSyncRequest(GetFilesMetadataSyncRequest {
            sync_type: if full { FilesSyncType::FullSync } else { FilesSyncType::IncrementalSync } as i32,
            seq_num: Some(seq_num),
            is_user_initiated: Some(user_initiated),
            supports_batched_response: None,
            batch_continuation_token: None,
            is_no_engagement_sync: no_engagement,
        })),
        request_id: new_guid_lower(),
    }
}

/// One batch of a batched full sync.
pub fn batched_full_sync_request(token: &str) -> FilesGetRequest {
    FilesGetRequest {
        request: Some(files_get_request::Request::FilesMetadataSyncRequest(GetFilesMetadataSyncRequest {
            sync_type: FilesSyncType::FullSync as i32,
            seq_num: Some(-1),
            is_user_initiated: Some(false),
            supports_batched_response: Some(true),
            batch_continuation_token: Some(token.to_owned()),
            is_no_engagement_sync: None,
        })),
        request_id: new_guid_lower(),
    }
}

/// A single thumbnail (the outer `request_id` is not set).
pub fn thumbnail_request(uri: &str, checksum: i64, size: i32) -> FilesGetRequest {
    FilesGetRequest {
        request: Some(files_get_request::Request::ThumbnailRequest(GetThumbnailRequest {
            target_uri: uri.to_owned(),
            target_checksum: checksum,
            width: size,
            height: size,
            request_id: new_guid_lower(),
        })),
        request_id: String::new(),
    }
}

/// One item of a prefetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefetchItem {
    pub id: i64,
    pub uri: String,
    pub checksum: i64,
    /// The item's full local location; the phone ignores it.
    pub path: String,
}

pub fn prefetch_request(items: &[PrefetchItem], size: i32) -> FilesGetRequest {
    FilesGetRequest {
        request: Some(files_get_request::Request::PrefetchThumbnailRequest(PrefetchThumbnailRequest {
            metadata: items
                .iter()
                .map(|i| FileMetadata {
                    id: i.id,
                    uri: i.uri.clone(),
                    checksum: i.checksum,
                    path: i.path.clone(),
                    is_directory: false,
                    ..Default::default()
                })
                .collect(),
            request_id: new_guid_lower(),
            width: size,
            height: size,
        })),
        request_id: String::new(),
    }
}

/// A `FilesGetResponse` read: its status must be `SUCCESS`.
pub fn parse_get_response(payload: &[u8]) -> Result<FilesGetResponse, Status> {
    if payload.is_empty() {
        return Err(Status::IoError);
    }
    let reply = FilesGetResponse::decode(payload).map_err(|_| Status::IoError)?;
    match Status::from_wire(reply.status) {
        Status::Success => Ok(reply),
        other => Err(other),
    }
}

/// `FilesUpdateRequest { create_folder_request }`; the parent's `uri`/`checksum` empty / 0 when it has
/// none.
pub fn create_folder_request(parent_uri: &str, parent_checksum: i64, name: &str) -> Vec<u8> {
    FilesUpdateRequest {
        request: Some(files_update_request::Request::CreateFolderRequest(CreateFolderRequest {
            folder_uri: parent_uri.to_owned(),
            folder_checksum: parent_checksum,
            folder_name: name.to_owned(),
        })),
    }
    .encode_to_vec()
}

/// `FilesUpdateRequest { rename_request }` to the remote path `target_path`.
pub fn rename_request(uri: &str, checksum: i64, target_path: &str) -> Vec<u8> {
    FilesUpdateRequest {
        request: Some(files_update_request::Request::RenameRequest(RenameRequest {
            target_uri: uri.to_owned(),
            target_checksum: checksum,
            target_path: target_path.to_owned(),
        })),
    }
    .encode_to_vec()
}

/// A `FilesUpdateResponse` read; an empty body is `IO_ERROR`.
pub fn update_response(payload: &[u8]) -> Result<FilesUpdateResponse, Status> {
    if payload.is_empty() {
        return Err(Status::IoError);
    }
    FilesUpdateResponse::decode(payload).map_err(|_| Status::IoError)
}

/// One `FilesDeleteRequest { target_uri, target_checksum }`.
pub fn delete_request(uri: &str, checksum: i64) -> Vec<u8> {
    FilesDeleteRequest { target_uri: uri.to_owned(), target_checksum: checksum, ..Default::default() }.encode_to_vec()
}

/// A batch `FilesDeleteRequest` of `(uri, checksum)` in order.
pub fn delete_batch_request(items: &[(String, i64)]) -> Vec<u8> {
    FilesDeleteRequest {
        is_batch: true,
        batch_requests: items
            .iter()
            .map(|(uri, checksum)| BatchedDeleteRequest { target_uri: uri.clone(), target_checksum: *checksum })
            .collect(),
        ..Default::default()
    }
    .encode_to_vec()
}

/// A `FilesDeleteResponse` read; an empty body is `IO_ERROR`.
pub fn delete_response(payload: &[u8]) -> Result<FilesDeleteResponse, Status> {
    if payload.is_empty() {
        return Err(Status::IoError);
    }
    FilesDeleteResponse::decode(payload).map_err(|_| Status::IoError)
}

/// The stream-error message reports the file is not found.
pub fn is_file_not_found(body: &[u8]) -> bool {
    FilesStreamsMessage::decode(body)
        .is_ok_and(|m| m.stream_message_status == StreamMessageStatus::ErrorFileNotFound as i32)
}

/// The first phone version whose upload manifest entries are named by a GUID (older ones by the
/// remote path).
pub const GUID_UPLOAD_ID_VERSION: ClientVersion = ClientVersion::new(1, 24062, 83, 0);

/// The manifest id of an upload to a phone of `client_version`.
pub fn upload_id(client_version: Option<&str>, remote_path: &str) -> String {
    if client_version_passes(client_version, GUID_UPLOAD_ID_VERSION) {
        new_guid_lower()
    } else {
        remote_path.to_owned()
    }
}

/// The metadata of an upload's manifest entry; times are Unix milliseconds.
pub fn upload_metadata(
    remote_path: &str,
    created_ms: i64,
    modified_ms: i64,
    accessed_ms: i64,
) -> HashMap<String, String> {
    HashMap::from([
        (manifest_key::PATH.to_owned(), remote_path.to_owned()),
        (manifest_key::CREATED.to_owned(), created_ms.to_string()),
        (manifest_key::MODIFIED.to_owned(), modified_ms.to_string()),
        (manifest_key::ACCESSED.to_owned(), accessed_ms.to_string()),
    ])
}

#[cfg(test)]
mod tests;
