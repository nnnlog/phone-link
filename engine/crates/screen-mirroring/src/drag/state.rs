//! The drag state.

use super::{Phase, PhoneDragEvent, PhoneDragItem};
use crate::ScreenMirroringEvent;
use crate::input::InputPipe;
use crate::wire::ClipData;
use phonelink_core::sync::MutexExt;
use phonelink_features::shared::notifier::SystemNotifier;
use phonelink_nano::channel::ChannelError;
use phonelink_nano::channel::blob::{BlobChannel, BlobEvent, ReceiveEnd, status};
use phonelink_nano::channel::message::MessageChannel;
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

/// The Mac → phone drag the window shows now (from its enter to the next enter).
pub(super) struct ToPhone {
    pub(super) correlation_id: String,
    /// Bumped per enter: a delayed start belongs to one drag only.
    pub(super) generation: u64,
    /// `startPc` went out.
    pub(super) started: bool,
    /// Dropped before the delayed start: `dropPc` follows it.
    pub(super) drop_pending: bool,
    /// Left or dropped: nothing more is sent for its pointer.
    pub(super) ended: bool,
    /// The phone's `cancel` 9: the drag shows no effect from now on.
    pub(super) effect_none: bool,
}

/// One received file of a phone → Mac drag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ItemStatus {
    Receiving,
    Done(PathBuf),
    Failed,
}

pub(super) struct FromPhoneItem {
    pub(super) item: PhoneDragItem,
    pub(super) status: watch::Receiver<ItemStatus>,
    pub(super) written: bool,
}

/// A started phone → Mac drag: its files on their way into `dir`.
pub(super) struct FromPhone {
    pub(super) dir: PathBuf,
    pub(super) items: Vec<FromPhoneItem>,
    /// Ends its receivers and its idle timer.
    pub(super) stop: CancellationToken,
    /// Files and bytes received from the phone, of `total` bytes.
    pub(super) received_files: usize,
    pub(super) bytes: u64,
    pub(super) total: u64,
    /// The Mac drag was released (dropped, or asked for a file): its notifications show from now on.
    pub(super) released: bool,
    /// When the progress notification was last posted.
    pub(super) posted: Option<Instant>,
    /// Why the drag was given up, for its end's line.
    pub(super) failure: Option<String>,
}

pub(super) struct State {
    pub(super) channels: Option<(Arc<MessageChannel>, Arc<BlobChannel>)>,
    /// A drag activity exists (`startPc` sent or `startPhone` received, and no `complete`, `clipData`
    /// or `cancel` since): `dropPc` goes only during one.
    pub(super) activity: bool,
    pub(super) to_phone: Option<ToPhone>,
    pub(super) generation: u64,
    /// The Mac → phone files the phone may still request, by blob name.
    pub(super) outgoing: HashMap<String, PathBuf>,
    /// Blob names being sent now.
    pub(super) sending: HashSet<String>,
    pub(super) cool_off_until: Option<Instant>,
    pub(super) phase: Phase,
    /// `clipData` kept until the next pointer move starts the Mac drag.
    pub(super) pending: Option<Option<ClipData>>,
    pub(super) from_phone: Option<FromPhone>,
    /// Bumped per reset: a timer belongs to one phone drag only.
    pub(super) phone_epoch: u64,
    /// The Mac → phone drag (by `generation`) whose first file not sent was logged.
    pub(super) send_failure_logged: Option<u64>,
}

/// One window's drag and drop.
pub struct WindowDrag {
    pub(super) session_id: String,
    pub(super) input: Arc<Mutex<InputPipe>>,
    pub(super) events: broadcast::Sender<ScreenMirroringEvent>,
    pub(super) runtime: Handle,
    pub(super) notifier: Arc<dyn SystemNotifier>,
    pub(super) staging: PathBuf,
    pub(super) state: Mutex<State>,
    /// Posting the drag's notification has failed since it last succeeded: the failure was logged,
    /// and the next ones are not (the progress is posted every second).
    pub(super) notify_failing: AtomicBool,
}

impl WindowDrag {
    /// `staging` is a private folder the window's received files are written under.
    pub fn new(
        session_id: String,
        input: Arc<Mutex<InputPipe>>,
        events: broadcast::Sender<ScreenMirroringEvent>,
        runtime: Handle,
        notifier: Arc<dyn SystemNotifier>,
        staging: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            input,
            events,
            runtime,
            notifier,
            staging,
            state: Mutex::new(State {
                channels: None,
                activity: false,
                to_phone: None,
                generation: 0,
                outgoing: HashMap::new(),
                sending: HashSet::new(),
                cool_off_until: None,
                phase: Phase::Ready,
                pending: None,
                from_phone: None,
                phone_epoch: 0,
                send_failure_logged: None,
            }),
            notify_failing: AtomicBool::new(false),
        })
    }

    pub(super) fn send(&self, st: &State, target: &str, body: &[u8]) {
        match self.try_send(st, target, body) {
            Ok(()) => {}
            Err(Some(e)) => warn!(error = %e, route = target, "drag message not sent"),
            Err(None) => debug!(route = target, "drag message not sent; its channel is not open"),
        }
    }

    /// Sends `body` to `target` on the window's message channel: the error when it was not sent,
    /// `None` without a channel.
    pub(super) fn try_send(&self, st: &State, target: &str, body: &[u8]) -> Result<(), Option<ChannelError>> {
        let (m, _) = st.channels.as_ref().ok_or(None)?;
        m.send(target, body).map_err(Some)
    }

    /// The window's channel set opened (`Some`) or closed. Closing ends both directions without
    /// telling the phone.
    pub fn set_channels(self: &Arc<Self>, channels: Option<(Arc<MessageChannel>, Arc<BlobChannel>)>) {
        let mut st = self.state.lock_or_panic();
        if channels.is_none() {
            self.clear_outgoing(&mut st);
            st.to_phone = None;
            st.activity = false;
            if st.phase != Phase::Ready || st.pending.is_some() {
                self.reset(&mut st, PhoneDragEvent::PhoneCancel);
            }
        }
        st.channels = channels;
    }

    /// Forgets the Mac → phone files not yet requested and cancels the blobs being sent.
    pub(super) fn clear_outgoing(&self, st: &mut State) {
        st.outgoing.clear();
        let sending: Vec<String> = st.sending.drain().collect();
        if let Some((_, b)) = &st.channels {
            cancel_blobs(b, sending.iter().map(String::as_bytes));
        }
    }
}

/// The progress notification's title: file `received + 1` of `total`.
pub fn progress_title(total: usize, received: usize) -> String {
    if total == 1 {
        "Transferring 1 file".to_string()
    } else {
        format!("Transferring {} of {total} files", received + 1)
    }
}

/// The outcome notification's title and body after `received` of `total` files arrived.
pub fn completion_text(total: usize, received: usize) -> (String, String) {
    const TRY_AGAIN: &str = "Please try again";
    if total == 1 {
        return if received == 1 {
            ("Successfully transferred 1 file".to_string(), String::new())
        } else {
            ("We ran into an issue transferring this file".to_string(), TRY_AGAIN.to_string())
        };
    }
    if received >= total {
        (format!("Successfully transferred {total} files"), String::new())
    } else if received > 0 {
        (
            format!("Successfully transferred {received} of {total} files"),
            format!("Failed to transfer {} of {total} files", total - received),
        )
    } else {
        ("We ran into an issue transferring files".to_string(), TRY_AGAIN.to_string())
    }
}

/// `"{written}/{total} {unit}"`, both in the unit of `total` with two decimals.
pub fn size_text(written: u64, total: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let t = total as f64;
    let unit = (0..UNITS.len()).find(|&u| t < 1024f64.powi(u as i32 + 1)).unwrap_or(UNITS.len() - 1);
    let scale = 1024f64.powi(unit as i32);
    format!("{:.2}/{:.2} {}", written as f64 / scale, t / scale, UNITS[unit])
}

/// Cancels the blobs `names` on `blob`, with one line for the cancels not sent.
pub(super) fn cancel_blobs<'a>(blob: &BlobChannel, names: impl Iterator<Item = &'a [u8]>) {
    let mut failed_count = 0usize;
    let mut first = None;
    for name in names {
        if let Err(e) = blob.cancel(name) {
            failed_count += 1;
            first.get_or_insert(e);
        }
    }

    if let Some(e) = first {
        debug!(failed_count, error = %e, "blob cancels not sent");
    }
}

pub(super) fn remove_dir(dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!(error = %e, "drag staging folder not removed");
    }
}

/// Receives one file of a phone drag into `path`: done at the sender's Completion 1 (answered with 4,
/// or 6 when closing fails), failed at any other end.
#[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
pub(super) async fn receive(
    drag: Arc<WindowDrag>,
    blob: Arc<BlobChannel>,
    name: String,
    path: PathBuf,
    mut events: tokio::sync::mpsc::Receiver<BlobEvent>,
    status_tx: watch::Sender<ItemStatus>,
    last_data: Arc<Mutex<Instant>>,
    stop: CancellationToken,
) {
    let mut receiving = Receiving {
        drag: &drag,
        blob: &blob,
        name: &name,
        path: &path,
        status_tx: &status_tx,
        last_data: &last_data,
        file: None,
        dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
    };

    loop {
        let event = tokio::select! {
            e = events.recv() => e,
            _ = stop.cancelled() => return,
        };
        if receiving.on_event(event).await.is_break() {
            return;
        }
    }
}

/// One file of a phone drag being received, between its blob events.
struct Receiving<'a> {
    drag: &'a WindowDrag,
    blob: &'a BlobChannel,
    name: &'a str,
    path: &'a Path,
    status_tx: &'a watch::Sender<ItemStatus>,
    last_data: &'a Mutex<Instant>,
    file: Option<tokio::fs::File>,
    dir: PathBuf,
}

impl Receiving<'_> {
    /// Handles one blob event (`None` when the channel closed); `Break` when the file's receive is over.
    async fn on_event(&mut self, event: Option<BlobEvent>) -> ControlFlow<()> {
        match event {
            Some(BlobEvent::Start { .. }) => match tokio::fs::File::create(self.path).await {
                Ok(f) => self.file = Some(f),
                Err(e) => return self.fail(Some(ReceiveEnd::Failed), format!("a file not created: {e}")),
            },
            Some(BlobEvent::Chunk(bytes)) => return self.on_chunk(&bytes).await,
            Some(BlobEvent::Completed(status::COMPLETED)) => return self.on_completed().await,
            Some(BlobEvent::Completed(s)) => {
                return self.fail(None, format!("the phone ended a file with status {s}"));
            }
            None => return self.fail(None, "the blob channel closed".to_string()),
        }
        ControlFlow::Continue(())
    }

    /// Writes one chunk of the file.
    async fn on_chunk(&mut self, bytes: &[u8]) -> ControlFlow<()> {
        *self.last_data.lock_or_panic() = Instant::now();
        self.drag.received_bytes(&self.dir, bytes.len() as u64);

        let Some(f) = self.file.as_mut() else {
            return self.fail(Some(ReceiveEnd::Failed), "a file's data came before its start".to_string());
        };
        if let Err(e) = f.write_all(bytes).await {
            return self.fail(Some(ReceiveEnd::Failed), format!("a file not written: {e}"));
        }
        ControlFlow::Continue(())
    }

    /// Closes the file the sender completed and reports it received.
    async fn on_completed(&mut self) -> ControlFlow<()> {
        let closed = match self.file.take() {
            Some(mut f) => f.flush().await.and(f.sync_all().await),
            None => return self.fail(Some(ReceiveEnd::CloseFailed), "a file completed before its start".to_string()),
        };

        if let Err(e) = closed {
            return self.fail(Some(ReceiveEnd::CloseFailed), format!("a file not closed: {e}"));
        }

        if let Err(e) = self.blob.end_receive(self.name.as_bytes(), ReceiveEnd::Closed) {
            debug!(error = %e, "blob end not sent; the file is kept");
        }

        let _ = self.status_tx.send(ItemStatus::Done(self.path.to_path_buf()));
        self.drag.received_file(&self.dir);
        ControlFlow::Break(())
    }

    /// Ends the receive as failed because of `failure`, answering the sender with `end` when given.
    fn fail(&self, end: Option<ReceiveEnd>, failure: String) -> ControlFlow<()> {
        if let Some(end) = end
            && let Err(e) = self.blob.end_receive(self.name.as_bytes(), end)
        {
            debug!(error = %e, "blob end not sent");
        }

        if *self.status_tx.borrow() == ItemStatus::Receiving {
            let _ = self.status_tx.send(ItemStatus::Failed);
            self.drag.item_failed(&self.dir, failure);
        }
        ControlFlow::Break(())
    }
}
