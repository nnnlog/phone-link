//! A send to the phone: the gate, one send per phone, the silent permission check, the context
//! handle, the send channel, the file list, then the phone's requests for the files, their bytes and
//! the phone's completion. The user may cancel it until it ends. Also the send window's request for
//! the files permission.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_trait::async_trait;
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace, warn};

use super::wire::{self, Control, FileEntry, scenario};
use super::{
    DcgClient, FileTransferError, HandleError, Inner, PermissionTrigger, SendPhase, SendProgress, Side, key,
    share_capable,
};
use phonelink_core::edge::LockState;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{TraceContext, new_guid_lower};
use phonelink_features::shared::files_permission::PermissionResult;
use phonelink_features::shared::nano_channels::{ChannelOpenError, OpenedChannel, TransferChannel};
use phonelink_nano::device::{AcceptanceReason, DeviceConnectionError, PlatformChannelError};
use phonelink_nano::named_streams::{SendStreamsStatus, SingleStreamStatus, StreamSource};
use phonelink_proto::file_transfer::v1::FileShareFileStatus;
use phonelink_proto::platform_sdk::v1::ProtoManifestEntry;
use phonelink_transport::auth::PeerDevice;

/// Takes the phone out of the send window's requests in flight when its request ends, however it ends.
struct WindowRequest<'a> {
    inner: &'a Inner,
    phone: DcgClient,
}

impl Drop for WindowRequest<'_> {
    fn drop(&mut self) {
        self.inner.state.lock_or_recover().permission_requests.remove(&self.phone);
    }
}

/// The failure a channel that did not open stands for. A `send` takes the phone's `NOT_SUPPORTED` as
/// another transfer in progress; every other refusal is a rejection (or metered, when the phone's
/// connection is).
fn channel_failure(e: ChannelOpenError, metered: bool, send: bool) -> SendError {
    match e.acceptance() {
        Some(AcceptanceReason::NotSupported) if send => SendError::AnotherTransferInProgress,
        Some(_) if metered => SendError::Metered,
        Some(_) => SendError::ChannelRejected(e),
        None => match e {
            ChannelOpenError::Channel(PlatformChannelError::CreateRequestTimedOut)
            | ChannelOpenError::Device(DeviceConnectionError::ChannelOpenTimeout) => SendError::ChannelTimedOut,
            ChannelOpenError::Channel(
                PlatformChannelError::Timeout
                | PlatformChannelError::DuplicateChannel
                | PlatformChannelError::Channel(_),
            ) => SendError::ChannelFailed(e),
            _ if metered => SendError::Metered,
            _ => SendError::ChannelRejected(e),
        },
    }
}

/// Where the user started a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOrigin {
    /// The app's send window, which shows the progress and the outcome itself: no notifications.
    Window,
    /// The share menu or a drop outside the window: the progress and the outcome are notifications.
    ShareMenu,
}

/// Why a send ended without success.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    /// The phone is disabled, or *Share* is off for it.
    #[error("sending to this phone is turned off")]
    NotSupportedPc,
    /// The phone cannot receive files this way.
    #[error("the phone does not support receiving files")]
    PhoneNotSupported,
    /// Nothing to send: no regular file among the items.
    #[error("no file to send")]
    FileNotSupported,
    #[error("another transfer with the phone is in progress")]
    AnotherTransferInProgress,
    /// The phone asks the user for the files permission.
    #[error("the phone needs the files permission")]
    PermissionRequired,
    #[error("the files permission request failed: {0}")]
    Permission(String),
    #[error("the phone did not hand over its connection in time")]
    HandleTimedOut,
    #[error("the phone's connection could not be obtained")]
    HandleFailed,
    #[error("cancelled")]
    Cancelled,
    #[error("the transfer channel did not open in time")]
    ChannelTimedOut,
    #[error("the Mac is locked")]
    PcLocked,
    #[error("the phone does not transfer on a metered connection")]
    Metered,
    #[error("the phone refused the transfer channel: {0}")]
    ChannelRejected(#[source] ChannelOpenError),
    #[error("the transfer channel failed to open: {0}")]
    ChannelFailed(#[source] ChannelOpenError),
    #[error("the transfer channel was lost")]
    ChannelLost,
    /// The push of the files did not succeed: its status, and why a file did not open when one did not.
    #[error("the files could not be sent: {0}")]
    SendStreamsFailed(String),
    /// The phone's message, and what is wrong with it.
    #[error("the phone sent a message the Mac does not understand: {0}")]
    InvalidMessage(&'static str),
    #[error("a transfer error occurred")]
    TransferError,
    #[error("the transfer channel closed")]
    ChannelClosed,
    #[error("{0}")]
    Internal(String),
}

impl From<HandleError> for SendError {
    fn from(e: HandleError) -> Self {
        match e {
            HandleError::Permission(PermissionResult::Required) => SendError::PermissionRequired,
            HandleError::Permission(other) => SendError::Permission(format!("{other:?}")),
            HandleError::TimedOut => SendError::HandleTimedOut,
            HandleError::Cancelled => SendError::Cancelled,
            HandleError::Failed => SendError::HandleFailed,
        }
    }
}

/// One file of a send.
struct OutFile {
    id: String,
    path: PathBuf,
    name: String,
    size: u64,
    status: FileShareFileStatus,
    /// The most bytes of the file read for the phone so far.
    sent: Arc<AtomicU64>,
}

/// Reads the files' bytes for the channel and counts them.
struct Source {
    files: HashMap<String, (PathBuf, Arc<AtomicU64>)>,
    /// Why the first file that did not open did not, for the send's error.
    open_error: Mutex<Option<io::Error>>,
}

#[async_trait]
impl StreamSource for Source {
    async fn open(&self, entry: &ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>> {
        let (path, counter) = self.files.get(&entry.id)?;
        match tokio::fs::File::open(path).await {
            Ok(file) => Some(Box::new(Counting { inner: file, read: 0, counter: counter.clone() })),
            Err(e) => {
                self.open_error.lock_or_panic().get_or_insert(e);
                None
            }
        }
    }
}

struct Counting<R> {
    inner: R,
    read: u64,
    counter: Arc<AtomicU64>,
}

impl<R: AsyncRead + Unpin> AsyncRead for Counting<R> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &polled {
            self.read += (buf.filled().len() - before) as u64;
            self.counter.fetch_max(self.read, Ordering::Relaxed);
        }
        polled
    }
}

/// Why an item the user chose is left out of a send.
#[derive(Debug, thiserror::Error)]
enum Skipped {
    #[error("not a regular file")]
    NotAFile,
    #[error("unreadable: {0}")]
    Unreadable(#[source] io::Error),
    #[error("no file name")]
    NoName,
}

/// The regular files among `paths`, with their names and sizes, and why each other item is left out.
async fn read_files(paths: Vec<PathBuf>) -> (Vec<OutFile>, Vec<Skipped>) {
    let mut files = Vec::new();
    let mut skipped = Vec::new();

    for path in paths {
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_file() => m,
            Ok(_) => {
                skipped.push(Skipped::NotAFile);
                continue;
            }
            Err(e) => {
                skipped.push(Skipped::Unreadable(e));
                continue;
            }
        };
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            skipped.push(Skipped::NoName);
            continue;
        };

        files.push(OutFile {
            id: new_guid_lower(),
            path,
            name,
            size: meta.len(),
            status: FileShareFileStatus::NotStarted,
            sent: Arc::default(),
        });
    }

    (files, skipped)
}

fn progress(files: &[OutFile]) -> (u64, u64) {
    let done = files.iter().map(|f| f.sent.load(Ordering::Relaxed).min(f.size)).sum();
    (done, files.iter().map(|f| f.size).sum())
}

impl Inner {
    /// Runs a send to `peer` to its end.
    pub(super) async fn send(
        self: Arc<Self>,
        peer: PeerDevice,
        paths: Vec<PathBuf>,
        origin: SendOrigin,
    ) -> Result<u32, SendError> {
        let phone = peer.client.clone();

        if !self.phone_enabled(&phone) {
            return Err(SendError::NotSupportedPc);
        }
        if !share_capable(&peer) {
            return Err(SendError::PhoneNotSupported);
        }
        let data = self.data(&phone).map_err(|e| SendError::Internal(e.to_string()))?;
        if !data.share_enabled {
            return Err(SendError::NotSupportedPc);
        }

        let (mut files, skipped) = read_files(paths).await;
        if files.is_empty() {
            return Err(SendError::FileNotSupported);
        }

        let cancel = CancellationToken::new();
        {
            let mut state = self.state.lock_or_panic();
            if state.sending.contains_key(&key(&phone)) {
                return Err(SendError::AnotherTransferInProgress);
            }
            state.sending.insert(key(&phone), cancel.clone());
        }

        log_send_started(&phone, files.len(), origin, &skipped);
        let file_names = files.iter().map(|f| f.name.clone()).collect();
        let bytes_total = files.iter().map(|f| f.size).sum();
        self.set_transfers(&phone, |t| {
            t.send = Some(SendProgress { phase: SendPhase::Connecting, file_names, bytes_done: 0, bytes_total })
        });
        if origin == SendOrigin::ShareMenu {
            self.notify_send_state(&peer, SendPhase::Connecting, 0.0);
        }

        let mut channel = None;
        let result = self.run_send(&peer, &mut files, &cancel, origin, &mut channel).await;
        let result = match result {
            // A cancel after every byte was read for the phone counts as sent.
            Err(SendError::Cancelled) if files.iter().all(|f| f.sent.load(Ordering::Relaxed) >= f.size) => {
                Ok(files.len() as u32)
            }
            other => other,
        };

        if let Some(c) = channel {
            c.close();
        }
        self.state.lock_or_panic().sending.remove(&key(&phone));
        self.set_transfers(&phone, |t| t.send = None);
        log_send_end(&phone, &result);
        if origin == SendOrigin::ShareMenu {
            self.notify_send_end(&peer, &result, files.len());
        }

        result
    }

    pub(super) fn cancel_send(&self, phone: &DcgClient) {
        if let Some(cancel) = self.state.lock_or_panic().sending.get(&key(phone)) {
            info!(%phone, "send cancelled by the user");
            cancel.cancel();
        }
    }

    async fn run_send(
        &self,
        peer: &PeerDevice,
        files: &mut [OutFile],
        cancel: &CancellationToken,
        origin: SendOrigin,
        opened: &mut Option<Arc<TransferChannel>>,
    ) -> Result<u32, SendError> {
        // One trace for the send's permission requests, its channel and its manifests, with an empty
        // trace state, as Phone Link on Windows sends it.
        let trace = TraceContext::new_root(Vec::new());
        self.check_send_permission(peer, origin, &trace, cancel).await?;
        let handle = self.ensure_handle(peer, Side::Send, &trace, Some(cancel.clone())).await?;
        let OpenedChannel { channel, control: mut messages } =
            self.open_channel(&handle, wire::SEND_SCENARIO, &trace, cancel, true).await?;
        *opened = Some(channel.clone());
        let mut closed = channel.closed();

        self.send_file_list(&peer.client, &channel, files)?;
        self.send_phase(peer, files, SendPhase::Preparing, origin);

        let source = Arc::new(Source {
            files: files.iter().map(|f| (f.id.clone(), (f.path.clone(), f.sent.clone()))).collect(),
            open_error: Mutex::new(None),
        });
        let (pushed_tx, mut pushed) = mpsc::unbounded_channel::<SendStreamsStatus>();
        let mut tick = tokio::time::interval(self.config.progress_interval);
        let mut session = SendSession {
            inner: self,
            peer,
            files,
            channel: &channel,
            source,
            pushed: pushed_tx,
            trace: &trace,
            origin,
            reported: (u64::MAX, u64::MAX),
            phase: SendPhase::Preparing,
        };

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(SendError::Cancelled),
                m = messages.recv() => {
                    let Some(m) = m else { return Err(SendError::ChannelClosed) };
                    if let Some(result) = session.on_control(&m.body) {
                        return result;
                    }
                }
                status = pushed.recv() => {
                    if status != Some(SendStreamsStatus::Success) {
                        return Err(session.streams_failed(status));
                    }
                }
                _ = closed.wait_for(|c| *c) => return Err(SendError::ChannelClosed),
                _ = tick.tick() => session.report_progress(),
            }
        }
    }

    /// The silent permission check before a send.
    async fn check_send_permission(
        &self,
        peer: &PeerDevice,
        origin: SendOrigin,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<(), SendError> {
        let result = self
            .permission(peer, true, PermissionTrigger::CheckFilePermission, Some(trace), Some(cancel.clone()))
            .await;
        self.record_send_permission(&peer.client, &result);

        match result {
            PermissionResult::Granted => Ok(()),
            _ if cancel.is_cancelled() => Err(SendError::Cancelled),
            PermissionResult::Required => {
                // The send window shows that the phone needs the permission itself.
                if origin == SendOrigin::ShareMenu {
                    self.notify_grant_permission(peer);
                }
                Err(SendError::PermissionRequired)
            }
            other => Err(SendError::Permission(format!("{other:?}"))),
        }
    }

    /// The send window's request for the files permission: sent only while the phone needs it, and
    /// one at a time; a grant records the permission as held.
    pub(super) async fn window_permission(&self, peer: &PeerDevice) -> Result<PermissionResult, FileTransferError> {
        let phone = &peer.client;
        let data = self.data(phone)?;
        if !peer.info.is_android() || data.files_permission_granted {
            return Ok(PermissionResult::Granted);
        }

        if !self.state.lock_or_panic().permission_requests.insert(key(phone)) {
            debug!(%phone, "files permission request already in flight; not sent again");
            return Ok(PermissionResult::Required);
        }
        let _in_flight = WindowRequest { inner: self, phone: key(phone) };

        let result = self.permission(peer, false, PermissionTrigger::UserGrantFilesPermission, None, None).await;
        if result == PermissionResult::Granted {
            self.set_files_permission(phone, true);
        }
        Ok(result)
    }

    /// Sends the file list that starts the transfer.
    fn send_file_list(&self, phone: &DcgClient, channel: &TransferChannel, files: &[OutFile]) -> Result<(), SendError> {
        let start = Control {
            transfer_id: new_guid_lower(),
            scenario: scenario::TO_PHONE.to_owned(),
            sender_name: self.sender_name(),
            received_file_count: 0,
            content: files
                .iter()
                .map(|f| FileEntry {
                    file_id: f.id.clone(),
                    file_name: f.name.clone(),
                    file_size: f.size as i64,
                    status: FileShareFileStatus::NotStarted,
                })
                .collect(),
            is_concurrent_mode: true,
        };

        channel
            .send_message(phonelink_features::shared::nano_channels::CONTROL_TARGET, &start.encode())
            .map_err(|_| SendError::ChannelLost)?;
        debug!(%phone, transfer = start.transfer_id, "file list sent");
        Ok(())
    }

    /// Publishes the send's phase and progress (and its notification for a share-menu send).
    fn send_phase(&self, peer: &PeerDevice, files: &[OutFile], phase: SendPhase, origin: SendOrigin) {
        let (done, total) = progress(files);
        self.set_transfers(&peer.client, |t| {
            if let Some(s) = t.send.as_mut() {
                s.phase = phase;
                s.bytes_done = done;
                s.bytes_total = total;
            }
        });

        if origin == SendOrigin::ShareMenu {
            let fraction = if total == 0 { 0.0 } else { done as f64 / total as f64 };
            self.notify_send_state(peer, phase, fraction);
        }
    }

    /// Creates and opens the channel `scenario` against `handle`. A `send` maps the phone's
    /// `NOT_SUPPORTED` to another transfer in progress.
    pub(super) async fn open_channel(
        &self,
        handle: &phonelink_transport::platform::ContextHandle,
        scenario: &str,
        trace: &TraceContext,
        cancel: &CancellationToken,
        send: bool,
    ) -> Result<OpenedChannel, SendError> {
        let phone = handle.partner.clone();
        let lock_state = {
            let lock = self.env.edges.lock.clone();
            tokio::task::spawn_blocking(move || lock.lock_state()).await.unwrap_or(LockState::Unknown)
        };
        if lock_state != LockState::Unlocked {
            return Err(SendError::PcLocked);
        }

        let open = tokio::time::timeout(self.config.channel_timeout, self.channels.open(handle, scenario, trace));
        let opened = tokio::select! {
            r = open => r,
            _ = cancel.cancelled() => return Err(SendError::Cancelled),
        };

        match opened {
            Err(_) => Err(SendError::ChannelTimedOut),
            Ok(Ok(channel)) => Ok(channel),
            Ok(Err(e)) => Err(channel_failure(e, self.drm.metered_sync_disabled(&phone), send)),
        }
    }
}

/// The send's start, and the items it left out.
fn log_send_started(phone: &DcgClient, file_count: usize, origin: SendOrigin, skipped: &[Skipped]) {
    info!(%phone, file_count, origin_kind = ?origin, "send started");
    if let Some(first) = skipped.first() {
        warn!(%phone, skipped_count = skipped.len(), error = %first, "items left out of the send; the other files are sent");
    }
}

/// Logs the send's end when it sent or was cancelled. A failure is not logged here: the app gets it in
/// the result and logs it.
fn log_send_end(phone: &DcgClient, result: &Result<u32, SendError>) {
    match result {
        Ok(sent) => info!(%phone, outcome = "sent", sent_count = *sent, "send ended"),
        Err(SendError::Cancelled) => info!(%phone, outcome = "cancelled", "send ended"),
        Err(_) => {}
    }
}

/// Takes the phone's status of each listed file that is one of ours.
fn apply_statuses(files: &mut [OutFile], content: &[FileEntry]) {
    for entry in content {
        if let Some(f) = files.iter_mut().find(|f| f.id == entry.file_id) {
            f.status = entry.status;
        }
    }
}

/// The state of one send after its file list: the files, the channel and the progress reported.
struct SendSession<'a> {
    inner: &'a Inner,
    peer: &'a PeerDevice,
    files: &'a mut [OutFile],
    channel: &'a Arc<TransferChannel>,
    source: Arc<Source>,
    pushed: mpsc::UnboundedSender<SendStreamsStatus>,
    trace: &'a TraceContext,
    origin: SendOrigin,
    reported: (u64, u64),
    phase: SendPhase,
}

impl SendSession<'_> {
    /// One control message from the phone; `Some` ends the send with that result.
    fn on_control(&mut self, body: &[u8]) -> Option<Result<u32, SendError>> {
        let phone = &self.peer.client;
        let Ok(control) = Control::decode(body) else {
            // A control message that does not parse ends the transfer.
            return Some(Err(SendError::InvalidMessage("a control message does not parse")));
        };

        debug!(
            %phone,
            raw_scenario = control.scenario,
            file_count = control.content.len(),
            "control message from the phone"
        );

        match control.scenario.as_str() {
            scenario::TO_PHONE_FILE => self.push_requested(&control),
            scenario::TO_PHONE_COMPLETED => {
                apply_statuses(self.files, &control.content);
                let sent = self.files.iter().filter(|f| f.status == FileShareFileStatus::Completed).count();
                Some(Ok(sent as u32))
            }
            scenario::TO_PHONE_REJECTED => Some(Err(SendError::AnotherTransferInProgress)),
            _ => Some(Err(SendError::InvalidMessage("a control message of a scenario the send does not expect"))),
        }
    }

    /// The phone asks for files: pushes the ones in progress; `Some` ends the send with that result.
    fn push_requested(&mut self, control: &Control) -> Option<Result<u32, SendError>> {
        let phone = &self.peer.client;
        apply_statuses(self.files, &control.content);
        if self.channel.is_closed() {
            return Some(Err(SendError::ChannelLost));
        }

        let entries: Vec<ProtoManifestEntry> = self
            .files
            .iter()
            .filter(|f| f.status == FileShareFileStatus::InProgress)
            .map(|f| ProtoManifestEntry {
                id: f.id.clone(),
                size: f.size,
                metadata: HashMap::from([(wire::FILE_NAME_METADATA.to_owned(), f.name.clone())]),
            })
            .collect();

        if self.phase == SendPhase::Sending {
            debug!(%phone, file_count = entries.len(), "the phone asks for more files");
        } else {
            info!(%phone, file_count = entries.len(), "the phone asks for files; sending");
        }
        self.phase = SendPhase::Sending;
        self.inner.send_phase(self.peer, self.files, self.phase, self.origin);

        let (channel, source, done, trace) =
            (self.channel.clone(), self.source.clone(), self.pushed.clone(), self.trace.clone());
        tokio::spawn(async move {
            let (status, results) = channel.send_streams(entries, source.as_ref(), &trace).await;
            debug!(
                ?status,
                file_count = results.len(),
                failed_count = results.iter().filter(|(_, s)| *s != SingleStreamStatus::Success).count(),
                "files pushed"
            );
            let _ = done.send(status);
        });
        None
    }

    /// The progress tick: publishes the progress when it changed.
    fn report_progress(&mut self) {
        let phone = &self.peer.client;
        let now = progress(self.files);
        if let Some(metrics) = self.channel.metrics() {
            trace!(%phone, read_bytes = now.0, raw_metrics = %metrics, "send progress");
        }

        if now != self.reported {
            self.reported = now;
            self.inner.send_phase(self.peer, self.files, self.phase, self.origin);
        }
    }

    /// The error of a push that did not succeed (`None`: no status came).
    fn streams_failed(&self, status: Option<SendStreamsStatus>) -> SendError {
        let status = status.map_or_else(|| "no status".to_owned(), |s| format!("{s:?}"));
        match self.source.open_error.lock_or_panic().take() {
            Some(e) => SendError::SendStreamsFailed(format!("{status}; a file did not open: {e}")),
            None => SendError::SendStreamsFailed(status),
        }
    }
}

#[cfg(test)]
mod tests;
