//! The Message channel: a handshake, then target-addressed messages fragmented to the transport's
//! packet size and reassembled by message id. Every packet is sent reliably.
//!
//! A message is delivered to the registered scope that is the longest segment prefix of its target:
//! the target is cut at its first `?` or `#` and split on `/`, and the scope whose segments match the
//! most leading segments wins, so a scope `/a/b` receives `/a/b`, `/a/b/c` and `/a/b?x` but not
//! `/a/bc`. The handler gets the full target.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

use super::{Announcement, ChannelError, ChannelSender, PT_CHANNEL, RawChannel, RawInbound, Reader, frame, unframe};
use crate::transport::SendMode;
use phonelink_core::sync::MutexExt;

mod kind {
    pub(super) const HANDSHAKE: u32 = 1;
    pub(super) const DATA: u32 = 2;
    pub(super) const CANCEL: u32 = 3;
}

pub const PROTOCOL_VERSION: u32 = 1;
/// Bytes a Data packet adds to its fragment: the 8-byte channel header and 28 bytes of fields.
pub const DATA_OVERHEAD: usize = 36;

/// Message flags.
pub mod flags {
    pub const PLAIN: u32 = 0;
    /// The sender expects a completion.
    pub const EXPECTS_COMPLETION: u32 = 0x1;
    /// The message is a completion: it answers an earlier message and has an empty target.
    pub const COMPLETION: u32 = 0x2;
    /// Completion "no handler" sent back for a message that expected one.
    pub const COMPLETION_NO_HANDLER: u32 = 0x6;
    /// Completion "error" sent back for a message that expected one and could not be delivered.
    pub const COMPLETION_ERROR: u32 = 0xA;
}

/// Bytes a registered scope cannot hold. A scope segment is a pattern in which `.` matches any one
/// byte but a line break; these are the rest of the pattern syntax, which no scope uses, so the
/// matcher does not implement them.
const PATTERN_SYNTAX: &[u8] = b"\\^$*+?()[]{}|";

/// A registered scope: the `/`-separated segments of its path, empty ones dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scope(Vec<Vec<u8>>);

/// A scope the matcher cannot represent.
#[derive(Debug, thiserror::Error)]
enum BadScope {
    #[error("message scope {0:?} does not start with '/'")]
    NoLeadingSlash(String),
    #[error("message scope {path:?} holds {byte:?}, which the scope matcher does not support")]
    PatternSyntax { path: String, byte: char },
}

impl Scope {
    fn parse(path: &str) -> Result<Self, BadScope> {
        if !path.starts_with('/') {
            return Err(BadScope::NoLeadingSlash(path.to_owned()));
        }
        if let Some(b) = path.bytes().find(|b| PATTERN_SYNTAX.contains(b)) {
            return Err(BadScope::PatternSyntax { path: path.to_owned(), byte: b as char });
        }

        Ok(Self(path.split('/').filter(|s| !s.is_empty()).map(|s| s.as_bytes().to_vec()).collect()))
    }

    /// Panics on a scope the matcher cannot represent: scopes are the engine's own constants.
    fn of(path: &str) -> Self {
        Self::parse(path).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Whether every segment of this scope matches the target segment at its position.
    fn matches(&self, target: &[&[u8]]) -> bool {
        self.0.len() <= target.len() && self.0.iter().zip(target).all(|(p, s)| segment_matches(p, s))
    }
}

fn segment_matches(pattern: &[u8], segment: &[u8]) -> bool {
    pattern.len() == segment.len()
        && pattern.iter().zip(segment).all(|(p, s)| if *p == b'.' { !matches!(s, b'\n' | b'\r') } else { p == s })
}

/// The path segments of a target: the target cut at its first `?` or `#`, split on `/`, empty
/// segments dropped; `None` when the cut target does not start with `/`.
fn target_segments(target: &str) -> Option<Vec<&[u8]>> {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    path.starts_with('/').then(|| path.split('/').filter(|s| !s.is_empty()).map(str::as_bytes).collect())
}

struct Handler {
    scope: Scope,
    tx: mpsc::UnboundedSender<InboundMessage>,
}

/// The handler whose scope matches the most leading segments of `target`; the earlier registered
/// on a tie.
fn best<'a>(handlers: &'a [Handler], target: &[&[u8]]) -> Option<&'a Handler> {
    let mut best: Option<&Handler> = None;
    for h in handlers {
        if h.scope.matches(target) && best.is_none_or(|b| b.scope.0.len() < h.scope.0.len()) {
            best = Some(h);
        }
    }
    best
}

/// Registers `tx` for `scope`, replacing an earlier registration of the same scope.
fn insert(handlers: &mut Vec<Handler>, scope: Scope, tx: mpsc::UnboundedSender<InboundMessage>) {
    match handlers.iter_mut().find(|h| h.scope == scope) {
        Some(h) => h.tx = tx,
        None => handlers.push(Handler { scope, tx }),
    }
}

/// One reassembled inbound message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundMessage {
    pub target: String,
    pub body: Vec<u8>,
    pub flags: u32,
    pub message_id: u32,
}

/// The Data packet fields.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fragment {
    flags: u32,
    message_id: u32,
    fragment_count: u32,
    target_length: u32,
    body_length: u32,
    offset: u32,
    data: Vec<u8>,
}

impl Fragment {
    fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(28 + self.data.len());
        for v in [self.flags, self.message_id, self.fragment_count, self.target_length, self.body_length, self.offset] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(&(self.data.len() as u32).to_le_bytes());
        b.extend_from_slice(&self.data);
        frame(kind::DATA, &b)
    }

    fn decode(body: &[u8]) -> Result<Self, ChannelError> {
        let mut r = Reader::new(body);
        let flags = r.u32()?;
        let message_id = r.u32()?;
        let fragment_count = r.u32()?;
        let target_length = r.u32()?;
        let body_length = r.u32()?;
        let offset = r.u32()?;
        let len = r.u32()? as usize;
        Ok(Self { flags, message_id, fragment_count, target_length, body_length, offset, data: r.take(len)?.to_vec() })
    }
}

/// The fragments of `target ‖ body` for a channel whose packets carry at most `max_payload` bytes.
fn fragments(flags: u32, message_id: u32, target: &[u8], body: &[u8], max_payload: usize) -> Vec<Vec<u8>> {
    let capacity = max_payload.saturating_sub(DATA_OVERHEAD).max(1);
    let mut whole = Vec::with_capacity(target.len() + body.len());
    whole.extend_from_slice(target);
    whole.extend_from_slice(body);
    let count = whole.len().div_ceil(capacity).max(1) as u32;

    let mut out = Vec::new();
    let mut offset = 0;
    loop {
        let end = (offset + capacity).min(whole.len());
        out.push(
            Fragment {
                flags,
                message_id,
                fragment_count: count,
                target_length: target.len() as u32,
                body_length: body.len() as u32,
                offset: offset as u32,
                data: whole[offset..end].to_vec(),
            }
            .encode(),
        );

        offset = end;
        if offset >= whole.len() {
            return out;
        }
    }
}

struct Partial {
    buffer: Vec<u8>,
    received: Vec<bool>,
    filled: usize,
    target_length: usize,
    flags: u32,
}

type Handlers = Arc<Mutex<Vec<Handler>>>;

/// An open Message channel.
pub struct MessageChannel {
    pub announcement: Announcement,
    sender: ChannelSender,
    next_id: Arc<AtomicU32>,
    handlers: Handlers,
    closed: watch::Receiver<bool>,
}

impl std::fmt::Debug for MessageChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MessageChannel").field("announcement", &self.announcement).finish()
    }
}

impl MessageChannel {
    /// Opens the channel and runs the handshake: both sides send Handshake(1); another version
    /// fails the channel.
    pub async fn open(raw: RawChannel) -> Result<Self, ChannelError> {
        Ok(Self::open_listening(raw, &[]).await?.0)
    }

    /// [`MessageChannel::open`], with handlers for the scopes `targets` in place before the first
    /// inbound message is dispatched: what the peer sends to them right after its side opens is not
    /// lost. Panics on a scope [`MessageChannel::register`] refuses.
    pub async fn open_listening(
        mut raw: RawChannel,
        targets: &[String],
    ) -> Result<(Self, HashMap<String, mpsc::UnboundedReceiver<InboundMessage>>), ChannelError> {
        let scopes: Vec<Scope> = targets.iter().map(|t| Scope::of(t)).collect();

        let early = raw.open().await?;
        let (sender, announcement, mut inbound) = raw.split();

        sender.send(SendMode::Reliable, PT_CHANNEL, frame(kind::HANDSHAKE, &PROTOCOL_VERSION.to_le_bytes()))?;

        let mut queued = early.into_iter();
        let mut backlog = Vec::new();
        loop {
            let item = match queued.next() {
                Some(i) => i,
                None => inbound.recv().await.unwrap_or(RawInbound::Closed),
            };

            match item {
                RawInbound::Data { payload, .. } => {
                    let (k, body) = unframe(&payload)?;
                    if k == kind::HANDSHAKE {
                        let version = Reader::new(body).u32()?;
                        if version != PROTOCOL_VERSION {
                            sender.close();
                            return Err(ChannelError::Handshake(format!("message protocol version {version}")));
                        }
                        break;
                    }
                    backlog.push(payload);
                }
                RawInbound::Opened => {}
                RawInbound::Closed => return Err(ChannelError::Closed),
            }
        }

        backlog.extend(queued.filter_map(|i| match i {
            RawInbound::Data { payload, .. } => Some(payload),
            _ => None,
        }));

        let handlers: Handlers = Arc::default();
        let mut listening = HashMap::new();
        for (target, scope) in targets.iter().zip(scopes) {
            let (tx, rx) = mpsc::unbounded_channel();
            insert(&mut handlers.lock_or_panic(), scope, tx);
            listening.insert(target.clone(), rx);
        }

        let (closed_tx, closed) = watch::channel(false);
        let next_id = Arc::new(AtomicU32::new(0));
        let reader = Inbound {
            sender: sender.clone(),
            handlers: handlers.clone(),
            partial: HashMap::new(),
            scenario: announcement.scenario_id().unwrap_or_default().to_string(),
        };
        tokio::spawn(reader.run(backlog, inbound, closed_tx));
        Ok((Self { announcement, sender, next_id, handlers, closed }, listening))
    }

    /// Messages for the scope `target` and every path under it that no longer registered scope
    /// claims, from now on; replaces an earlier registration of the same scope. Panics when `target`
    /// does not start with `/` or holds pattern syntax other than `.`.
    pub fn register(&self, target: impl Into<String>) -> mpsc::UnboundedReceiver<InboundMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        insert(&mut self.handlers.lock_or_panic(), Scope::of(&target.into()), tx);
        rx
    }

    pub fn unregister(&self, target: &str) {
        let scope = Scope::of(target);
        self.handlers.lock_or_panic().retain(|h| h.scope != scope);
    }

    /// Sends a plain message (flags 0).
    pub fn send(&self, target: &str, body: &[u8]) -> Result<(), ChannelError> {
        if *self.closed.borrow() {
            return Err(ChannelError::Closed);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        debug!(target, len = body.len(), body = %String::from_utf8_lossy(&body[..body.len().min(300)]), "message sent");

        for f in fragments(flags::PLAIN, id, target.as_bytes(), body, self.sender.max_payload()) {
            self.sender.send(SendMode::Reliable, PT_CHANNEL, f)?;
        }
        Ok(())
    }

    /// Becomes `true` when the channel closes.
    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    pub fn close(&self) {
        self.sender.close();
    }
}

struct Inbound {
    sender: ChannelSender,
    handlers: Handlers,
    partial: HashMap<u32, Partial>,
    scenario: String,
}

impl Inbound {
    async fn run(
        mut self,
        backlog: Vec<Vec<u8>>,
        mut inbound: mpsc::UnboundedReceiver<RawInbound>,
        closed: watch::Sender<bool>,
    ) {
        for payload in backlog {
            self.on_packet(&payload);
        }

        while let Some(item) = inbound.recv().await {
            match item {
                RawInbound::Data { payload, .. } => self.on_packet(&payload),
                RawInbound::Opened => {}
                RawInbound::Closed => break,
            }
        }

        debug!(scenario = %self.scenario, "Nano message channel closed");
        let _ = closed.send(true);
    }

    fn on_packet(&mut self, payload: &[u8]) {
        let (k, body) = match unframe(payload) {
            Ok(x) => x,
            Err(e) => return warn!(error = %e, "malformed message-channel packet dropped"),
        };

        if k == kind::DATA {
            match Fragment::decode(body) {
                Ok(f) => self.on_fragment(f),
                Err(e) => warn!(error = %e, "malformed message fragment dropped"),
            }
        } else {
            log_control_packet(k);
        }
    }

    fn on_fragment(&mut self, f: Fragment) {
        let Some(p) = self.fill_partial(&f) else {
            return;
        };

        let target = String::from_utf8_lossy(&p.buffer[..p.target_length]).into_owned();
        let message = InboundMessage {
            body: p.buffer[p.target_length..].to_vec(),
            target,
            flags: p.flags,
            message_id: f.message_id,
        };
        self.dispatch(message);
    }

    /// Writes a fragment into its partial message; the message once every byte has arrived, `None`
    /// while it is incomplete or when the fragment is dropped (logged).
    fn fill_partial(&mut self, f: &Fragment) -> Option<Partial> {
        let total = f.target_length as usize + f.body_length as usize;
        let start = f.offset as usize;
        let end = start + f.data.len();

        if end > total {
            warn!(message_id = f.message_id, "message fragment outside its message dropped");
            return None;
        }

        let p = self.partial.entry(f.message_id).or_insert_with(|| Partial {
            buffer: vec![0; total],
            received: vec![false; total],
            filled: 0,
            target_length: f.target_length as usize,
            flags: f.flags,
        });

        if p.buffer.len() != total {
            warn!(message_id = f.message_id, "message fragment that disagrees with its message's length dropped");
            return None;
        }

        for i in start..end {
            if !p.received[i] {
                p.received[i] = true;
                p.filled += 1;
            }
        }
        p.buffer[start..end].copy_from_slice(&f.data);

        if p.filled < total {
            return None;
        }
        Some(self.partial.remove(&f.message_id).expect("present"))
    }

    fn dispatch(&mut self, message: InboundMessage) {
        debug!(
            raw_target = %message.target,
            scenario = %self.scenario,
            body = %String::from_utf8_lossy(&message.body[..message.body.len().min(300)]),
            "message received"
        );

        let Some(path) = self.request_path(&message) else {
            return;
        };

        if self.deliver(&message, &path) {
            return;
        }

        warn!(raw_target = %message.target, scenario = %self.scenario, "message for a target with no handler dropped");
        self.complete(&message, flags::COMPLETION_NO_HANDLER);
    }

    /// The path of a request's target; `None` for a completion or a malformed target (answered with
    /// a completion error when it expects one).
    fn request_path<'m>(&self, message: &'m InboundMessage) -> Option<Vec<&'m [u8]>> {
        let is_completion = message.flags & flags::COMPLETION != 0;
        if is_completion != message.target.is_empty() {
            warn!(raw_target = %message.target, flags = message.flags, "message whose target disagrees with its flags dropped");
            self.complete(message, flags::COMPLETION_ERROR);
            return None;
        }

        if is_completion {
            // This side never asks for a completion, so there is nothing for one to answer.
            debug!(message_id = message.message_id, flags = message.flags, "unsolicited completion dropped");
            return None;
        }

        let Some(path) = target_segments(&message.target) else {
            warn!(raw_target = %message.target, scenario = %self.scenario, "message whose target is not a path dropped");
            self.complete(message, flags::COMPLETION_ERROR);
            return None;
        };

        Some(path)
    }

    /// Hands the message to the most specific live handler of `path`, unregistering handlers whose
    /// receiver is gone; `false` when no handler takes it.
    fn deliver(&self, message: &InboundMessage, path: &[&[u8]]) -> bool {
        loop {
            let handler = best(&self.handlers.lock_or_panic(), path).map(|h| h.tx.clone());
            let Some(tx) = handler else {
                return false;
            };

            if tx.send(message.clone()).is_ok() {
                return true;
            }

            // The handler's receiver is gone: it is no longer registered.
            self.handlers.lock_or_panic().retain(|h| !h.tx.is_closed());
        }
    }

    /// Answers `message` with the completion `completion_flags` when it expects one.
    fn complete(&self, message: &InboundMessage, completion_flags: u32) {
        if message.flags & flags::EXPECTS_COMPLETION == 0 {
            return;
        }

        // A completion carries an empty target: the phone drops one that names a target.
        for f in fragments(completion_flags, message.message_id, b"", &[], self.sender.max_payload()) {
            if let Err(e) = self.sender.send(SendMode::Reliable, PT_CHANNEL, f) {
                super::log_not_sent("message", &e);
            }
        }
    }
}

/// A packet other than Data: a repeated handshake or a cancel request is ignored, an unknown kind
/// dropped.
fn log_control_packet(k: u32) {
    match k {
        kind::HANDSHAKE => debug!("repeated message-channel handshake; ignored"),
        kind::CANCEL => debug!("message cancel request; nothing in flight to cancel"),
        other => warn!(kind = other, "message-channel packet of an unknown kind dropped"),
    }
}

#[cfg(test)]
mod tests;
