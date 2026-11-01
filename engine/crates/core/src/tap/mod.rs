//! The decode seam for observing live traffic: every layer reports what it sends and receives as a
//! [`TapEvent`], already unwrapped to plaintext inside the process.
//!
//! The CLI writes them as JSON lines into a directory the caller chooses (a gitignored `raw/`
//! dump), and so does the app when it is launched with `PHONELINK_TAP_DIR` set ([`tap_from_env`]);
//! tests collect them in memory. Events are **unredacted** except for bearer tokens, which no
//! analysis needs: [`redact_secret_header`] replaces them before an event is built.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sync::MutexExt;
use serde::{Deserialize, Serialize};

/// The seam an event was taken at, bottom-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TapLayer {
    /// A DCG / MSA REST call (method, URL, status, bodies).
    Rest,
    /// One `SignalR` hub message (handshake, invocation, completion, ping, close).
    HubMessage,
    /// One `ms-dcg` multiplex packet (fragment or ack property map).
    DcgPacket,
    /// One reassembled binary inner envelope (headers + payload length).
    Envelope,
    /// One platform message (route, headers, decoded body).
    Platform,
    /// One App message (headers + `ValueSet`).
    App,
    /// Something a layer could not decode (layer named in the summary).
    Undecodable,
}

/// Which way a message went: into the Mac or out of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    In,
    Out,
}

/// One observation at a seam.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TapEvent {
    pub ts_unix_ms: u64,
    pub layer: TapLayer,
    pub direction: Direction,
    /// The partner (`<dcgClientId>.<Environment>`) or environment the event belongs to, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partner: Option<String>,
    /// The layer's decoded view.
    pub summary: serde_json::Value,
    /// The raw bytes at this seam, hex-encoded, when the layer has them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_hex: Option<String>,
    /// The PC identity (`clientType`, `PL` or `WEA`) whose stack took the event, set by
    /// [`LabeledTap`]; both identities talk to the same phone, so the partner alone does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

impl TapEvent {
    /// An event of `layer` going `direction`, with no bytes or decoded body yet.
    pub fn new(
        layer: TapLayer,
        direction: Direction,
        partner: Option<String>,
        summary: serde_json::Value,
        bytes: Option<&[u8]>,
    ) -> Self {
        let ts_unix_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        Self { ts_unix_ms, layer, direction, partner, summary, bytes_hex: bytes.map(hex::encode), identity: None }
    }
}

/// Receives the decode seam's events.
pub trait TapSink: Send + Sync {
    /// Records or forwards one event.
    fn emit(&self, event: TapEvent);
}

/// The shared handle every layer emits its events to.
pub type Tap = Arc<dyn TapSink>;

/// Discards everything.
#[derive(Debug, Default)]
pub struct NullTap;

impl TapSink for NullTap {
    fn emit(&self, _event: TapEvent) {}
}

/// A tap that discards everything.
pub fn null_tap() -> Tap {
    Arc::new(NullTap)
}

/// Collects events in memory (tests).
#[derive(Debug, Default)]
pub struct MemoryTap(Mutex<Vec<TapEvent>>);

impl MemoryTap {
    /// An empty in-memory tap.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    /// The events recorded so far, removing them.
    pub fn take(&self) -> Vec<TapEvent> {
        std::mem::take(&mut *self.0.lock_or_panic())
    }
    /// A copy of the events recorded so far.
    pub fn snapshot(&self) -> Vec<TapEvent> {
        self.0.lock_or_panic().clone()
    }
}

impl TapSink for MemoryTap {
    fn emit(&self, event: TapEvent) {
        self.0.lock_or_panic().push(event);
    }
}

/// Stamps every event with the identity of the stack that took it, then passes it on.
pub struct LabeledTap {
    inner: Tap,
    identity: &'static str,
}

impl LabeledTap {
    /// A tap that labels every event with `identity` and passes it to `inner`.
    pub fn wrap(inner: Tap, identity: &'static str) -> Tap {
        Arc::new(Self { inner, identity })
    }
}

impl TapSink for LabeledTap {
    fn emit(&self, mut event: TapEvent) {
        event.identity = Some(self.identity.to_string());
        self.inner.emit(event);
    }
}

/// Writes one JSON object per line.
pub struct JsonlTap {
    writer: Mutex<Box<dyn Write + Send>>,
    /// An event was lost: the next losses (a full disk fails every event) are not logged.
    write_failed: AtomicBool,
}

impl JsonlTap {
    /// A tap that writes every event to `writer` as one JSON line.
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self { writer: Mutex::new(writer), write_failed: AtomicBool::new(false) }
    }
}

impl TapSink for JsonlTap {
    fn emit(&self, event: TapEvent) {
        let written = serde_json::to_string(&event).map_err(std::io::Error::from).and_then(|line| {
            let mut w = self.writer.lock_or_panic();
            writeln!(w, "{line}").and_then(|()| w.flush())
        });

        if let Err(e) = written
            && !self.write_failed.swap(true, Ordering::Relaxed)
        {
            tracing::warn!(error = %e, layer_kind = ?event.layer, "tap event not written; later losses are not logged");
        }
    }
}

/// The environment variable naming the directory the app's engine writes its tap into.
pub const TAP_DIR_VAR: &str = "PHONELINK_TAP_DIR";

/// A [`JsonlTap`] writing `tap-<unix seconds>.jsonl` in the directory [`TAP_DIR_VAR`] names (created
/// `0700` when missing; the file is `0600`), or [`null_tap`] when the variable is unset or empty.
///
/// A directory or file that cannot be created is an error: the caller asked for a tap.
pub fn tap_from_env() -> std::io::Result<Tap> {
    let Some(dir) = std::env::var_os(TAP_DIR_VAR).filter(|d| !d.is_empty()) else {
        return Ok(null_tap());
    };

    let dir = std::path::PathBuf::from(dir);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(&dir)?;

    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(dir.join(format!("tap-{secs}.jsonl")))?;
    Ok(Arc::new(JsonlTap::new(Box::new(std::io::BufWriter::new(file)))))
}

/// Header names whose values are bearer credentials.
const SECRET_HEADERS: [&str; 3] = ["authorization", "useridentitytoken", "dcg-token"];

/// Replaces a credential header's value with `<redacted N bytes>`; other values pass unchanged.
pub fn redact_secret_header(name: &str, value: &str) -> String {
    if SECRET_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
        format!("<redacted {} bytes>", value.len())
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests;
