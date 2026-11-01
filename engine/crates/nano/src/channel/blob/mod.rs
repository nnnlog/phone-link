//! The Blob channel: named byte streams — Start, chunks of at most 100000 bytes fragmented to the
//! packet size, Completion — with the receiver's per-chunk `BlobWritten` feedback pacing the sender.
//!
//! Every packet is sent reliably.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{Notify, mpsc, watch};
use tracing::{debug, warn};

use super::{Announcement, ChannelError, ChannelSender, PT_CHANNEL, RawChannel, RawInbound, Reader, frame, unframe};
use crate::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod kind {
    pub(super) const HANDSHAKE: u32 = 1;
    pub(super) const DATA: u32 = 2;
    pub(super) const COMPLETION: u32 = 3;
    pub(super) const START: u32 = 4;
    pub(super) const BLOB_WRITTEN: u32 = 5;
}

const MIN_VERSION: u32 = 2;
const MAX_VERSION: u32 = 3;
/// Bytes read from a source per chunk.
pub const MAX_CHUNK: usize = 100_000;
/// Bytes a Data packet adds besides the name.
const DATA_OVERHEAD: usize = 48;
/// Pacing: every `REFRESH`, while the transport's send queue is below `QUEUE_LIMIT`, release up to
/// `UTILIZATION × QUEUE_LIMIT × bandwidth / 8` bytes (at most `QUEUE_MAX`), holding a stream whose
/// peer-reported write queue is at `PEER_QUEUE_LIMIT` chunks or more.
const REFRESH: Duration = Duration::from_millis(200);
const QUEUE_LIMIT: f64 = 0.5;
const UTILIZATION: f64 = 0.5;
const QUEUE_MAX: f64 = 25_000_000.0;
const PEER_QUEUE_LIMIT: u32 = 10;
/// Chunks a receiver's queue holds before the channel waits for the consumer.
const RECEIVE_QUEUE: usize = 64;

/// Completion statuses.
pub mod status {
    pub const COMPLETED: u32 = 1;
    /// The receiver failed: starting or writing its stream for this name failed.
    pub const RECEIVER_ERROR: u32 = 2;
    pub const CANCELLED: u32 = 3;
    /// The receiver has closed its stream for this name after the sender's Completion 1: its
    /// acknowledgement of the stream's end.
    pub const CLOSED_BY_RECEIVER: u32 = 4;
    /// The receiver has no stream for this name.
    pub const NO_STREAM: u32 = 5;
    /// The receiver failed to close its stream for this name after the sender's Completion 1.
    pub const CLOSE_FAILED: u32 = 6;
    /// Not on the wire: what a registered receiver gets when the channel closes under its stream.
    pub const CHANNEL_CLOSED: u32 = u32::MAX;
}

/// How this side ends a stream it is receiving: the Completion it sends the stream's sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveEnd {
    /// There is nothing to write the stream to (status 5).
    NoStream,
    /// Starting or writing the stream failed (status 2).
    Failed,
    /// The stream the sender completed is closed (status 4).
    Closed,
    /// Closing the stream the sender completed failed (status 6).
    CloseFailed,
}

impl ReceiveEnd {
    pub fn status(self) -> u32 {
        match self {
            Self::NoStream => status::NO_STREAM,
            Self::Failed => status::RECEIVER_ERROR,
            Self::Closed => status::CLOSED_BY_RECEIVER,
            Self::CloseFailed => status::CLOSE_FAILED,
        }
    }
}

/// What a registered receiver gets for its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobEvent {
    Start {
        size: u64,
    },
    Chunk(Vec<u8>),
    /// The sender's Completion status.
    Completed(u32),
}

/// Why sending a stream failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlobSendError {
    #[error("id already used")]
    NameInUse,
    #[error("the receiver ended the stream with status {0}")]
    Peer(u32),
    #[error("reading the source failed: {0}")]
    Source(String),
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

struct Incoming {
    receiver: mpsc::Sender<BlobEvent>,
    next_chunk: u64,
    chunks: HashMap<u64, (Vec<u8>, Vec<bool>, usize)>,
}

#[derive(Default)]
struct State {
    version: u32,
    registered: HashMap<Vec<u8>, mpsc::Sender<BlobEvent>>,
    incoming: HashMap<Vec<u8>, Incoming>,
    sending: HashSet<Vec<u8>>,
    /// A Completion the receiver sent for one of our streams.
    peer_ended: HashMap<Vec<u8>, u32>,
    /// Peer write queues: per name (version 3) or under the empty name.
    peer_queue: HashMap<Vec<u8>, u32>,
    budget: f64,
    budget_at: Option<tokio::time::Instant>,
}

/// An open Blob channel.
pub struct BlobChannel {
    pub announcement: Announcement,
    sender: ChannelSender,
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    closed: watch::Receiver<bool>,
}

fn named(kind: u32, head: &[u8], name: &[u8]) -> Vec<u8> {
    let mut b = head.to_vec();
    b.extend_from_slice(&(name.len() as u32).to_le_bytes());
    b.extend_from_slice(name);
    frame(kind, &b)
}

fn completion(status: u32, name: &[u8]) -> Vec<u8> {
    named(kind::COMPLETION, &status.to_le_bytes(), name)
}

/// The version for a peer range that reaches into 2–3. A peer whose maximum is above 3 gets its
/// minimum (at least 2), not 3.
fn negotiate(min: u32, max: u32) -> u32 {
    if max == MAX_VERSION { MAX_VERSION } else { min.max(MIN_VERSION) }
}

impl BlobChannel {
    /// Opens the channel and negotiates the version: a peer range that does not reach into 2–3
    /// fails the channel; otherwise 3 if the peer's maximum is exactly 3, else the larger of 2 and
    /// the peer's minimum.
    pub async fn open(mut raw: RawChannel) -> Result<Self, ChannelError> {
        let early = raw.open().await?;
        let (sender, announcement, mut inbound) = raw.split();

        let mut hs = MIN_VERSION.to_le_bytes().to_vec();
        hs.extend_from_slice(&MAX_VERSION.to_le_bytes());
        sender.send(SendMode::Reliable, PT_CHANNEL, frame(kind::HANDSHAKE, &hs))?;

        let mut queued = early.into_iter();
        let mut backlog = Vec::new();
        let version = loop {
            let item = match queued.next() {
                Some(i) => i,
                None => inbound.recv().await.unwrap_or(RawInbound::Closed),
            };

            match item {
                RawInbound::Data { payload, .. } => {
                    let (k, body) = unframe(&payload)?;
                    if k != kind::HANDSHAKE {
                        backlog.push(payload);
                        continue;
                    }

                    let mut r = Reader::new(body);
                    let (min, max) = (r.u32()?, r.u32()?);

                    if min > MAX_VERSION || max < MIN_VERSION {
                        sender.close();
                        return Err(ChannelError::Handshake(format!("blob versions {min}–{max}")));
                    }
                    break negotiate(min, max);
                }
                RawInbound::Opened => {}
                RawInbound::Closed => return Err(ChannelError::Closed),
            }
        };

        backlog.extend(queued.filter_map(|i| match i {
            RawInbound::Data { payload, .. } => Some(payload),
            _ => None,
        }));

        let state = Arc::new(Mutex::new(State { version, ..Default::default() }));
        let changed = Arc::new(Notify::new());
        let (closed_tx, closed) = watch::channel(false);
        let reader = BlobInbound {
            sender: sender.clone(),
            state: state.clone(),
            changed: changed.clone(),
            anomaly_noted: AtomicBool::new(false),
        };
        tokio::spawn(reader.run(backlog, inbound, closed_tx));
        Ok(Self { announcement, sender, state, changed, closed })
    }

    pub fn version(&self) -> u32 {
        self.state.lock_or_panic().version
    }

    /// Accepts a stream named `name`; replaces (and cancels) an earlier registration of it.
    pub fn register(&self, name: impl Into<Vec<u8>>) -> mpsc::Receiver<BlobEvent> {
        let name = name.into();
        let (tx, rx) = mpsc::channel(RECEIVE_QUEUE);

        let mut s = self.state.lock_or_panic();
        if let Some(old) = s.registered.insert(name.clone(), tx) {
            let _ = old.try_send(BlobEvent::Completed(status::CANCELLED));
        }
        s.incoming.remove(&name);
        rx
    }

    pub fn unregister(&self, name: &[u8]) {
        let mut s = self.state.lock_or_panic();
        s.registered.remove(name);
        s.incoming.remove(name);
    }

    /// Cancels `name` in both directions (Completion status 3).
    pub fn cancel(&self, name: &[u8]) -> Result<(), ChannelError> {
        {
            let mut s = self.state.lock_or_panic();
            s.incoming.remove(name);
            s.peer_ended.insert(name.to_vec(), status::CANCELLED);
        }

        self.changed.notify_waiters();
        self.sender.send(SendMode::Reliable, PT_CHANNEL, completion(status::CANCELLED, name))
    }

    /// Ends the stream `name` this side is receiving, telling its sender how: no more of it is
    /// delivered.
    pub fn end_receive(&self, name: &[u8], end: ReceiveEnd) -> Result<(), ChannelError> {
        self.state.lock_or_panic().incoming.remove(name);
        self.sender.send(SendMode::Reliable, PT_CHANNEL, completion(end.status(), name))
    }

    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    pub fn close(&self) {
        self.sender.close();
    }

    fn peer_queue(&self, name: &[u8]) -> u32 {
        let s = self.state.lock_or_panic();
        let key: &[u8] = if s.version >= 3 { name } else { &[] };
        s.peer_queue.get(key).copied().unwrap_or(0)
    }

    /// Waits until the pacing releases `bytes` for `name`.
    async fn pace(&self, name: &[u8], bytes: usize) -> Result<(), BlobSendError> {
        loop {
            if *self.closed.borrow() {
                return Err(ChannelError::Closed.into());
            }
            if let Some(status) = self.state.lock_or_panic().peer_ended.get(name) {
                return Err(BlobSendError::Peer(*status));
            }

            let held = self.peer_queue(name) >= PEER_QUEUE_LIMIT;
            if !held {
                let now = tokio::time::Instant::now();
                let fill = self.sender.send_queue_fill();
                let release = (UTILIZATION * QUEUE_LIMIT * self.sender.bandwidth_bps() / 8.0).min(QUEUE_MAX);

                let mut s = self.state.lock_or_panic();
                if s.budget_at.is_none_or(|t| now >= t + REFRESH) {
                    s.budget_at = Some(now);
                    s.budget = if fill < QUEUE_LIMIT { release } else { 0.0 };
                }

                if s.budget > 0.0 {
                    s.budget -= bytes as f64;
                    return Ok(());
                }
            }

            let notified = self.changed.notified();
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(REFRESH) => {}
            }
        }
    }

    /// Sends one stream: Start, chunks of at most 100000 bytes read from `source`, Completion 1.
    pub async fn send_stream(
        &self,
        name: &[u8],
        size: u64,
        mut source: impl AsyncRead + Unpin,
    ) -> Result<(), BlobSendError> {
        {
            let mut s = self.state.lock_or_panic();
            if !s.sending.insert(name.to_vec()) {
                return Err(BlobSendError::NameInUse);
            }
            s.peer_ended.remove(name);
            s.peer_queue.remove(name);
        }

        let result = self.send_chunks(name, size, &mut source).await;
        self.state.lock_or_panic().sending.remove(name);
        result
    }

    async fn send_chunks(
        &self,
        name: &[u8],
        size: u64,
        source: &mut (impl AsyncRead + Unpin),
    ) -> Result<(), BlobSendError> {
        self.sender.send(SendMode::Reliable, PT_CHANNEL, named(kind::START, &size.to_le_bytes(), name))?;

        let mut chunk_index = 0u64;
        let mut chunk = vec![0u8; MAX_CHUNK];
        loop {
            let mut filled = 0;
            while filled < MAX_CHUNK {
                let n = source.read(&mut chunk[filled..]).await.map_err(|e| BlobSendError::Source(e.to_string()))?;
                if n == 0 {
                    break;
                }
                filled += n;
            }

            if filled == 0 {
                break;
            }

            self.pace(name, filled).await?;

            let capacity = self.sender.max_payload().saturating_sub(name.len() + DATA_OVERHEAD).max(1);
            // The receiver takes a chunk as complete once it has this many fragments.
            let fragment_count = filled.div_ceil(capacity) as u32;

            let mut offset = 0;
            while offset < filled {
                let end = (offset + capacity).min(filled);
                let mut b = Vec::with_capacity(48 + name.len() + end - offset);
                b.extend_from_slice(&0u32.to_le_bytes());
                b.extend_from_slice(&fragment_count.to_le_bytes());
                b.extend_from_slice(&chunk_index.to_le_bytes());
                b.extend_from_slice(&size.to_le_bytes());
                b.extend_from_slice(&(name.len() as u32).to_le_bytes());
                b.extend_from_slice(name);
                b.extend_from_slice(&(filled as u32).to_le_bytes());
                b.extend_from_slice(&(offset as u32).to_le_bytes());
                b.extend_from_slice(&((end - offset) as u32).to_le_bytes());
                b.extend_from_slice(&chunk[offset..end]);

                self.sender.send(SendMode::Reliable, PT_CHANNEL, frame(kind::DATA, &b))?;
                offset = end;
            }

            chunk_index += 1;
            if filled < MAX_CHUNK {
                break;
            }
        }

        if let Some(status) = self.state.lock_or_panic().peer_ended.get(name) {
            return Err(BlobSendError::Peer(*status));
        }

        self.sender.send(SendMode::Reliable, PT_CHANNEL, completion(status::COMPLETED, name))?;
        Ok(())
    }
}

/// The channel's inbound side.
struct BlobInbound {
    sender: ChannelSender,
    state: Arc<Mutex<State>>,
    changed: Arc<Notify>,
    /// A malformed or unknown inbound packet was logged at `warn`; later ones, which come with
    /// every chunk while the condition lasts, are logged at `debug`.
    anomaly_noted: AtomicBool,
}

impl BlobInbound {
    async fn run(
        self,
        backlog: Vec<Vec<u8>>,
        mut inbound: mpsc::UnboundedReceiver<RawInbound>,
        closed: watch::Sender<bool>,
    ) {
        for payload in backlog {
            self.on_packet(&payload).await;
        }

        while let Some(item) = inbound.recv().await {
            match item {
                RawInbound::Data { payload, .. } => self.on_packet(&payload).await,
                RawInbound::Opened => {}
                RawInbound::Closed => break,
            }
        }

        debug!("Nano blob channel closed");
        let receivers: Vec<_> = self.state.lock_or_panic().incoming.drain().map(|(_, i)| i.receiver).collect();
        for r in receivers {
            let _ = r.try_send(BlobEvent::Completed(status::CHANNEL_CLOSED));
        }

        let _ = closed.send(true);
        self.changed.notify_waiters();
    }

    fn reply(&self, packet: Vec<u8>) {
        if let Err(e) = self.sender.send(SendMode::Reliable, PT_CHANNEL, packet) {
            super::log_not_sent("blob", &e);
        }
    }

    /// `true` the first time an inbound anomaly is seen on the channel.
    fn first_anomaly(&self) -> bool {
        !self.anomaly_noted.swap(true, Ordering::Relaxed)
    }

    fn log_unknown_kind(&self, kind: u32) {
        if self.first_anomaly() {
            warn!(kind, "blob-channel packet of an unknown kind dropped");
        } else {
            debug!(kind, "blob-channel packet of an unknown kind dropped");
        }
    }

    async fn on_packet(&self, payload: &[u8]) {
        if let Err(e) = self.handle(payload).await {
            if self.first_anomaly() {
                warn!(error = %e, "malformed blob-channel packet dropped");
            } else {
                debug!(error = %e, "malformed blob-channel packet dropped");
            }
        }
    }

    async fn handle(&self, payload: &[u8]) -> Result<(), ChannelError> {
        let (k, body) = unframe(payload)?;
        let mut r = Reader::new(body);
        match k {
            kind::START => self.on_start(&mut r).await?,
            kind::DATA => {
                let _flags = r.u32()?;
                let _fragment_count = r.u32()?;
                let chunk_index = r.u64()?;
                let _stream_size = r.u64()?;
                let len = r.u32()? as usize;
                let name = r.take(len)?.to_vec();
                let chunk_length = r.u32()? as usize;
                let fragment_offset = r.u32()? as usize;
                let data_length = r.u32()? as usize;
                let data = r.take(data_length)?;

                self.on_data(name, chunk_index, chunk_length, fragment_offset, data).await?;
            }
            kind::COMPLETION => self.on_completion(&mut r).await?,
            kind::BLOB_WRITTEN => self.on_blob_written(&mut r)?,
            kind::HANDSHAKE => debug!("repeated blob handshake; ignored"),
            other => self.log_unknown_kind(other),
        }

        Ok(())
    }

    /// `Start`: opens the incoming stream of a registered name, else answers `NoStream`.
    async fn on_start(&self, r: &mut Reader<'_>) -> Result<(), ChannelError> {
        let size = r.u64()?;
        let len = r.u32()? as usize;
        let name = r.take(len)?.to_vec();

        let receiver = self.state.lock_or_panic().registered.get(&name).cloned();
        match receiver {
            Some(receiver) => {
                if receiver.send(BlobEvent::Start { size }).await.is_err() {
                    // The receiver is gone: no stream for the name.
                    self.reply(completion(status::NO_STREAM, &name));
                    return Ok(());
                }

                self.state
                    .lock_or_panic()
                    .incoming
                    .insert(name, Incoming { receiver, next_chunk: 0, chunks: HashMap::new() });
            }
            None => {
                debug!(name = %String::from_utf8_lossy(&name), "blob Start for an unregistered name");
                self.reply(completion(status::NO_STREAM, &name));
            }
        }

        Ok(())
    }

    /// `Completion` from the peer: ends the incoming stream of the name and records a failed end of
    /// a stream being sent.
    async fn on_completion(&self, r: &mut Reader<'_>) -> Result<(), ChannelError> {
        let status = r.u32()?;
        let len = r.u32()? as usize;
        let name = r.take(len)?.to_vec();
        debug!(name = %String::from_utf8_lossy(&name), status, "blob completion from the peer");

        let incoming = self.state.lock_or_panic().incoming.remove(&name);
        if let Some(incoming) = incoming {
            let _ = incoming.receiver.send(BlobEvent::Completed(status)).await;
        }

        let mut s = self.state.lock_or_panic();
        if s.sending.contains(&name) && status != status::COMPLETED {
            s.peer_ended.insert(name, status);
        }
        drop(s);
        self.changed.notify_waiters();
        Ok(())
    }

    /// `BlobWritten`: the peer's queue for the name (one queue before version 3).
    fn on_blob_written(&self, r: &mut Reader<'_>) -> Result<(), ChannelError> {
        let queue = r.u32()?;
        let len = r.u32()? as usize;
        let name = r.take(len)?.to_vec();
        debug!(name = %String::from_utf8_lossy(&name), queue, "blob written by the peer");

        let mut s = self.state.lock_or_panic();
        let key = if s.version >= 3 { name } else { Vec::new() };
        s.peer_queue.insert(key, queue);
        drop(s);
        self.changed.notify_waiters();
        Ok(())
    }

    async fn on_data(
        &self,
        name: Vec<u8>,
        chunk_index: u64,
        chunk_length: usize,
        offset: usize,
        data: &[u8],
    ) -> Result<(), ChannelError> {
        let mut ready = Vec::new();
        let receiver = {
            let mut s = self.state.lock_or_panic();
            let Some(incoming) = s.incoming.get_mut(&name) else {
                debug!(name = %String::from_utf8_lossy(&name), "blob data for no open stream; dropped");
                return Ok(());
            };

            if chunk_index < incoming.next_chunk {
                return Ok(());
            }
            if offset + data.len() > chunk_length {
                return Err(ChannelError::Invalid("blob fragment outside its chunk".into()));
            }

            let entry = incoming
                .chunks
                .entry(chunk_index)
                .or_insert_with(|| (vec![0; chunk_length], vec![false; chunk_length], 0));
            if entry.0.len() != chunk_length {
                return Err(ChannelError::Invalid("blob fragment disagrees with its chunk length".into()));
            }

            for i in offset..offset + data.len() {
                if !entry.1[i] {
                    entry.1[i] = true;
                    entry.2 += 1;
                }
            }
            entry.0[offset..offset + data.len()].copy_from_slice(data);

            while let Some((_, _, filled)) = incoming.chunks.get(&incoming.next_chunk) {
                let len = incoming.chunks[&incoming.next_chunk].0.len();
                if *filled < len {
                    break;
                }
                let (bytes, _, _) = incoming.chunks.remove(&incoming.next_chunk).expect("present");
                ready.push(bytes);
                incoming.next_chunk += 1;
            }

            incoming.receiver.clone()
        };

        for chunk in ready {
            if receiver.send(BlobEvent::Chunk(chunk)).await.is_err() {
                self.state.lock_or_panic().incoming.remove(&name);
                // The receiver is gone mid-stream: the stream is cancelled.
                self.reply(completion(status::CANCELLED, &name));
                return Ok(());
            }

            let queued = (receiver.max_capacity() - receiver.capacity()) as u32;
            self.reply(named(kind::BLOB_WRITTEN, &queued.to_le_bytes(), &name));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests;
