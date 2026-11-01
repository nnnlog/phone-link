//! The app's log: the engine's `tracing` events and the lines the app sends through [`record`], in
//! size-bounded files of one directory (macOS: `~/Library/Logs/<bundle id>/`).
//!
//! No value the code did not classify is written in the clear. A field is written as it is only when
//! its name is on the public list ([`policy`]), error texts are scrubbed of identifier shapes, every
//! other field becomes `{{REDACTED:<field>|<tag>}}`, and the app marks each part of its own lines
//! public (scrubbed like an error text) or private. Messages are constants (`scripts/log_calls.py`), so a saved copy of the log
//! needs no further redaction.
//!
//! The CLI logs to stderr through its own subscriber and does not use this module.

mod format;
mod policy;
mod redact;
mod store;

#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::SubscriberBuilder;
use tracing_subscriber::fmt::format::DefaultFields;

use self::format::LineFormat;
use self::redact::Redactor;
use self::store::LogStore;

/// The file earlier builds wrote, unbounded and unredacted; [`install`] removes it.
const LEGACY_FILE: &str = "engine.log";

/// The level of a line the app records. `Notice`, the unified log's level for what the engine logs at
/// `warn`, is written as `WARN`, and `Fault` as `ERROR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Debug,
    Info,
    Notice,
    Error,
    Fault,
}

/// One part of a line the app records: scrubbed of identifier shapes when `public`, a token otherwise.
#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    pub text: &'a str,
    pub public: bool,
}

/// Why the log could not be installed, saved, or cleared.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("the log is not installed")]
    NotInstalled,
    #[error("the log is installed already")]
    AlreadyInstalled,
    #[error("opening the log file {}: {source}", path.display())]
    Open { path: PathBuf, source: io::Error },
    #[error("reading or writing the log key {}: {source}", path.display())]
    Key { path: PathBuf, source: io::Error },
    #[error("saving the log to {}: {source}", path.display())]
    Export { path: PathBuf, source: io::Error },
    #[error("clearing the log file {}: {source}", path.display())]
    Clear { path: PathBuf, source: io::Error },
    #[error("installing the log subscriber: {0}")]
    Subscriber(#[source] Box<dyn std::error::Error + Send + Sync>),
}

struct Log {
    store: LogStore,
    redactor: Arc<Redactor>,
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Installs the process-wide subscriber that writes to the log in `dir` (created when missing), at
/// `info` unless `RUST_LOG` says otherwise. Only the first call takes effect; a later call is an
/// error.
pub fn install(dir: &Path) -> Result<(), LogError> {
    if LOG.get().is_some() {
        return Err(LogError::AlreadyInstalled);
    }

    fs::create_dir_all(dir).map_err(|e| LogError::Open { path: dir.to_owned(), source: e })?;
    match fs::remove_file(dir.join(LEGACY_FILE)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(LogError::Clear { path: dir.join(LEGACY_FILE), source: e }),
    }

    let redactor = Arc::new(Redactor::open(dir)?);
    let store = LogStore::open(dir)?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    subscriber(store.clone(), Arc::clone(&redactor), filter).try_init().map_err(LogError::Subscriber)?;
    LOG.set(Log { store, redactor }).map_err(|_| LogError::AlreadyInstalled)
}

/// The subscriber [`install`] installs, writing to `store`.
fn subscriber(
    store: LogStore,
    redactor: Arc<Redactor>,
    filter: EnvFilter,
) -> SubscriberBuilder<DefaultFields, LineFormat, EnvFilter, LogStore> {
    tracing_subscriber::fmt().with_env_filter(filter).with_writer(store).event_format(LineFormat::new(redactor))
}

/// Writes a line of the app's, under the target `gui` with its `category`. Without an installed
/// log the line goes nowhere.
pub fn record(level: Level, category: &str, segments: &[Segment<'_>]) {
    let Some(log) = LOG.get() else { return };
    emit(level, category, &render(&log.redactor, segments));
}

/// `segments` joined, each private one replaced by its token and each public one scrubbed: the app
/// marks an error's description public, and a Foundation error quotes the paths and URLs it failed on.
fn render(redactor: &Redactor, segments: &[Segment<'_>]) -> String {
    segments.iter().map(|s| if s.public { redactor.scrub(s.text) } else { redactor.token("private", s.text) }).collect()
}

fn emit(level: Level, category: &str, line: &str) {
    match level {
        Level::Debug => tracing::debug!(target: "gui", category, "{line}"),
        Level::Info => tracing::info!(target: "gui", category, "{line}"),
        Level::Notice | Level::Error | Level::Fault => emit_alert(level, category, line),
    }
}

/// [`emit`] for the levels above `info`, apart because each `tracing` call weighs on the function's
/// complexity.
fn emit_alert(level: Level, category: &str, line: &str) {
    if level == Level::Notice {
        tracing::warn!(target: "gui", category, "{line}");
    } else {
        tracing::error!(target: "gui", category, "{line}");
    }
}

/// The bytes the log holds; 0 when it is not installed.
pub fn size() -> u64 {
    LOG.get().map_or(0, |log| log.store.size())
}

/// Writes the whole log, oldest line first, to `destination`.
pub fn export(destination: &Path) -> Result<(), LogError> {
    LOG.get().ok_or(LogError::NotInstalled)?.store.export(destination)
}

/// Empties the log and replaces the tagging key, so lines written after it cannot be matched to
/// earlier ones by their tags.
pub fn clear() -> Result<(), LogError> {
    let log = LOG.get().ok_or(LogError::NotInstalled)?;
    log.store.clear()?;
    log.redactor.rekey()
}
