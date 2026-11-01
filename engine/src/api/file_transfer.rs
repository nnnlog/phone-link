//! File transfer for the UI ([`crate::features::file_transfer`]).
//!
//! It covers the selected phone's settings, the transfers running with it, sending files and
//! cancelling the send, the files permission request, and the file or folder a received
//! notification opens.

use std::path::PathBuf;
use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::file_transfer::{
    FileTransfer, FileTransferError, FileTransferEvent, PhoneSettings, ReceiveProgress, SendError, SendOrigin,
    SendPhase, SendProgress, Transfers,
};
use crate::features::shared::files_permission::PermissionResult;

use super::{FeatureApiError, core_of, features_of, selected_phone};

impl From<FileTransferError> for FeatureApiError {
    fn from(e: FileTransferError) -> Self {
        match e {
            FileTransferError::UnknownPhone => FeatureApiError::NotAvailable { reason: e.to_string() },
            FileTransferError::Store(e) => e.into(),
        }
    }
}

/// The selected phone's file-transfer settings.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileTransferSettings {
    /// The phone can be sent files.
    pub share_capable: bool,
    /// Show the *Share* toggle.
    pub share_toggle_shown: bool,
    /// *Share*: the user may send files to the phone.
    pub share_enabled: bool,
    /// Offer the phone as a send target now (the send window, the share menu, drops).
    pub offered_as_send_target: bool,
    /// Show the received-files folder setting.
    pub folder_setting_shown: bool,
    /// Where files from the phone go; `None` when there is no folder.
    pub received_files_folder: Option<String>,
}

impl From<PhoneSettings> for FileTransferSettings {
    fn from(s: PhoneSettings) -> Self {
        Self {
            share_capable: s.share_capable,
            share_toggle_shown: s.share_toggle_shown,
            share_enabled: s.share_enabled,
            offered_as_send_target: s.offered_as_send_target,
            folder_setting_shown: s.folder_setting_shown,
            received_files_folder: s.received_files_folder.map(|p| p.to_string_lossy().into_owned()),
        }
    }
}

/// The step a send to the phone is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FileSendPhase {
    Connecting,
    Preparing,
    Sending,
}

/// A send to the phone in progress: its files and how many bytes are sent.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileSendProgress {
    pub phase: FileSendPhase,
    pub file_names: Vec<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A receive from the phone in progress: its files and how many bytes have arrived.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileReceiveProgress {
    /// The phone's file list has not arrived yet.
    pub starting: bool,
    pub file_names: Vec<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// The transfers running with the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FileTransfers {
    pub send: Option<FileSendProgress>,
    pub receive: Option<FileReceiveProgress>,
}

impl From<SendProgress> for FileSendProgress {
    fn from(s: SendProgress) -> Self {
        Self {
            phase: match s.phase {
                SendPhase::Connecting => FileSendPhase::Connecting,
                SendPhase::Preparing => FileSendPhase::Preparing,
                SendPhase::Sending => FileSendPhase::Sending,
            },
            file_names: s.file_names,
            bytes_done: s.bytes_done,
            bytes_total: s.bytes_total,
        }
    }
}

impl From<ReceiveProgress> for FileReceiveProgress {
    fn from(r: ReceiveProgress) -> Self {
        Self { starting: r.starting, file_names: r.file_names, bytes_done: r.bytes_done, bytes_total: r.bytes_total }
    }
}

impl From<Transfers> for FileTransfers {
    fn from(t: Transfers) -> Self {
        Self { send: t.send.map(Into::into), receive: t.receive.map(Into::into) }
    }
}

/// Where the user started a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FileSendOrigin {
    /// The app's send window, which shows the progress and the outcome: no notifications.
    Window,
    /// The share menu or a drop outside the window: notifications show the progress and the outcome.
    ShareMenu,
}

/// Why a send ended without success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FileSendFailure {
    /// Sending to the phone is turned off (the phone or its *Share*).
    TurnedOff,
    PhoneNotSupported,
    NoFile,
    AnotherTransferInProgress,
    /// The phone asks the user for the files permission (a notification offers to ask again).
    PermissionRequired,
    Cancelled,
    PcLocked,
    Metered,
    /// Any other failure; the message says which.
    Failed,
}

/// How a send ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FileSendResult {
    /// The phone completed `count` files.
    Sent {
        count: u32,
    },
    Failed {
        failure: FileSendFailure,
        message: String,
    },
}

impl From<Result<u32, SendError>> for FileSendResult {
    fn from(r: Result<u32, SendError>) -> Self {
        let e = match r {
            Ok(count) => return FileSendResult::Sent { count },
            Err(e) => e,
        };

        let failure = match &e {
            SendError::UnknownPhone | SendError::NotSupportedPc => FileSendFailure::TurnedOff,
            SendError::PhoneNotSupported => FileSendFailure::PhoneNotSupported,
            SendError::FileNotSupported => FileSendFailure::NoFile,
            SendError::AnotherTransferInProgress => FileSendFailure::AnotherTransferInProgress,
            SendError::PermissionRequired => FileSendFailure::PermissionRequired,
            SendError::Cancelled => FileSendFailure::Cancelled,
            SendError::PcLocked => FileSendFailure::PcLocked,
            SendError::Metered => FileSendFailure::Metered,
            _ => FileSendFailure::Failed,
        };

        FileSendResult::Failed { failure, message: e.to_string() }
    }
}

/// How the files permission request ended.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FilePermissionResult {
    Granted,
    /// The phone shows its prompt.
    Required,
    Failed {
        reason: String,
    },
}

/// A change to file transfer: re-read settings and transfers, or open or reveal a received file.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FileTransferChange {
    /// Re-read `settings` and `transfers`.
    Changed,
    /// Open the received file at `path`.
    OpenFile { path: String },
    /// Show `path` in the Finder.
    RevealInFinder { path: String },
}

impl From<&FileTransferEvent> for FileTransferChange {
    fn from(e: &FileTransferEvent) -> Self {
        match e {
            FileTransferEvent::Changed { .. } => FileTransferChange::Changed,
            FileTransferEvent::Open { path } => {
                FileTransferChange::OpenFile { path: path.to_string_lossy().into_owned() }
            }
            FileTransferEvent::Reveal { path } => {
                FileTransferChange::RevealInFinder { path: path.to_string_lossy().into_owned() }
            }
        }
    }
}

/// File transfer with the selected phone.
#[derive(uniffi::Object)]
pub struct FileTransferApi {
    engine: Arc<Engine>,
}

impl FileTransferApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn feature(&self) -> Result<(Arc<FileTransfer>, crate::types::DcgClient), FeatureApiError> {
        let core = core_of(&self.engine)?;
        let phone = selected_phone(&core)?;
        Ok((features_of(&core)?.file_transfer.clone(), phone))
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl FileTransferApi {
    /// The selected phone's file-transfer settings and whether it can be sent files now.
    ///
    /// Fails with `NotAvailable` when the phone is not in the account's device list.
    pub fn settings(&self) -> Result<FileTransferSettings, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.settings(&phone)?.into())
    }

    /// *Share*.
    pub fn set_share_enabled(&self, on: bool) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.set_share_enabled(&phone, on)?)
    }

    /// The received-files folder (an absolute path).
    pub fn set_received_files_folder(&self, path: String) -> Result<(), FeatureApiError> {
        let folder = PathBuf::from(&path);
        if !folder.is_absolute() {
            return Err(FeatureApiError::InvalidInput { reason: "not an absolute path".to_owned() });
        }

        let (feature, phone) = self.feature()?;
        Ok(feature.set_received_files_folder(&phone, folder)?)
    }

    /// The send and the receive running with the selected phone, each `None` when there is none.
    pub fn transfers(&self) -> Result<FileTransfers, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(feature.transfers(&phone).into())
    }

    /// Sends the files at `paths` to the phone and waits for the end.
    pub async fn send(&self, paths: Vec<String>, origin: FileSendOrigin) -> Result<FileSendResult, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        let origin = match origin {
            FileSendOrigin::Window => SendOrigin::Window,
            FileSendOrigin::ShareMenu => SendOrigin::ShareMenu,
        };
        Ok(feature.send(&phone, paths.into_iter().map(PathBuf::from).collect(), origin).await.into())
    }

    /// Cancels the running send, if any.
    pub fn cancel_send(&self) -> Result<(), FeatureApiError> {
        let (feature, phone) = self.feature()?;
        feature.cancel_send(&phone);
        Ok(())
    }

    /// Asks the phone for the files permission (the phone prompts).
    pub async fn request_permission(&self) -> Result<FilePermissionResult, FeatureApiError> {
        let (feature, phone) = self.feature()?;
        Ok(match feature.request_permission(&phone).await? {
            PermissionResult::Granted => FilePermissionResult::Granted,
            PermissionResult::Required => FilePermissionResult::Required,
            other => FilePermissionResult::Failed { reason: format!("{other:?}") },
        })
    }
}
