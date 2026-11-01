//! The app's log, which the app installs at launch.
//!
//! Without a subscriber every engine log line is dropped, so the app installs the log in a directory
//! of its choosing (macOS: `~/Library/Logs/<bundle id>/`), sends its own lines to it, and saves or
//! clears it from Settings.

use std::path::Path;

use crate::api::FeatureApiError;
use crate::logging::{self, LogError, Segment};

/// The level of a line the app records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LogLevel {
    Debug,
    Info,
    Notice,
    Error,
    Fault,
}

/// One part of a line the app records: scrubbed of identifier shapes when `is_public`, as a
/// redaction token otherwise.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LogSegment {
    pub text: String,
    pub is_public: bool,
}

impl From<LogError> for FeatureApiError {
    fn from(e: LogError) -> Self {
        match e {
            LogError::NotInstalled => FeatureApiError::NotAvailable { reason: e.to_string() },
            _ => FeatureApiError::Internal { message: e.to_string() },
        }
    }
}

/// Installs the log in `directory` (created when missing), at `info` unless `RUST_LOG` says
/// otherwise. Installs the process-wide subscriber, so only the first call takes effect; a later
/// call is an error.
#[uniffi::export]
pub fn install_log(directory: String) -> Result<(), FeatureApiError> {
    Ok(logging::install(Path::new(&directory))?)
}

/// Writes one line of the app's to the log, under `category`.
#[uniffi::export]
pub fn log_message(level: LogLevel, category: String, segments: Vec<LogSegment>) {
    let level = match level {
        LogLevel::Debug => logging::Level::Debug,
        LogLevel::Info => logging::Level::Info,
        LogLevel::Notice => logging::Level::Notice,
        LogLevel::Error => logging::Level::Error,
        LogLevel::Fault => logging::Level::Fault,
    };
    let segments: Vec<Segment<'_>> = segments.iter().map(|s| Segment { text: &s.text, public: s.is_public }).collect();
    logging::record(level, &category, &segments);
}

/// The bytes the log holds; 0 before [`install_log`].
#[uniffi::export]
pub fn log_size() -> u64 {
    logging::size()
}

/// Writes the whole log, oldest line first, to the file at `destination`, replacing it.
#[uniffi::export]
pub fn export_log(destination: String) -> Result<(), FeatureApiError> {
    Ok(logging::export(Path::new(&destination))?)
}

/// Empties the log.
#[uniffi::export]
pub fn clear_log() -> Result<(), FeatureApiError> {
    Ok(logging::clear()?)
}
