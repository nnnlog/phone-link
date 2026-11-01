//! The options and timers of the `file_transfer` feature, at their defaults.

use std::path::PathBuf;
use std::time::Duration;

/// Every option and timer of the feature, at its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileTransferConfig {
    /// The Mac receives files from the phone: it advertises `FILE_TRANSFER`, and the received-files
    /// folder setting may show (default on). The receive request is answered either way.
    pub phone_to_pc_enabled: bool,
    /// Phones are offered as send targets, and the *Share* toggle shows (default on). Phone Link on
    /// Windows does not offer an Android phone as a send target by default.
    pub share_enabled: bool,
    /// The wait for the phone's context handle after the `GetContextHandle` request (`120` s).
    pub handle_timeout: Duration,
    /// Creating and opening a transfer's channel (`120` s).
    pub channel_timeout: Duration,
    /// A receive's channel is closed this long after the session ended (`10` s).
    pub receive_dispose_delay: Duration,
    /// The received-files folder of a phone that has none of its own: the user's Downloads folder +
    /// `Mobile Devices`; `None` when the home folder is unknown (a receive then has no destination).
    pub default_folder: Option<PathBuf>,
    /// How often a running transfer's progress is published (the UI and the progress notification).
    pub progress_interval: Duration,
}

impl Default for FileTransferConfig {
    fn default() -> Self {
        Self {
            phone_to_pc_enabled: true,
            share_enabled: true,
            handle_timeout: Duration::from_secs(120),
            channel_timeout: Duration::from_secs(120),
            receive_dispose_delay: Duration::from_secs(10),
            default_folder: std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join("Downloads").join("Mobile Devices")),
            progress_interval: Duration::from_millis(500),
        }
    }
}
