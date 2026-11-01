//! The options and timers of the `clipboard` feature, at their defaults.

use std::time::Duration;

/// Every timer and limit of the clipboard, at its default; tests shorten the timers.
#[derive(Debug, Clone)]
pub struct ClipboardConfig {
    /// Pasteboard change-count poll: 250 ms.
    pub poll_interval: Duration,
    /// Trailing debounce of detected pasteboard changes: 200 ms.
    pub debounce: Duration,
    /// Plain-text read limit, also for HTML: 131,072 UTF-16 code units.
    pub text_read_limit: usize,
    /// Outbound image: PNG at most 1,048,576 bytes …
    pub image_byte_limit: usize,
    /// … after at most 5 downscale passes.
    pub image_max_passes: u32,
    /// Bounds every PC request, send plus wait: 120 s. The content pull has no shorter timeout and
    /// no cancellation.
    pub request_timeout: Duration,
}

/// The outbound PNG limit `L` (bytes).
pub const PNG_LIMIT: usize = 1_048_576;

/// At most this many downscale passes.
pub const MAX_PASSES: u32 = 5;

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(250),
            debounce: Duration::from_millis(200),
            text_read_limit: 131_072,
            image_byte_limit: PNG_LIMIT,
            image_max_passes: MAX_PASSES,
            request_timeout: Duration::from_secs(120),
        }
    }
}
