//! The options and timers of the `wallpaper` feature, at their defaults.

/// The feature's options, at their defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallpaperConfig {
    /// Answer the phone's `/wallpaper` requests with the Mac's desktop picture (default on). Off, no
    /// receiver is registered and the phone's requests go unanswered.
    pub pc_wallpaper_sync_enabled: bool,
}

impl Default for WallpaperConfig {
    fn default() -> Self {
        Self { pc_wallpaper_sync_enabled: true }
    }
}
