//! File content: a download pulls one file from the phone into a temporary file; an upload pushes a
//! new local file to the phone and waits for the phone's metadata of it. Both run on a new content
//! channel each, one at a time per phone, and try once more when the phone's connection was lost.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use rand::Rng;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use super::channel::{FilesChannel, TransferError, open_error};
use super::path;
use super::store::{Entry, Row, Store};
use super::sync::to_row;
use super::tree::Item;
use super::wire::{self, Status, Trigger, target};
use super::{Inner, PhoneFilesError};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext, new_guid_lower};
use phonelink_features::shared::db::StoreError;
use phonelink_nano::named_streams::{SendStreamsStatus, StreamReceiver, StreamSource, TransferStatus};
use phonelink_proto::phone_files::v1::FilesUpdateResponse;
use phonelink_proto::platform_sdk::v1::ProtoManifestEntry;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::ContextHandle;

/// A local file's times, as Unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileTimes {
    pub created_ms: i64,
    pub modified_ms: i64,
    pub accessed_ms: i64,
}

/// Why an upload did not complete.
#[derive(Debug, thiserror::Error)]
pub enum UploadError {
    #[error("phone files is not running for this phone")]
    NotRunning,
    #[error("the folder is not in the tree")]
    UnknownFolder,
    /// The name stays on the Mac: it is not sent to the phone.
    #[error("files with this name are not sent to the phone")]
    Ignored,
    #[error("an item with this name already exists")]
    Exists,
    #[error("the local file cannot be read: {0}")]
    Source(#[from] std::io::Error),
    #[error("the local file cannot be read: not a regular file")]
    NotAFile,
    #[error("cancelled")]
    Cancelled,
    /// The upload kept failing; the item stays on the Mac.
    #[error("the file could not be sent: {0}")]
    Transfer(#[from] TransferError),
    #[error("the file could not be sent: {0}")]
    Store(#[from] StoreError),
    #[error("the file could not be sent: {0}")]
    Tree(#[from] PhoneFilesError),
}

/// Receives a download into its temporary file.
struct Download {
    path: PathBuf,
    progress: Arc<dyn Fn(u64, u64) + Send + Sync>,
    done: Mutex<Option<oneshot::Sender<TransferStatus>>>,
    /// Why the temporary file could not be created, for the download's error.
    create_error: Mutex<Option<std::io::Error>>,
}

#[async_trait]
impl StreamReceiver for Download {
    async fn destination(&self, _id: &str, _size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        match tokio::fs::File::create(&self.path).await {
            Ok(f) => Some(Box::new(f)),
            Err(e) => {
                *self.create_error.lock_or_panic() = Some(e);
                None
            }
        }
    }

    fn progress(&self, _id: &str, done: u64, total: u64) {
        (self.progress)(done, total);
    }

    fn transfer_done(&self, status: TransferStatus) {
        if let Some(tx) = self.done.lock_or_panic().take() {
            let _ = tx.send(status);
        }
    }
}

/// Reads the file to upload.
struct Upload {
    path: PathBuf,
    /// Why the file could not be opened, for the upload's error.
    open_error: Mutex<Option<std::io::Error>>,
}

#[async_trait]
impl StreamSource for Upload {
    async fn open(&self, _entry: &ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        match tokio::fs::File::open(&self.path).await {
            Ok(f) => Some(Box::new(f)),
            Err(e) => {
                *self.open_error.lock_or_panic() = Some(e);
                None
            }
        }
    }
}

/// The wait before retry `attempt` (`0` = the first retry) of an upload: exponential back-off with
/// decorrelated jitter from `base`, as Polly computes it. `t = attempt + r` with `r` uniform in
/// `[0, 1)`; `f = 2^t · tanh(√(4t))`; the wait is `(f − prev) / 1.4 · base` and `prev` becomes `f`
/// (`prev` starts at `0` for each upload).
pub(super) fn retry_delay(attempt: u32, base: Duration, prev: &mut f64, r: f64) -> Duration {
    const MAX_NANOS: f64 = u64::MAX as f64;
    let t = attempt as f64 + r;
    let f = 2f64.powf(t) * (4.0 * t).sqrt().tanh();

    if f.is_infinite() {
        *prev = f;
        return Duration::from_nanos(u64::MAX);
    }

    let factor = f - *prev;
    *prev = f;
    // Whole 100 ns ticks, as .NET's TimeSpan keeps them.
    let ticks = (factor * (2925.0 / 4095.0) * (base.as_nanos() as f64 / 100.0)).min(MAX_NANOS / 100.0);
    Duration::from_nanos(ticks as u64 * 100)
}

/// A local file's times from its metadata (`0` for a time the file system does not keep).
fn local_times(meta: &std::fs::Metadata) -> FileTimes {
    FileTimes {
        created_ms: meta.created().map_or(0, |t| phonelink_core::time::unix_ms(t).max(0)),
        modified_ms: meta.modified().map_or(0, |t| phonelink_core::time::unix_ms(t).max(0)),
        accessed_ms: meta.accessed().map_or(0, |t| phonelink_core::time::unix_ms(t).max(0)),
    }
}

impl Inner {
    /// Creates and opens a content channel against `handle`, listening to its replies.
    async fn content_channel(
        &self,
        peer: &PeerDevice,
        handle: &ContextHandle,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<FilesChannel, TransferError> {
        if !self.unlocked().await {
            return Err(TransferError::UserSessionLocked);
        }

        let open = tokio::time::timeout(
            self.config.content_channel_timeout,
            self.channels.open(handle, wire::CONTENT_SCENARIO, &target::CONTENT_REPLIES, trace),
        );
        let opened = tokio::select! {
            r = open => r,
            _ = cancel.cancelled() => return Err(TransferError::UserCancelled),
        };

        match opened {
            Err(_) => Err(TransferError::ChannelCreationTimedOut),
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => {
                let error = open_error(&e, self.drm.metered_sync_disabled(&peer.client), true);
                if error == TransferError::InvalidContextHandle {
                    self.forget_handle(&peer.client);
                }
                Err(error)
            }
        }
    }

    /// Why a content channel closed under a transfer.
    fn closed_error(channel: &FilesChannel) -> TransferError {
        if channel.channel.device_closed() {
            TransferError::FailedConnectionLost
        } else {
            TransferError::FailedTransferClosed
        }
    }

    pub(super) async fn download(
        self: Arc<Self>,
        peer: &PeerDevice,
        identifier: &str,
        user: bool,
        progress: Arc<dyn Fn(u64, u64) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<(PathBuf, Item), TransferError> {
        let phone = &peer.client;
        let store = self.running_store(phone).map_err(|e| TransferError::Failed(e.to_string()))?;
        let Some(Entry::Row(row)) = store.entry(identifier).map_err(|e| TransferError::Failed(e.to_string()))? else {
            return Err(TransferError::FileNotFound);
        };

        if row.is_directory {
            return Err(TransferError::Failed("the item is a folder".into()));
        }
        if self.with_phone(phone, |p| p.ltw_off) {
            return Err(TransferError::LtwOff);
        }
        if self.drm.metered_sync_disabled(phone) {
            return Err(TransferError::MeteredSyncDisabled);
        }
        if !self.with_phone(phone, |p| p.downloads.insert(row.id)) {
            return Err(TransferError::InProgress);
        }

        // The caller's cancel, or the feature stopping for the phone; the watcher ends with the download.
        let run_cancel = self.with_phone(phone, |p| p.run_cancel.clone());
        let cancel = cancel.child_token();
        let finished = CancellationToken::new();
        let _finished = finished.clone().drop_guard();
        {
            let t = cancel.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = run_cancel.cancelled() => t.cancel(),
                    _ = finished.cancelled() => {}
                }
            });
        }

        let locks = self.locks(phone);
        let result = async {
            let _one = tokio::select! {
                g = locks.download.lock() => g,
                _ = cancel.cancelled() => return Err(TransferError::UserCancelled),
            };

            if let Err(e) = tokio::fs::create_dir_all(&self.config.content_dir).await {
                return Err(TransferError::Failed(format!("the download folder could not be created: {e}")));
            }

            // One context for the download (both attempts): the file manager's fetch.
            let trace = TraceContext::new_root(phonelink_core::types::trace_state(
                wire::trace_name::FETCH_DATA,
                wire::trace_name::OS_CALLBACK,
            ));

            let mut result = self.download_once(peer, &row, user, &progress, &trace, &cancel).await;
            let retry = match &result {
                Err(e) if e.retries_content() => {
                    warn!(%phone, id = row.id, error = %e, "download failed; once more");
                    true
                }
                _ => false,
            };
            if retry {
                result = self.download_once(peer, &row, user, &progress, &trace, &cancel).await;
            }
            result
        }
        .await;

        self.with_phone(phone, |p| p.downloads.remove(&row.id));

        let path = result?;
        let item = self
            .item_of_row(&store, &row, &self.phone_config(phone))
            .map_err(|e| TransferError::Failed(e.to_string()))?;
        Ok((path, item))
    }

    async fn download_once(
        &self,
        peer: &PeerDevice,
        row: &Row,
        user: bool,
        progress: &Arc<dyn Fn(u64, u64) + Send + Sync>,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, TransferError> {
        let handle = self.ensure_handle(peer, user, Trigger::GetContentStream, trace).await?;
        let mut channel = self.content_channel(peer, &handle, trace, cancel).await?;
        let mut errors = channel.take(target::FILES_STREAM_ERROR);

        let path = self.config.content_dir.join(new_guid_lower());
        let (done_tx, done) = oneshot::channel();
        let receiver = Arc::new(Download {
            path: path.clone(),
            progress: progress.clone(),
            done: Mutex::new(Some(done_tx)),
            create_error: Mutex::new(None),
        });
        let entry = ProtoManifestEntry { id: row.uri.clone(), size: row.size.max(0) as u64, metadata: HashMap::new() };

        let result = async {
            channel
                .channel
                .streams()
                .request_streams(vec![entry], receiver.clone(), trace)
                .map_err(|_| Self::closed_error(&channel))?;

            let mut closed = channel.channel.closed();
            tokio::pin!(done);
            let mut stream_error_logged = false;

            loop {
                tokio::select! {
                    status = &mut done => return match status {
                        Ok(TransferStatus::Success) => Ok(()),
                        Ok(TransferStatus::Cancelled) => Err(TransferError::UserCancelled),
                        Ok(status) => Err(TransferError::Failed(match receiver.create_error.lock_or_panic().take() {
                            Some(e) => format!("{status:?}; the download file could not be created: {e}"),
                            None => format!("{status:?}"),
                        })),
                        Err(_) => Err(TransferError::Failed("no transfer status".into())),
                    },
                    m = errors.recv() => match m {
                        Some(m) if wire::is_file_not_found(&m.body) => return Err(TransferError::FileNotFound),
                        Some(_) if !stream_error_logged => {
                            stream_error_logged = true;
                            warn!(phone = %peer.client, "stream error from the phone; the download goes on");
                        }
                        Some(_) | None => {}
                    },
                    _ = closed.wait_for(|c| *c) => return Err(Self::closed_error(&channel)),
                    _ = cancel.cancelled() => return Err(TransferError::UserCancelled),
                }
            }
        }
        .await;

        channel.channel.close();
        match result {
            Ok(()) => Ok(path),
            Err(e) => {
                let _ = tokio::fs::remove_file(&path).await;
                Err(e)
            }
        }
    }

    /// The upload's gate is open: the phone enabled and available, the feature running with its first
    /// sync done, a current configuration and the permission.
    fn upload_gate(&self, peer: &PeerDevice) -> Result<bool, StoreError> {
        let phone = &peer.client;
        let data = self.store(phone)?.data()?;
        let (running, up_to_date) = self.with_phone(phone, |p| (p.running, p.config_up_to_date));

        Ok(self.phone_enabled(phone)
            && self.available(phone)
            && running
            && data.initial_sync_done
            && up_to_date
            && data.permission_granted)
    }

    pub(super) async fn upload(
        self: Arc<Self>,
        peer: &PeerDevice,
        parent: &str,
        name: &str,
        source: PathBuf,
        times: Option<FileTimes>,
        cancel: CancellationToken,
    ) -> Result<Item, UploadError> {
        let phone = &peer.client;
        if path::ignored(name) {
            return Err(UploadError::Ignored);
        }

        let store = self.running_store(phone).map_err(|_| UploadError::NotRunning)?;
        let location = self.new_location(&store, parent, name)?;
        let meta = tokio::fs::metadata(&source).await?;
        if !meta.is_file() {
            return Err(UploadError::NotAFile);
        }

        let times = times.unwrap_or_else(|| local_times(&meta));
        let run_cancel = self.with_phone(phone, |p| p.run_cancel.clone());
        let mut attempt = 0u32;
        let mut backoff = 0.0;

        loop {
            self.wait_for_gate(peer, &cancel, &run_cancel).await?;
            let result = self.upload_attempt(peer, &location, &source, meta.len(), times, &cancel).await?;

            match result {
                Ok(row) => return self.finish_upload(phone, &store, &row),
                Err(TransferError::UserCancelled) => return Err(UploadError::Cancelled),
                Err(e) if attempt >= self.config.upload_retries => return Err(UploadError::Transfer(e)),
                Err(e) => {
                    let delay = retry_delay(attempt, self.config.upload_backoff, &mut backoff, rand::rng().random());
                    attempt += 1;
                    warn!(%phone, error = %e, attempt, delay_ms = delay.as_millis(), "upload failed; trying again");
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return Err(UploadError::Cancelled),
                        _ = run_cancel.cancelled() => return Err(UploadError::NotRunning),
                    }
                }
            }
        }
    }

    /// The location of the new item `name` in folder `parent`; an error when the folder is unknown or
    /// the location is taken.
    fn new_location(&self, store: &Store, parent: &str, name: &str) -> Result<String, UploadError> {
        let folder = self.folder(store, parent).map_err(UploadError::from)?.ok_or(UploadError::UnknownFolder)?;
        let location = path::join(&folder.location, name);
        if store.row_at(&location).map_err(UploadError::from)?.is_some() {
            return Err(UploadError::Exists);
        }
        Ok(location)
    }

    /// One attempt under the phone's upload lock, tried once more when the content is to be retried;
    /// the outer error when cancelled while waiting for the lock.
    async fn upload_attempt(
        &self,
        peer: &PeerDevice,
        location: &str,
        source: &std::path::Path,
        size: u64,
        times: FileTimes,
        cancel: &CancellationToken,
    ) -> Result<Result<Row, TransferError>, UploadError> {
        let locks = self.locks(&peer.client);
        let _one = tokio::select! {
            g = locks.upload.lock() => g,
            _ = cancel.cancelled() => return Err(UploadError::Cancelled),
        };

        // One context per attempt; Phone Link on Windows names nothing in it.
        let trace = TraceContext::new_root(Vec::new());
        let mut r = self.upload_once(peer, location, source, size, times, &trace, cancel).await;

        let retry = match &r {
            Err(e) if e.retries_content() => {
                warn!(phone = %peer.client, error = %e, "upload failed; once more");
                true
            }
            _ => false,
        };
        if retry {
            r = self.upload_once(peer, location, source, size, times, &trace, cancel).await;
        }
        Ok(r)
    }

    /// Stores the uploaded item's row and returns it as an item.
    fn finish_upload(&self, phone: &DcgClient, store: &Store, row: &Row) -> Result<Item, UploadError> {
        if let Err(e) = store.apply(std::slice::from_ref(row), &[], None) {
            error!(%phone, error = %e, "uploaded item not stored; it shows after the next sync");
        }
        self.items_changed(phone);
        self.item_of_row(store, row, &self.phone_config(phone)).map_err(UploadError::from)
    }

    async fn wait_for_gate(
        &self,
        peer: &PeerDevice,
        cancel: &CancellationToken,
        run_cancel: &CancellationToken,
    ) -> Result<(), UploadError> {
        let mut unreadable_logged = false;
        loop {
            let notified = self.gate.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            match self.upload_gate(peer) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                // Every wake reads it again: logged once per wait.
                Err(e) if !unreadable_logged => {
                    unreadable_logged = true;
                    warn!(phone = %peer.client, error = %e, "phone-files state unreadable; the upload waits");
                }
                Err(_) => {}
            }

            if !self.with_phone(&peer.client, |p| p.running) {
                return Err(UploadError::NotRunning);
            }

            tokio::select! {
                _ = notified => {}
                _ = cancel.cancelled() => return Err(UploadError::Cancelled),
                _ = run_cancel.cancelled() => return Err(UploadError::NotRunning),
            }
        }
    }

    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    async fn upload_once(
        &self,
        peer: &PeerDevice,
        location: &str,
        source: &std::path::Path,
        size: u64,
        times: FileTimes,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<Row, TransferError> {
        let handle = self.ensure_handle(peer, false, Trigger::SetContentStream, trace).await?;
        let mut channel = self.content_channel(peer, &handle, trace, cancel).await?;
        let mut replies = channel.take(target::CREATE_FILE_RESPONSE);
        let mut errors = channel.take(target::FILES_STREAM_ERROR);

        let remote = path::remote(location);
        let entry = ProtoManifestEntry {
            id: wire::upload_id(peer.info.client_version.as_deref(), &remote),
            size,
            metadata: wire::upload_metadata(&remote, times.created_ms, times.modified_ms, times.accessed_ms),
        };

        let (streams, trace) = (channel.channel.clone(), trace.clone());
        let source = Arc::new(Upload { path: source.to_owned(), open_error: Mutex::new(None) });
        let (pushed_tx, mut pushed) = mpsc::unbounded_channel();
        {
            let source = source.clone();
            tokio::spawn(async move {
                let (status, _) = streams.send_streams(vec![entry], source.as_ref(), &trace).await;
                let _ = pushed_tx.send(status);
            });
        }

        let mut closed = channel.channel.closed();
        let result = async {
            let mut sent = false;
            let mut stream_error_logged = false;

            loop {
                tokio::select! {
                    status = pushed.recv(), if !sent => {
                        if status != Some(SendStreamsStatus::Success) {
                            let status = status.map_or_else(|| "no status".to_owned(), |s| format!("{s:?}"));
                            return Err(TransferError::Failed(match source.open_error.lock_or_panic().take() {
                                Some(e) => format!("{status}; the local file could not be opened: {e}"),
                                None => status,
                            }));
                        }
                        // The phone answers once it wrote the file.
                        sent = true;
                    }
                    m = replies.recv() => {
                        let Some(m) = m else { return Err(Self::closed_error(&channel)) };
                        let reply = FilesUpdateResponse::decode(m.body.as_slice())
                            .map_err(|e| TransferError::Failed(format!("the phone's reply does not parse: {e}")))?;
                        return match Status::from_wire(reply.status) {
                            Status::Success => reply.target_metadata.ok_or(TransferError::Status(Status::IoError)),
                            other => Err(TransferError::Status(other)),
                        };
                    }
                    m = errors.recv() => if m.is_some() && !stream_error_logged {
                        stream_error_logged = true;
                        warn!(phone = %peer.client, "stream error from the phone; the upload goes on");
                    },
                    _ = closed.wait_for(|c| *c) => return Err(Self::closed_error(&channel)),
                    _ = cancel.cancelled() => return Err(TransferError::UserCancelled),
                }
            }
        }
        .await;

        channel.channel.close();
        let metadata = result?;
        let row = to_row(&metadata)
            .map_err(|e| TransferError::Failed(format!("the phone's item is not placed in the tree: {e}")))?;

        let modified = metadata.modified_time.as_ref().map(|t| t.seconds);
        let local_modified = times.modified_ms.div_euclid(1000);
        if modified != Some(local_modified) || metadata.size != size as i64 {
            return Err(TransferError::Failed(format!(
                "the phone's copy differs from the local file: {} of {size} bytes, modified {modified:?} for {local_modified}",
                metadata.size
            )));
        }
        Ok(row)
    }
}

#[cfg(test)]
mod tests;
