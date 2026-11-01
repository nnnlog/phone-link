//! The options and timers of the `photos` feature, at their defaults.

use std::time::Duration;

/// Every flag, timer and limit of photos, at its default.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotosConfig {
    /// `AutoHydratedThumbnailsCount` (default `25`): the connect value and the maximum inline
    /// thumbnails requested per metadata message.
    pub auto_hydrated_thumbnails_count: i32,
    /// Optimized thumbnails (default `false`): no inline thumbnails, no per-item lazy
    /// thumbnails; one batch per grid layout instead.
    pub media_optimized_thumbnails: bool,
    /// The delete request's timeout (default `60000` ms).
    pub media_delete_timeout: Duration,
    /// Month grouping in the gallery (default `false`); no wire effect.
    pub media_grouping: bool,
    /// `MediaInfoSnapshotSize` (fixed `2000`).
    pub media_info_snapshot_size: i32,
    /// Thumbnail batch size (fixed `25`).
    pub thumbnail_batch: usize,
    /// Full-resolution requests in flight (fixed `5`).
    pub full_concurrency: usize,
    /// Delay of the thumbnail worker's first batch (`1000` ms).
    pub thumbnail_initial_delay: Duration,
    /// Failures counted before the give-up (`2`: the 4th failure gives up).
    pub allowed_failures: u32,
    /// The pause before each batch or request is `min(min · 2^consecutiveFailures, max)`. Both
    /// default to `0`: retries at once, as Phone Link on Windows does.
    pub retry_pause_min: Duration,
    pub retry_pause_max: Duration,
    /// The "syncing" indicator shows for at most this long (`2` min).
    pub syncing_indicator_max: Duration,
    /// File actions and the notification activation wait at most this long for full bytes (`30` s).
    pub full_bytes_wait: Duration,
    /// A successfully deleted item leaves the displayed gallery after this delay (`250` ms).
    pub delete_removal_delay: Duration,
    /// Cache maintenance runs at start and with this period (`24` h).
    pub maintenance_interval: Duration,
    /// Maintenance keeps the full bytes of this many most recent items (`25`).
    pub dehydrate_keep_recent: u32,
    /// Maintenance drops full bytes not seen for this long (`30` days).
    pub dehydrate_after: Duration,
    /// New-photo heuristic window (`120` s).
    pub new_photo_window: Duration,
    /// New-photo candidates per batch (`3`).
    pub new_photo_candidates: usize,
    /// A notification within this time of the previous pop-up is silent (`7` s).
    pub new_photo_silent_window: Duration,
    /// The viewer fetches this many following and preceding items (`2`).
    pub viewer_neighbours: usize,
    /// The full-access reminder needs this Android SDK version (`34`).
    pub full_access_reminder_min_sdk: u32,
    /// The delete action's permission check applies from this Android major version (`13`).
    pub post_notification_ask_min_os: i32,
}

impl Default for PhotosConfig {
    fn default() -> Self {
        Self {
            auto_hydrated_thumbnails_count: 25,
            media_optimized_thumbnails: false,
            media_delete_timeout: Duration::from_millis(60_000),
            media_grouping: false,
            media_info_snapshot_size: 2000,
            thumbnail_batch: 25,
            full_concurrency: 5,
            thumbnail_initial_delay: Duration::from_millis(1000),
            allowed_failures: 2,
            retry_pause_min: Duration::ZERO,
            retry_pause_max: Duration::ZERO,
            syncing_indicator_max: Duration::from_secs(120),
            full_bytes_wait: Duration::from_secs(30),
            delete_removal_delay: Duration::from_millis(250),
            maintenance_interval: Duration::from_secs(24 * 3600),
            dehydrate_keep_recent: 25,
            dehydrate_after: Duration::from_secs(30 * 24 * 3600),
            new_photo_window: Duration::from_secs(120),
            new_photo_candidates: 3,
            new_photo_silent_window: Duration::from_secs(7),
            viewer_neighbours: 2,
            full_access_reminder_min_sdk: 34,
            post_notification_ask_min_os: 13,
        }
    }
}
