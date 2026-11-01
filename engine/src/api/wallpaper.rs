//! Wallpaper for the UI ([`crate::features::wallpaper`]): the selected phone's wallpaper as the main
//! window shows it, and its change event.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::wallpaper::{ShownWallpaper, WallpaperCopy, WallpaperEvent};

use super::{FeatureApiError, core_of, features_of, selected_phone};

/// Which copy of the phone's wallpaper is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WallpaperOrigin {
    /// What the phone delivered as `wallpaper` content.
    PhoneLink,
    /// What the Cross Device pull fetched.
    CrossDevice,
}

/// The selected phone's wallpaper.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhoneWallpaper {
    /// The image as the phone sent it (normally JPEG); decode it, do not trust its type.
    pub image: Vec<u8>,
    pub origin: WallpaperOrigin,
}

impl From<ShownWallpaper> for PhoneWallpaper {
    fn from(w: ShownWallpaper) -> Self {
        Self {
            image: w.image,
            origin: match w.copy {
                WallpaperCopy::PhoneLink => WallpaperOrigin::PhoneLink,
                WallpaperCopy::CrossDevice => WallpaperOrigin::CrossDevice,
            },
        }
    }
}

/// A change to the wallpaper: re-read [`WallpaperApi::wallpaper`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WallpaperChange {
    /// What `wallpaper` gives may have changed.
    Changed,
}

impl From<&WallpaperEvent> for WallpaperChange {
    fn from(e: &WallpaperEvent) -> Self {
        match e {
            WallpaperEvent::Changed { .. } => WallpaperChange::Changed,
        }
    }
}

/// The wallpaper of the selected phone.
#[derive(uniffi::Object)]
pub struct WallpaperApi {
    engine: Arc<Engine>,
}

impl WallpaperApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }
}

#[uniffi::export]
impl WallpaperApi {
    /// The selected phone's wallpaper; `None` when there is none to show.
    pub fn wallpaper(&self) -> Result<Option<PhoneWallpaper>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.wallpaper.clone();
        let phone = selected_phone(&core)?;
        Ok(feature.shown(&phone)?.map(Into::into))
    }
}
