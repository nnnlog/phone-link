//! The options and timers of the `phone_files` feature, at their defaults.

use std::path::PathBuf;
use std::time::Duration;

/// Every option and timer of the feature, at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneFilesConfig {
    /// Metadata requests go over the files-request channel when the phone can take them there.
    pub sync_over_nano: bool,
    /// Thumbnails go over the thumbnail channel when the phone allows it.
    pub thumbnails_over_nano: bool,
    /// Thumbnails are prefetched over the thumbnail channel when the phone allows it.
    pub prefetch_over_nano: bool,
    /// Full syncs are asked for in batches (off).
    pub batched_full_sync: bool,
    /// Creating and opening the files-request channel (`20` s + `20` s).
    pub request_channel_timeout: Duration,
    /// The wait for a metadata reply on the channel (`180` s).
    pub request_reply_timeout: Duration,
    /// Creating and opening the thumbnail channel (`60` s + `60` s).
    pub thumbnail_channel_timeout: Duration,
    /// The wait for a thumbnail reply on the channel (`10` s).
    pub thumbnail_reply_timeout: Duration,
    /// Creating and opening the content channel (`60` s + `60` s).
    pub content_channel_timeout: Duration,
    /// Consecutive channel failures that open the breaker (`3`).
    pub breaker_threshold: u32,
    /// How long the breaker stays open (`60` s).
    pub breaker_open: Duration,
    /// Starting the feature runs a full refresh when the last one is older than this (`24` h).
    pub full_refresh_interval: Duration,
    /// A full request that timed out is repeated once after this (`20` s).
    pub full_sync_retry_delay: Duration,
    /// Deletes are collected this long after the last one before the batch is sent (`500` ms).
    pub delete_debounce: Duration,
    /// An upload that failed is tried again this many times (`10`)…
    pub upload_retries: u32,
    /// …with exponential back-off from this delay (`1` s) and jitter.
    pub upload_backoff: Duration,
    /// The edge of a thumbnail, in pixels (`256`).
    pub thumbnail_size: i32,
    /// Where downloaded content is written before the file manager takes it.
    pub content_dir: PathBuf,
}

impl Default for PhoneFilesConfig {
    fn default() -> Self {
        Self {
            sync_over_nano: true,
            thumbnails_over_nano: true,
            prefetch_over_nano: true,
            batched_full_sync: false,
            request_channel_timeout: Duration::from_secs(40),
            request_reply_timeout: Duration::from_secs(180),
            thumbnail_channel_timeout: Duration::from_secs(120),
            thumbnail_reply_timeout: Duration::from_secs(10),
            content_channel_timeout: Duration::from_secs(120),
            breaker_threshold: 3,
            breaker_open: Duration::from_secs(60),
            full_refresh_interval: Duration::from_secs(24 * 60 * 60),
            full_sync_retry_delay: Duration::from_secs(20),
            delete_debounce: Duration::from_millis(500),
            upload_retries: 10,
            upload_backoff: Duration::from_secs(1),
            thumbnail_size: 256,
            content_dir: std::env::temp_dir().join("PhoneLinkPhoneFiles"),
        }
    }
}
