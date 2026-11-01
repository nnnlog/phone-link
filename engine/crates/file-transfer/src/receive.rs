//! A receive from the phone: the `/fileshare_android2cdeh_phase1` request and its answer, then — with
//! nothing more said to the phone — the context handle and the receive channel, the phone's file list,
//! pulling the files into the received-files folder, the completion message and the received
//! notification. Files that did not complete are deleted.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use super::send::SendError;
use super::wire::{self, Control, FileEntry, ReceiveRequest, scenario};
use super::{Inner, ReceiveProgress, key};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::shared::nano_channels::{OpenedChannel, TransferChannel};
use phonelink_nano::channel::ChannelError;
use phonelink_nano::named_streams::{SingleStreamStatus, StreamReceiver, TransferStatus};
use phonelink_proto::file_transfer::v1::FileShareFileStatus;
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoManifestEntry};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{DrmReceiver, DrmReceiverError, DrmRequest};

/// Why a receive session ended without its files.
#[derive(Debug, thiserror::Error)]
pub(super) enum ReceiveError {
    #[error("the phone cancelled")]
    Cancelled,
    #[error("a control message does not parse")]
    UnparsedControl,
    /// A control message whose scenario (as it arrived) the receive does not expect.
    #[error("a control message of a scenario the receive does not expect")]
    UnexpectedScenario(String),
    #[error("the phone listed a file name the Mac does not accept")]
    BadFileName,
    #[error("the transfer channel was lost")]
    ChannelLost,
    #[error("the files could not be requested: {0}")]
    RequestFailed(#[source] ChannelError),
    /// A file's stream did not succeed; `cause` is why the Mac refused its destination, when it did.
    #[error(
        "a file did not arrive: {status:?}{}",
        .cause.as_ref().map(|c| format!("; {c}")).unwrap_or_default()
    )]
    FileFailed { status: SingleStreamStatus, cause: Option<DestinationError> },
    #[error("the transfer did not complete: {0:?}")]
    TransferFailed(TransferStatus),
    #[error("the transfer channel closed")]
    ChannelClosed,
}

/// Why the Mac refused the destination of a pulled file.
#[derive(Debug, thiserror::Error)]
pub(super) enum DestinationError {
    #[error("the phone did not list the file")]
    Unlisted,
    #[error("there is no received-files folder (no home folder)")]
    NoFolder,
    #[error("the received-files folder could not be created: {0}")]
    Folder(#[source] io::Error),
    #[error("the file name leaves its folder")]
    LeavesFolder,
    #[error("the file could not be created: {0}")]
    Create(#[source] io::Error),
    #[error("no free file name")]
    NoFreeName,
}

/// The `/fileshare_android2cdeh_phase1` receiver.
pub(super) struct Receiver(pub(super) Weak<Inner>);

#[async_trait]
impl DrmReceiver for Receiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        let Some(inner) = self.0.upgrade() else { return Ok(None) };
        Ok(inner.receive_request(request))
    }
}

/// One file of a receive.
#[derive(Debug, Clone)]
struct InFile {
    name: String,
    size: u64,
    status: FileShareFileStatus,
    done: u64,
    path: Option<PathBuf>,
}

/// What the channel reports to the session.
#[derive(Debug)]
enum SinkEvent {
    Progress { id: String, done: u64 },
    EntryDone { id: String, status: SingleStreamStatus },
    TransferDone(TransferStatus),
}

/// The destinations of the pulled files.
struct Sink {
    folder: Option<PathBuf>,
    files: Arc<Mutex<HashMap<String, InFile>>>,
    events: mpsc::UnboundedSender<SinkEvent>,
    /// Why the first refused destination was refused, for the session's end.
    refusal: Arc<Mutex<Option<DestinationError>>>,
}

/// Creates `name` (or the first free candidate) exclusively in `folder`.
async fn create_destination(folder: &Path, name: &str) -> Result<(PathBuf, tokio::fs::File), DestinationError> {
    tokio::fs::create_dir_all(folder).await.map_err(DestinationError::Folder)?;
    let folder_text = folder.to_string_lossy().to_lowercase();

    for n in 0.. {
        let path = folder.join(wire::candidate(name, n));
        if !parent_is(&path, &folder_text) {
            return Err(DestinationError::LeavesFolder);
        }
        match create_new(&path).await {
            Created::File(file) => return Ok((path, file)),
            Created::Taken => {}
            Created::Failed(e) => return Err(DestinationError::Create(e)),
        }
    }
    Err(DestinationError::NoFreeName)
}

/// Whether `path`'s parent, lowercased, is `folder_text`.
fn parent_is(path: &Path, folder_text: &str) -> bool {
    path.parent().is_some_and(|p| p.to_string_lossy().to_lowercase() == folder_text)
}

/// What creating one candidate file did.
enum Created {
    File(tokio::fs::File),
    /// A file or a directory of that name exists.
    Taken,
    Failed(io::Error),
}

/// Creates `path` exclusively.
async fn create_new(path: &Path) -> Created {
    match tokio::fs::OpenOptions::new().write(true).create_new(true).open(path).await {
        Ok(file) => Created::File(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Created::Taken,
        // A directory of that name.
        Err(_) if tokio::fs::metadata(path).await.is_ok_and(|m| m.is_dir()) => Created::Taken,
        Err(e) => Created::Failed(e),
    }
}

#[async_trait]
impl StreamReceiver for Sink {
    async fn destination(&self, id: &str, _size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>> {
        match self.create(id).await {
            Ok(file) => Some(Box::new(file)),
            Err(e) => {
                self.refusal.lock_or_panic().get_or_insert(e);
                None
            }
        }
    }

    fn progress(&self, id: &str, done: u64, _total: u64) {
        let _ = self.events.send(SinkEvent::Progress { id: id.to_owned(), done });
    }

    fn entry_done(&self, id: &str, status: SingleStreamStatus) {
        let _ = self.events.send(SinkEvent::EntryDone { id: id.to_owned(), status });
    }

    fn transfer_done(&self, status: TransferStatus) {
        let _ = self.events.send(SinkEvent::TransferDone(status));
    }
}

impl Sink {
    /// Creates the destination of the listed file `id` and records its path.
    async fn create(&self, id: &str) -> Result<tokio::fs::File, DestinationError> {
        let name = self.files.lock_or_panic().get(id).map(|f| f.name.clone()).ok_or(DestinationError::Unlisted)?;
        let folder = self.folder.as_ref().ok_or(DestinationError::NoFolder)?;
        let (path, file) = create_destination(folder, &name).await?;

        debug!(id, "receiving a file");
        if let Some(f) = self.files.lock_or_panic().get_mut(id) {
            f.path = Some(path);
        }
        Ok(file)
    }
}

impl Inner {
    /// Answers a receive request, and starts the receive when it is answered `OK`.
    fn receive_request(&self, request: DrmRequest) -> Option<Vec<u8>> {
        let is_get = request.request_type == ProtoDeviceResourceRequestType::Get;
        let session = match wire::receive_request(is_get, &request.path, &request.payload) {
            ReceiveRequest::Refuse(body) => {
                warn!(partner = %request.source, raw_path = request.path, is_get, "malformed receive request refused");
                return Some(body);
            }
            ReceiveRequest::Session(session) => session,
        };

        let peer = self.claim_receive(&request.source)?;
        let phone = peer.client.clone();
        info!(%phone, session, "receive request accepted");
        self.set_transfers(&phone, |t| {
            t.receive = Some(ReceiveProgress { starting: true, file_names: Vec::new(), bytes_done: 0, bytes_total: 0 })
        });

        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                inner.run_receive(peer).await;
            }
        });

        Some(wire::init_ok(&session))
    }

    /// The Android phone of the account `source` names, marked as receiving; `None` (no reply) when there
    /// is none or a receive with it is running.
    fn claim_receive(&self, source: &DcgClient) -> Option<PeerDevice> {
        let Some(peer) = self.env.peers.peer(source).filter(|p| p.info.is_android()) else {
            warn!(partner = %source, "receive request from no Android phone of the account; no reply");
            return None;
        };

        let phone = &peer.client;
        if !self.state.lock_or_panic().receiving.insert(key(phone)) {
            debug!(%phone, "a receive with the phone is running; no reply");
            return None;
        }
        Some(peer)
    }

    async fn run_receive(self: Arc<Self>, peer: PeerDevice) {
        let phone = peer.client.clone();
        self.show_folder_setting(&phone);

        // One trace for the receive: the handle request, the channel and its manifest; Phone Link on
        // Windows names nothing in it.
        let trace = TraceContext::new_root(Vec::new());
        let channel = self.open_receive_channel(&peer, &trace).await;
        let OpenedChannel { channel, control } = match channel {
            Ok(c) => c,
            Err(e) => {
                warn!(%phone, error = %e, "receive not started; the phone's files are not received");
                self.end_receive(&phone);
                return;
            }
        };

        let end = self.receive_session(&peer, &channel, control, &trace).await;
        log_receive_end(&phone, &end);
        self.end_receive(&phone);

        let delay = self.config.receive_dispose_delay;
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            channel.close();
        });
    }

    /// Makes the folder setting visible for `phone`.
    fn show_folder_setting(&self, phone: &DcgClient) {
        match self.update(phone, |d| d.show_folder_setting = true) {
            Ok(true) => self.changed(phone),
            Ok(false) => {}
            Err(e) => error!(%phone, error = %e, "folder-setting visibility not stored; the setting stays hidden"),
        }
    }

    /// The context handle, then the receive channel over it.
    async fn open_receive_channel(&self, peer: &PeerDevice, trace: &TraceContext) -> Result<OpenedChannel, SendError> {
        let handle = self.ensure_handle(peer, trace, None).await.map_err(SendError::from)?;
        self.open_channel(&handle, wire::RECEIVE_SCENARIO, trace, &self.shutdown.child_token(), false).await
    }

    /// Clears the running receive with `phone`.
    fn end_receive(&self, phone: &DcgClient) {
        self.state.lock_or_panic().receiving.remove(&key(phone));
        self.set_transfers(phone, |t| t.receive = None);
    }

    async fn receive_session(
        &self,
        peer: &PeerDevice,
        channel: &Arc<TransferChannel>,
        mut messages: phonelink_nano::device::Listener,
        trace: &TraceContext,
    ) -> Result<u32, ReceiveError> {
        let phone = &peer.client;
        let folder = match self.data(phone) {
            Ok(d) => self.folder(&d),
            Err(e) => {
                warn!(%phone, error = %e, "settings unreadable; the default folder is used");
                self.config.default_folder.clone()
            }
        };

        let mut closed = channel.closed();
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let mut session = ReceiveSession {
            inner: self,
            phone,
            channel,
            trace,
            folder,
            files: Arc::default(),
            refusal: Arc::default(),
            order: Vec::new(),
            events: events_tx,
            transfer_id: None,
            received: 0,
        };

        let mut tick = tokio::time::interval(self.config.progress_interval);
        let end = loop {
            tokio::select! {
                biased;
                m = messages.recv() => {
                    let Some(m) = m else { break Err(ReceiveError::ChannelClosed) };
                    if let Some(end) = session.on_control(&m.body) {
                        break end;
                    }
                }
                e = events.recv() => {
                    // The session holds a sender, so the stream never ends here.
                    let Some(e) = e else { break Err(ReceiveError::TransferFailed(TransferStatus::Error)) };
                    if let Some(end) = session.on_sink_event(e) {
                        break end;
                    }
                }
                _ = closed.wait_for(|c| *c) => break Err(ReceiveError::ChannelClosed),
                _ = tick.tick() => session.report_progress(),
            }
        };

        self.finish_receive(peer, &session.files, session.folder.as_deref()).await;
        end
    }

    /// The received notification, then the files that did not complete are deleted.
    async fn finish_receive(&self, peer: &PeerDevice, files: &Mutex<HashMap<String, InFile>>, folder: Option<&Path>) {
        let (completed, incomplete): (Vec<InFile>, Vec<InFile>) = {
            let f = files.lock_or_panic();
            f.values().cloned().partition(|f| f.status == FileShareFileStatus::Completed)
        };
        let completed: Vec<PathBuf> = completed.into_iter().filter_map(|f| f.path).collect();
        if !completed.is_empty() {
            self.notify_received(peer, &completed, folder);
        }

        let (mut failed_count, mut first_error) = (0usize, None);
        for path in incomplete.into_iter().filter_map(|f| f.path) {
            if let Err(e) = tokio::fs::remove_file(&path).await {
                failed_count += 1;
                first_error.get_or_insert(e);
            }
        }

        if let Some(e) = first_error {
            warn!(
                phone = %peer.client,
                failed_count,
                error = %e,
                "incomplete received files not deleted; they stay in the folder"
            );
        }
    }
}

/// The state of one receive session: the phone's file list and how far its files are.
struct ReceiveSession<'a> {
    inner: &'a Inner,
    phone: &'a DcgClient,
    channel: &'a Arc<TransferChannel>,
    trace: &'a TraceContext,
    folder: Option<PathBuf>,
    files: Arc<Mutex<HashMap<String, InFile>>>,
    /// Shared with the sink: why it refused a destination.
    refusal: Arc<Mutex<Option<DestinationError>>>,
    /// The manifest order, for the completion message.
    order: Vec<String>,
    events: mpsc::UnboundedSender<SinkEvent>,
    transfer_id: Option<String>,
    received: u32,
}

impl ReceiveSession<'_> {
    /// One control message from the phone; `Some` ends the session.
    fn on_control(&mut self, body: &[u8]) -> Option<Result<u32, ReceiveError>> {
        let phone = self.phone;
        let Ok(control) = Control::decode(body) else {
            return Some(Err(ReceiveError::UnparsedControl));
        };

        debug!(
            %phone,
            raw_scenario = control.scenario,
            file_count = control.content.len(),
            "control message from the phone"
        );
        self.on_scenario(&control)
    }

    /// The control message `control` by its scenario; `Some` ends the session.
    fn on_scenario(&mut self, control: &Control) -> Option<Result<u32, ReceiveError>> {
        let phone = self.phone;
        match control.scenario.as_str() {
            scenario::TO_PC if self.transfer_id.is_some() => {
                debug!(%phone, "a second file list; ignored");
                None
            }
            scenario::TO_PC => self.pull(control),
            scenario::TO_PC_CANCEL => Some(Err(ReceiveError::Cancelled)),
            other => Some(Err(ReceiveError::UnexpectedScenario(other.to_owned()))),
        }
    }

    /// The phone's file list: records it and requests its files; `Some` ends the session.
    fn pull(&mut self, control: &Control) -> Option<Result<u32, ReceiveError>> {
        let phone = self.phone;
        if control.content.iter().any(|c| !wire::valid_file_name(&c.file_name)) {
            return Some(Err(ReceiveError::BadFileName));
        }

        self.transfer_id = Some(control.transfer_id.clone());
        let entries: Vec<ProtoManifestEntry> = {
            let mut f = self.files.lock_or_panic();
            let order = &mut self.order;
            control
                .content
                .iter()
                .map(|c| {
                    let size = u64::try_from(c.file_size).unwrap_or(0);
                    order.push(c.file_id.clone());
                    f.insert(
                        c.file_id.clone(),
                        InFile {
                            name: c.file_name.clone(),
                            size,
                            status: FileShareFileStatus::NotStarted,
                            done: 0,
                            path: None,
                        },
                    );
                    ProtoManifestEntry { id: c.file_id.clone(), size, metadata: HashMap::new() }
                })
                .collect()
        };

        info!(%phone, file_count = entries.len(), "the phone's file list arrived; pulling its files");
        let names = control.content.iter().map(|c| c.file_name.clone()).collect();
        let total = entries.iter().map(|e| e.size).sum();
        self.inner.set_transfers(phone, |t| {
            t.receive = Some(ReceiveProgress { starting: false, file_names: names, bytes_done: 0, bytes_total: total })
        });

        if self.channel.is_closed() {
            return Some(Err(ReceiveError::ChannelLost));
        }

        let sink = Arc::new(Sink {
            folder: self.folder.clone(),
            files: self.files.clone(),
            events: self.events.clone(),
            refusal: self.refusal.clone(),
        });
        if let Err(e) = self.channel.streams().request_streams(entries, sink, self.trace) {
            return Some(Err(ReceiveError::RequestFailed(e)));
        }
        None
    }

    /// One report of the channel; `Some` ends the session.
    fn on_sink_event(&mut self, e: SinkEvent) -> Option<Result<u32, ReceiveError>> {
        match e {
            SinkEvent::Progress { id, done } => {
                self.record_progress(&id, done);
                None
            }
            SinkEvent::EntryDone { id, status: SingleStreamStatus::Success } => {
                self.record_stream_done(&id);
                None
            }
            SinkEvent::EntryDone { status, .. } => {
                Some(Err(ReceiveError::FileFailed { status, cause: self.refusal.lock_or_panic().take() }))
            }
            SinkEvent::TransferDone(TransferStatus::Success) => Some(Ok(self.complete())),
            SinkEvent::TransferDone(status) => Some(Err(ReceiveError::TransferFailed(status))),
        }
    }

    /// File `id` has `done` bytes.
    fn record_progress(&mut self, id: &str, done: u64) {
        let mut f = self.files.lock_or_panic();
        if let Some(file) = f.get_mut(id) {
            file.done = done;
            if done > 0 && file.status == FileShareFileStatus::NotStarted {
                file.status = FileShareFileStatus::InProgress;
            }
            if done == file.size && file.status == FileShareFileStatus::InProgress {
                file.status = FileShareFileStatus::Completed;
                self.received += 1;
            }
        }
    }

    /// The stream of file `id` succeeded.
    fn record_stream_done(&mut self, id: &str) {
        // An empty file reports no progress; it is complete when its stream is.
        let mut f = self.files.lock_or_panic();
        if let Some(file) = f.get_mut(id) {
            if file.size == 0 && file.status != FileShareFileStatus::Completed {
                file.status = FileShareFileStatus::Completed;
                self.received += 1;
            }
        }
    }

    /// The transfer succeeded: sends the completion message; the number of files received.
    fn complete(&mut self) -> u32 {
        let phone = self.phone;
        let content: Vec<FileEntry> = {
            let f = self.files.lock_or_panic();
            let transferred = self.order.len() as u32;
            self.received = self.received.max(transferred);
            self.order
                .iter()
                .filter_map(|id| {
                    f.get(id).map(|file| FileEntry {
                        file_id: id.clone(),
                        file_name: file.name.clone(),
                        file_size: file.size as i64,
                        status: file.status,
                    })
                })
                .collect()
        };

        let done = Control {
            transfer_id: self.transfer_id.clone().unwrap_or_default(),
            scenario: scenario::TO_PC_COMPLETED.to_owned(),
            sender_name: self.inner.sender_name(),
            received_file_count: self.received as i32,
            content,
            is_concurrent_mode: false,
        };

        if let Err(e) =
            self.channel.send_message(phonelink_features::shared::nano_channels::CONTROL_TARGET, &done.encode())
        {
            warn!(%phone, error = %e, "completion message not sent; the phone does not learn the files arrived");
        }
        self.received
    }

    /// The progress tick: publishes the bytes done when they changed.
    fn report_progress(&self) {
        let phone = self.phone;
        let (done, total) = {
            let f = self.files.lock_or_panic();
            (f.values().map(|x| x.done.min(x.size)).sum::<u64>(), f.values().map(|x| x.size).sum::<u64>())
        };

        let current = self.inner.state.lock_or_panic().transfers.get(&key(phone)).and_then(|t| t.receive.clone());
        if current.as_ref().is_some_and(|r| r.bytes_done != done) {
            self.inner.set_transfers(phone, |t| {
                if let Some(r) = t.receive.as_mut() {
                    r.bytes_done = done;
                    r.bytes_total = total;
                }
            });
        }
    }
}

/// The one line of a receive session's end.
fn log_receive_end(phone: &DcgClient, end: &Result<u32, ReceiveError>) {
    match end {
        Ok(received) => info!(%phone, received_count = *received, "receive ended"),
        Err(ReceiveError::Cancelled) => info!(%phone, "receive cancelled by the phone"),
        Err(e) => log_receive_failed(phone, e),
    }
}

/// A failed receive's line, with the scenario of an unexpected control message as it came.
fn log_receive_failed(phone: &DcgClient, e: &ReceiveError) {
    let raw_scenario = match e {
        ReceiveError::UnexpectedScenario(raw) => Some(raw.as_str()),
        _ => None,
    };
    warn!(%phone, raw_scenario, error = %e, "receive failed");
}
