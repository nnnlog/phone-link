//! The `NamedStreams` platform channel.
//!
//! It covers feature messages on `/<scenario>/NamedStreams<sub>`, and named byte streams — a
//! manifest exchanged on the Message child, each entry's bytes as one blob named by its id on the
//! Blob child.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message as _;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

use super::channel::blob::{BlobChannel, BlobEvent, BlobSendError, ReceiveEnd, status};
use super::channel::message::InboundMessage;
use super::channel::{ChannelError, log_not_sent};
use super::device::{PlatformChannel, PlatformChannelType};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::TraceContext;
use phonelink_proto::platform_sdk::v1::{
    ProtoManifest, ProtoManifestAck, ProtoManifestEntry, ProtoManifestStatus, ProtoTraceContext,
};
use phonelink_transport::platform::context_message::proto_trace;

/// Where the PC's streams come from.
#[async_trait]
pub trait StreamSource: Send + Sync {
    /// The bytes of `entry`; `None` when the feature cannot supply them.
    async fn open(&self, entry: &ProtoManifestEntry) -> Option<Box<dyn AsyncRead + Send + Unpin>>;
}

/// Where the phone's streams go.
#[async_trait]
pub trait StreamReceiver: Send + Sync {
    /// A writable destination for stream `id` of `size` bytes; `None` refuses it.
    async fn destination(&self, id: &str, size: u64) -> Option<Box<dyn AsyncWrite + Send + Unpin>>;
    /// Progress of one entry.
    fn progress(&self, _id: &str, _done: u64, _total: u64) {}
    /// One entry ended.
    fn entry_done(&self, _id: &str, _status: SingleStreamStatus) {}
    /// Every entry of a manifest ended.
    fn transfer_done(&self, _status: TransferStatus) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleStreamStatus {
    Success,
    Error,
    /// The feature could not supply a stream.
    ErrorInvalidStream,
    ErrorClosedStreamBeforeCompletion,
    /// The blob send failed or the channel closed.
    TransportError,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendStreamsStatus {
    Success,
    Failure,
    Cancelled,
    PartialFailure,
    ReceiverNotReady,
    NanoChannelFailure,
    StreamFailure,
    NoEntriesSent,
    ChannelInitializationTimeout,
    MalformedManifestAck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    Success,
    Error,
    Cancelled,
    StreamNotReady,
    ErrorClosedStream,
    TransportError,
}

/// An open `NamedStreams` channel.
pub struct NamedStreams {
    channel: PlatformChannel,
    receiver: Mutex<Option<Arc<dyn StreamReceiver>>>,
    sender: Mutex<Option<Arc<dyn StreamSource>>>,
    timeout: Duration,
    closed: watch::Receiver<bool>,
}

impl std::fmt::Debug for NamedStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedStreams").field("scenario", &self.channel.scenario_id).finish()
    }
}

/// Logs a received entry the PC could not write: at `warn` the first time in its transfer, at
/// `debug` after, since one cause (a full disk) fails every entry of a transfer.
/// `phase` is what failed: `write` or `close`.
fn log_receive_failure(noted: &AtomicBool, error: &std::io::Error, phase: &'static str) {
    if noted.swap(true, Ordering::Relaxed) {
        debug!(%error, phase, "received named stream not written; it ends failed");
    } else {
        warn!(%error, phase, "received named stream not written; it ends failed");
    }
}

/// A manifest the PC sends: its trace context is a child of the operation's, trace state included.
fn manifest_of(entries: &[ProtoManifestEntry], trace: &TraceContext) -> ProtoManifest {
    ProtoManifest {
        entries: entries.iter().map(|e| (e.id.clone(), e.clone())).collect(),
        trace_context: Some(proto_trace(&trace.child())),
    }
}

/// The trace context of an ack to a parsed manifest that brought none: a fresh one.
fn fresh_trace() -> ProtoTraceContext {
    proto_trace(&TraceContext::new_root(Vec::new()))
}

impl NamedStreams {
    /// Wraps an open `NamedStreams` platform channel and starts answering the phone's manifests.
    pub fn new(channel: PlatformChannel, timeout: Duration) -> Result<Arc<Self>, ChannelError> {
        if channel.kind != PlatformChannelType::NamedStreams || channel.blob.is_none() {
            return Err(ChannelError::Invalid("not a NamedStreams channel".into()));
        }

        // Either child closing closes the channel.
        let (closed_tx, closed) = watch::channel(false);
        let mut message_closed = channel.message.closed();
        let mut blob_closed = channel.blob.as_ref().expect("checked above").closed();
        tokio::spawn(async move {
            tokio::select! {
                _ = message_closed.wait_for(|c| *c) => {}
                _ = blob_closed.wait_for(|c| *c) => {}
            }
            let _ = closed_tx.send(true);
        });

        let me = Arc::new(Self { channel, receiver: Mutex::default(), sender: Mutex::default(), timeout, closed });
        let manifests = me.channel.message.register(me.scope("Manifest"));
        let requests = me.channel.message.register(me.scope("ManifestRequest"));
        tokio::spawn(Self::serve_manifests(Arc::downgrade(&me), manifests));
        tokio::spawn(Self::serve_requests(Arc::downgrade(&me), requests));
        Ok(me)
    }

    fn scope(&self, name: &str) -> String {
        format!("/{}/{name}", self.channel.scenario_id)
    }

    fn blob(&self) -> &BlobChannel {
        self.channel.blob.as_ref().expect("checked in new")
    }

    pub fn platform_channel(&self) -> &PlatformChannel {
        &self.channel
    }

    /// The receiving provider for the phone's streams.
    pub fn set_receiver(&self, receiver: Arc<dyn StreamReceiver>) {
        *self.receiver.lock_or_panic() = Some(receiver);
    }

    /// The sending provider the phone's manifest requests are served from.
    pub fn set_sender(&self, sender: Arc<dyn StreamSource>) {
        *self.sender.lock_or_panic() = Some(sender);
    }

    /// Sends feature bytes on `/<scenario>/NamedStreams<sub>`.
    pub fn send_message(&self, sub: &str, body: &[u8]) -> Result<(), ChannelError> {
        self.channel.send(sub, body)
    }

    /// Feature bytes on `/<scenario>/NamedStreams<sub>`.
    pub fn messages(&self, sub: &str) -> mpsc::UnboundedReceiver<InboundMessage> {
        self.channel.register(sub)
    }

    pub fn close(&self) {
        self.channel.close();
    }

    /// Becomes `true` once the Message or the Blob child closed.
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    async fn serve_manifests(me: std::sync::Weak<Self>, mut inbound: mpsc::UnboundedReceiver<InboundMessage>) {
        while let Some(m) = inbound.recv().await {
            let Some(me) = me.upgrade() else {
                return;
            };

            // An ack carries the manifest's trace context, or a fresh one when it brought none; the
            // ack to a manifest that does not parse carries none. No receiving provider outranks a
            // parse failure.
            let parsed = ProtoManifest::decode(m.body.as_slice());
            if let Err(e) = &parsed {
                warn!(error = %e, "named-streams manifest does not parse; its acknowledgement says so");
            }
            let trace = match &parsed {
                Ok(manifest) => Some(manifest.trace_context.clone().unwrap_or_else(fresh_trace)),
                Err(_) => None,
            };

            let receiver = me.receiver.lock_or_panic().clone();
            let status = match (receiver, parsed) {
                (None, _) => ProtoManifestStatus::StreamUnreadyError,
                (Some(_), Err(_)) => ProtoManifestStatus::ParseError,
                (Some(receiver), Ok(manifest)) => {
                    me.accept(manifest.entries.into_values().collect(), receiver);
                    ProtoManifestStatus::Success
                }
            };

            let ack = ProtoManifestAck { status: status as i32, trace_context: trace };
            if let Err(e) = me.channel.message.send(&me.scope("ManifestAck"), &ack.encode_to_vec()) {
                log_not_sent("message", &e);
            }
        }
    }

    async fn serve_requests(me: std::sync::Weak<Self>, mut inbound: mpsc::UnboundedReceiver<InboundMessage>) {
        while let Some(m) = inbound.recv().await {
            let Some(me) = me.upgrade() else {
                return;
            };

            let Some(sender) = me.sender.lock_or_panic().clone() else {
                debug!("named-streams manifest request without a sending provider ignored");
                continue;
            };

            let manifest = match ProtoManifest::decode(m.body.as_slice()) {
                Ok(manifest) => manifest,
                Err(e) => {
                    warn!(error = %e, "named-streams manifest request that does not parse dropped");
                    continue;
                }
            };

            let entries: Vec<ProtoManifestEntry> = manifest.entries.into_values().collect();
            tokio::spawn(async move {
                let results = me.send_entries(&entries, sender.as_ref()).await;
                let sent_count = results.iter().filter(|(_, s)| *s == SingleStreamStatus::Success).count();
                info!(entry_count = results.len(), sent_count, "requested named streams sent");
            });
        }
    }

    /// Registers `entries` as acceptable blob names and receives each into `receiver`. The receiver
    /// is told how each entry and the transfer ended, and logs the transfer; an entry's failure
    /// whose cause the status does not carry is logged here, the first of the transfer at `warn`.
    fn accept(self: &Arc<Self>, entries: Vec<ProtoManifestEntry>, receiver: Arc<dyn StreamReceiver>) {
        let total = entries.len();
        debug!(entry_count = total, "named-streams transfer started");
        let results = Arc::new(Mutex::new(HashMap::<String, SingleStreamStatus>::new()));
        let failure_noted = Arc::new(AtomicBool::new(false));

        for entry in entries {
            let events = self.blob().register(entry.id.clone().into_bytes());
            let (me, receiver, results, failure_noted) =
                (self.clone(), receiver.clone(), results.clone(), failure_noted.clone());

            tokio::spawn(async move {
                let status = me.receive_one(&entry, events, receiver.as_ref(), &failure_noted).await;
                receiver.entry_done(&entry.id, status);

                let mut r = results.lock_or_panic();
                r.insert(entry.id.clone(), status);

                if r.len() == total {
                    let transfer = if r.values().all(|s| *s == SingleStreamStatus::Success) {
                        TransferStatus::Success
                    } else if r.values().any(|s| *s == SingleStreamStatus::Cancelled) {
                        TransferStatus::Cancelled
                    } else if r.values().any(|s| *s == SingleStreamStatus::TransportError) {
                        TransferStatus::TransportError
                    } else {
                        TransferStatus::Error
                    };
                    debug!(transfer_status = ?transfer, entry_count = total, "named-streams transfer ended");
                    receiver.transfer_done(transfer);
                }
            });
        }
    }

    async fn receive_one(
        &self,
        entry: &ProtoManifestEntry,
        mut events: mpsc::Receiver<BlobEvent>,
        receiver: &dyn StreamReceiver,
        failure_noted: &AtomicBool,
    ) -> SingleStreamStatus {
        let mut destination: Option<Box<dyn AsyncWrite + Send + Unpin>> = None;
        let mut done = 0u64;
        let mut total = entry.size;

        while let Some(event) = events.recv().await {
            match event {
                BlobEvent::Start { size } => {
                    total = size;
                    destination = receiver.destination(&entry.id, size).await;
                    if destination.is_none() {
                        self.end(entry, ReceiveEnd::NoStream);
                        return SingleStreamStatus::ErrorInvalidStream;
                    }
                }
                BlobEvent::Chunk(bytes) => {
                    let Some(d) = destination.as_mut() else {
                        return SingleStreamStatus::Error;
                    };

                    if let Err(e) = d.write_all(&bytes).await {
                        log_receive_failure(failure_noted, &e, "write");
                        self.end(entry, ReceiveEnd::Failed);
                        return SingleStreamStatus::Error;
                    }

                    done += bytes.len() as u64;
                    receiver.progress(&entry.id, done, total);
                }
                BlobEvent::Completed(s) => {
                    let Some(mut d) = destination else {
                        return SingleStreamStatus::ErrorClosedStreamBeforeCompletion;
                    };

                    return match s {
                        status::COMPLETED => match d.shutdown().await {
                            Ok(()) => {
                                self.end(entry, ReceiveEnd::Closed);
                                SingleStreamStatus::Success
                            }
                            Err(e) => {
                                log_receive_failure(failure_noted, &e, "close");
                                self.end(entry, ReceiveEnd::CloseFailed);
                                SingleStreamStatus::Error
                            }
                        },
                        status::CANCELLED => SingleStreamStatus::Cancelled,
                        status::CHANNEL_CLOSED => SingleStreamStatus::TransportError,
                        _ => SingleStreamStatus::Error,
                    };
                }
            }
        }
        SingleStreamStatus::TransportError
    }

    /// Tells the sender of `entry` how this side ended it.
    fn end(&self, entry: &ProtoManifestEntry, end: ReceiveEnd) {
        if let Err(e) = self.blob().end_receive(entry.id.as_bytes(), end) {
            log_not_sent("blob", &e);
        }
    }

    async fn send_entries(
        &self,
        entries: &[ProtoManifestEntry],
        source: &dyn StreamSource,
    ) -> Vec<(String, SingleStreamStatus)> {
        let mut out = Vec::new();
        // An entry's failure cause is not in its status: the first of the batch is logged at `warn`.
        let mut failure_noted = false;

        for entry in entries {
            let status = match source.open(entry).await {
                None => SingleStreamStatus::ErrorInvalidStream,
                Some(reader) => match self.blob().send_stream(entry.id.as_bytes(), entry.size, reader).await {
                    Ok(()) => SingleStreamStatus::Success,
                    Err(BlobSendError::Peer(status::CANCELLED)) => SingleStreamStatus::Cancelled,
                    Err(e) => {
                        let gone = matches!(&e, BlobSendError::Channel(c) if c.is_gone());
                        if gone || std::mem::replace(&mut failure_noted, true) {
                            debug!(error = %e, "named stream not sent");
                        } else {
                            warn!(error = %e, "named stream not sent");
                        }

                        match e {
                            BlobSendError::Peer(_) | BlobSendError::Channel(_) => SingleStreamStatus::TransportError,
                            BlobSendError::NameInUse | BlobSendError::Source(_) => SingleStreamStatus::Error,
                        }
                    }
                },
            };
            out.push((entry.id.clone(), status));
        }
        out
    }

    /// Pushes `entries` (sent in this order): the manifest, its acknowledgement, then each stream.
    pub async fn send_streams(
        &self,
        entries: Vec<ProtoManifestEntry>,
        source: &dyn StreamSource,
        trace: &TraceContext,
    ) -> (SendStreamsStatus, Vec<(String, SingleStreamStatus)>) {
        let mut acks = self.channel.message.register(self.scope("ManifestAck"));
        let manifest = manifest_of(&entries, trace);
        if self.channel.message.send(&self.scope("Manifest"), &manifest.encode_to_vec()).is_err() {
            return (SendStreamsStatus::NanoChannelFailure, Vec::new());
        }

        let ack = tokio::time::timeout(self.timeout, acks.recv()).await;
        self.channel.message.unregister(&self.scope("ManifestAck"));
        let ack = match ack {
            Err(_) => return (SendStreamsStatus::ChannelInitializationTimeout, Vec::new()),
            Ok(None) => return (SendStreamsStatus::NanoChannelFailure, Vec::new()),
            Ok(Some(m)) => match ProtoManifestAck::decode(m.body.as_slice()) {
                Ok(a) => a,
                Err(_) => return (SendStreamsStatus::MalformedManifestAck, Vec::new()),
            },
        };

        if ack.status != ProtoManifestStatus::Success as i32 {
            return (SendStreamsStatus::Failure, Vec::new());
        }

        let results = self.send_entries(&entries, source).await;
        let ok = results.iter().filter(|(_, s)| *s == SingleStreamStatus::Success).count();
        let status = if results.is_empty() {
            SendStreamsStatus::NoEntriesSent
        } else if ok == results.len() {
            SendStreamsStatus::Success
        } else if results.iter().any(|(_, s)| *s == SingleStreamStatus::Cancelled) {
            SendStreamsStatus::Cancelled
        } else if ok == 0 {
            SendStreamsStatus::StreamFailure
        } else {
            SendStreamsStatus::PartialFailure
        };
        (status, results)
    }

    /// Pulls `entries`: registers their ids with `receiver`, then sends the manifest on
    /// `ManifestRequest`; the phone pushes the blobs.
    pub fn request_streams(
        self: &Arc<Self>,
        entries: Vec<ProtoManifestEntry>,
        receiver: Arc<dyn StreamReceiver>,
        trace: &TraceContext,
    ) -> Result<(), ChannelError> {
        let manifest = manifest_of(&entries, trace);
        self.accept(entries, receiver);
        self.channel.message.send(&self.scope("ManifestRequest"), &manifest.encode_to_vec())
    }
}

#[cfg(test)]
mod tests;
