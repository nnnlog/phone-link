//! The desktop picture this computer shows, for the phone's `/wallpaper` request.
//!
//! The running desktop's own setting names the picture: GNOME, Budgie, Cinnamon and MATE keep it in
//! gsettings, KDE Plasma's shell answers for its primary screen over D-Bus (its default picture comes
//! from the look-and-feel package), and Xfce keeps it in xfconf per monitor and workspace. The picture
//! is decoded — the image formats of the `image` crate, JPEG XL and SVG, and GNOME's slideshow and
//! timed backgrounds as the picture they show now —, converted to sRGB, scaled to the engine's size
//! rule and encoded as JPEG, on every request: the phone asks when it wants the picture, so nothing
//! watches for changes. A desktop this app does not know, or a setting that cannot be read, is an
//! error; no picture set (or its file gone) is no picture; a picture that does not decode is
//! unreadable.

mod gsettings;
mod picture;
mod plasma;
mod slideshow;
mod xfce;

use std::path::PathBuf;

use gsettings::{gnome_setting, mate_setting};
use phonelink_engine::edge::EdgeError;
use phonelink_wallpaper::{PcWallpaper, PcWallpaperSource};
use picture::picture_of;
use plasma::{package_picture, plasma_dark, plasma_setting};
use tracing::{info, warn};
use xfce::xfce_setting;

use crate::edges::unexpected;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

/// The desktops whose picture setting this app reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Desktop {
    /// GNOME Shell (and GNOME Classic and Flashback): `org.gnome.desktop.background`, with its dark
    /// picture while the dark style is on.
    Gnome,
    /// Budgie: `org.gnome.desktop.background`'s picture.
    Budgie,
    Cinnamon,
    Mate,
    Plasma,
    Xfce,
}

impl Desktop {
    /// The first desktop of `XDG_CURRENT_DESKTOP` (`:`-separated) that this app knows.
    fn from_current_desktop(value: &str) -> Option<Self> {
        value.split(':').find_map(|token| match token.to_ascii_lowercase().as_str() {
            "gnome" | "gnome-classic" | "gnome-flashback" => Some(Self::Gnome),
            "budgie" => Some(Self::Budgie),
            "x-cinnamon" | "cinnamon" => Some(Self::Cinnamon),
            "mate" => Some(Self::Mate),
            "kde" => Some(Self::Plasma),
            "xfce" => Some(Self::Xfce),
            _ => None,
        })
    }
}

/// What a desktop's setting names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Setting {
    /// No picture: a plain color, or no picture file named.
    NoPicture,
    /// The picture file (or, for Plasma, a wallpaper package folder).
    Picture(PathBuf),
}

impl Setting {
    /// A picture setting given as a URI (or, as older settings write it, an absolute path). Only a
    /// local file is a picture this app can read.
    fn from_uri(uri: &str) -> Self {
        if uri.starts_with('/') {
            return Self::Picture(PathBuf::from(uri));
        }

        match crate::images::local_path(uri) {
            Some(path) if !path.is_empty() => Self::Picture(PathBuf::from(path)),
            _ => Self::NoPicture,
        }
    }

    /// A picture setting given as a path, empty for none.
    fn from_path(path: &str) -> Self {
        if path.is_empty() { Self::NoPicture } else { Self::Picture(PathBuf::from(path)) }
    }
}

/// The desktop picture of the session this app runs in.
pub struct DesktopWallpaper {
    /// `XDG_CURRENT_DESKTOP`, for the error of a desktop this app does not know.
    current_desktop: String,
    desktop: Option<Desktop>,
}

impl DesktopWallpaper {
    pub fn detect() -> Self {
        let current_desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        let desktop = Desktop::from_current_desktop(&current_desktop);

        match desktop {
            Some(desktop) => info!(?desktop, "the desktop picture is read from the desktop's settings"),
            None => warn!(current_desktop, "this desktop's picture is not read; the phone keeps the one it has"),
        }

        Self { current_desktop, desktop }
    }

    /// What the running desktop's setting names.
    fn setting(&self) -> Result<Setting, EdgeError> {
        match self.desktop {
            Some(Desktop::Gnome) => gnome_setting("org.gnome.desktop.background", true),
            Some(Desktop::Budgie) => gnome_setting("org.gnome.desktop.background", false),
            Some(Desktop::Cinnamon) => gnome_setting("org.cinnamon.desktop.background", false),
            Some(Desktop::Mate) => mate_setting(),
            Some(Desktop::Plasma) => plasma_setting(),
            Some(Desktop::Xfce) => xfce_setting(),
            None => Err(unexpected(format!(
                "the desktop picture of {:?} is not read: this app reads GNOME's, Budgie's, Cinnamon's, MATE's, KDE Plasma's and Xfce's",
                self.current_desktop
            ))),
        }
    }
}

impl PcWallpaperSource for DesktopWallpaper {
    fn current_wallpaper(&self) -> Result<PcWallpaper, EdgeError> {
        match self.setting()? {
            Setting::NoPicture => Ok(PcWallpaper::NotSet),
            Setting::Picture(path) if self.desktop == Some(Desktop::Plasma) && path.is_dir() => {
                Ok(match package_picture(&path, plasma_dark()) {
                    Some(picture) => picture_of(&picture),
                    None => {
                        warn!(folder = %path.display(), "the Plasma wallpaper package has no picture this app can choose");
                        PcWallpaper::Unreadable
                    }
                })
            }
            Setting::Picture(path) => Ok(picture_of(&path)),
        }
    }
}
