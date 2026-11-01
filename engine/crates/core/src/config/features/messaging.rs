//! The options and timers of the `messaging` feature, at their defaults.

use std::time::Duration;

/// Every timer, flag and limit of messaging, at its default.
#[derive(Debug, Clone, PartialEq)]
pub struct MessagingConfig {
    /// The mark-as-read delay after a selection or focus trigger.
    pub mark_as_read_delay: Duration,
    /// New-message notifications only for messages at most this old.
    pub notification_recency: Duration,
    /// How long the messages UI shows its syncing state at most while not up to date.
    pub syncing_display_limit: Duration,
    /// Whether audio, video, document and contact-card parts can be downloaded and sent; off by
    /// default.
    pub audio_parts_enabled: bool,
    pub video_parts_enabled: bool,
    pub document_parts_enabled: bool,
    pub vcard_parts_enabled: bool,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            mark_as_read_delay: Duration::from_millis(500),
            notification_recency: Duration::from_secs(5 * 60),
            syncing_display_limit: Duration::from_secs(2 * 60),
            audio_parts_enabled: false,
            video_parts_enabled: false,
            document_parts_enabled: false,
            vcard_parts_enabled: false,
        }
    }
}
