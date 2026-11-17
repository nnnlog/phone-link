//! KDE Plasma's picture: the wallpaper its shell names for the primary screen over D-Bus, the default
//! picture of its look-and-feel package or theme, and the picture of a wallpaper package that fits the
//! phone and Plasma's light or dark colors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use phonelink_engine::edge::EdgeError;
use phonelink_wallpaper::edge::MAX_PICTURE_HEIGHT;
use tracing::warn;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedValue;

use super::Setting;
use crate::edges::unexpected;

#[cfg(test)]
mod tests;

/// The wallpaper Plasma's theme shows when the look-and-feel package names none.
const PLASMA_THEME_WALLPAPER: &str = "Next";
/// The look-and-feel package Plasma uses when `kdeglobals` names none.
const PLASMA_LOOK_AND_FEEL: &str = "org.kde.breeze.desktop";

/// The text value of `key` in the wallpaper's settings; `None` without it.
fn config_string(config: &HashMap<String, OwnedValue>, key: &str) -> Result<Option<String>, EdgeError> {
    config
        .get(key)
        .map(|value| {
            String::try_from(&**value).map_err(|e| unexpected(format!("Plasma's wallpaper {key} is not text: {e}")))
        })
        .transpose()
}

/// Plasma's picture on its primary screen (screen 0), from `org.kde.PlasmaShell.wallpaper`: the image
/// wallpaper's `Image`, or the default picture when it names none; none for a plain color. Other
/// wallpaper plugins (a slideshow, a picture of the day) name no single file.
pub(super) fn plasma_setting() -> Result<Setting, EdgeError> {
    let call = || -> zbus::Result<HashMap<String, OwnedValue>> {
        let bus = Connection::session()?;
        let shell = Proxy::new(&bus, "org.kde.plasmashell", "/PlasmaShell", "org.kde.PlasmaShell")?;
        shell.call("wallpaper", &(0u32,))
    };
    let config = call().map_err(|e| unexpected(format!("Plasma's wallpaper was not read: {e}")))?;
    let plugin = config_string(&config, "wallpaperPlugin")?.unwrap_or_default();

    match plugin.as_str() {
        // No `Image` (or an empty one) is the default picture.
        "org.kde.image" => match config_string(&config, "Image")?.filter(|image| !image.is_empty()) {
            Some(image) => Ok(Setting::from_uri(&image)),
            None => plasma_default_picture(
                &xdg_dirs("XDG_CONFIG_HOME", ".config", "XDG_CONFIG_DIRS", "/etc/xdg"),
                &data_dirs(),
            )
            .map(Setting::Picture)
            .ok_or_else(|| {
                unexpected(
                    "Plasma shows a default picture that neither its look-and-feel package nor its theme names here",
                )
            }),
        },
        "org.kde.color" => Ok(Setting::NoPicture),
        other => Err(unexpected(format!("Plasma's wallpaper plugin {other:?} names no single picture"))),
    }
}

/// An XDG base directory list, most important first: the user's (`$<home_var>`, else `~/<home_default>`),
/// then the system's (`$<system_var>`, else `system_default`).
fn xdg_dirs(home_var: &str, home_default: &str, system_var: &str, system_default: &str) -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    let home =
        env(home_var).map(PathBuf::from).or_else(|| env("HOME").map(|home| PathBuf::from(home).join(home_default)));
    let system: Vec<PathBuf> = match env(system_var) {
        Some(dirs) => std::env::split_paths(&dirs).collect(),
        None => std::env::split_paths(system_default).collect(),
    };
    home.into_iter().chain(system).collect()
}

/// The XDG data folders, most important first.
fn data_dirs() -> Vec<PathBuf> {
    xdg_dirs("XDG_DATA_HOME", ".local/share", "XDG_DATA_DIRS", "/usr/local/share:/usr/share")
}

/// The value of `key` in the `[group]` of a `KConfig` file's text.
fn kconfig_value<'a>(text: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let header = format!("[{group}]");
    let mut in_group = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == header;
        } else if in_group
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim());
        }
    }

    None
}

/// The picture Plasma's image wallpaper shows when it names none, found as Plasma finds it: the `Image`
/// of `[Wallpaper]` in the `defaults` of the look-and-feel package (`kdeglobals`' `[KDE]`
/// `LookAndFeelPackage`, Breeze when none), a wallpaper package under `wallpapers/` of the data folders
/// or at a path; else the wallpaper of Plasma's theme. `None` when no such package is installed.
fn plasma_default_picture(config_dirs: &[PathBuf], data_dirs: &[PathBuf]) -> Option<PathBuf> {
    let look_and_feel = config_dirs
        .iter()
        .find_map(|dir| {
            let kdeglobals = std::fs::read_to_string(dir.join("kdeglobals")).ok()?;
            kconfig_value(&kdeglobals, "KDE", "LookAndFeelPackage").filter(|name| !name.is_empty()).map(str::to_owned)
        })
        .unwrap_or_else(|| PLASMA_LOOK_AND_FEEL.to_owned());
    let defaults = data_dirs.iter().find_map(|dir| {
        std::fs::read_to_string(dir.join("plasma/look-and-feel").join(&look_and_feel).join("contents/defaults")).ok()
    });
    let package = |name: &str| -> Option<PathBuf> {
        let candidates: Vec<PathBuf> = if name.starts_with('/') {
            vec![PathBuf::from(name)]
        } else {
            data_dirs.iter().map(|dir| dir.join("wallpapers").join(name)).collect()
        };
        candidates.into_iter().find(|folder| folder.join("contents/images").is_dir())
    };
    defaults
        .as_deref()
        .and_then(|defaults| kconfig_value(defaults, "Wallpaper", "Image"))
        .filter(|image| !image.is_empty())
        .and_then(package)
        .or_else(|| package(PLASMA_THEME_WALLPAPER))
}

/// The width and height of a wallpaper package's picture, from its file name (`1920x1080.png`).
fn package_size(name: &str) -> Option<(u32, u32)> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let (width, height) = stem.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// The picture of a package's `sizes` that is sent: a landscape one if there is one; of those, the
/// smallest at least as tall as the phone's picture, else the largest.
pub(super) fn package_choice(sizes: &[(u32, u32)]) -> Option<(u32, u32)> {
    let landscape: Vec<(u32, u32)> = sizes.iter().copied().filter(|(w, h)| w >= h).collect();
    let candidates = if landscape.is_empty() { sizes.to_vec() } else { landscape };
    let area = |(w, h): &(u32, u32)| u64::from(*w) * u64::from(*h);
    candidates
        .iter()
        .filter(|(_, h)| *h >= MAX_PICTURE_HEIGHT)
        .min_by_key(|size| area(size))
        .or_else(|| candidates.iter().max_by_key(|size| area(size)))
        .copied()
}

/// The picture of the Plasma wallpaper package at `folder`: from `contents/images_dark` while Plasma's
/// colors are dark and the package has dark pictures, else from `contents/images`.
pub(super) fn package_picture(folder: &Path, dark: bool) -> Option<PathBuf> {
    let dark_images = folder.join("contents").join("images_dark");
    let images = if dark && dark_images.is_dir() { dark_images } else { folder.join("contents").join("images") };
    let pictures: Vec<((u32, u32), PathBuf)> = std::fs::read_dir(&images)
        .inspect_err(|e| warn!(folder = %images.display(), error = %e, "a Plasma wallpaper package was not read"))
        .ok()?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            Some((package_size(path.file_name()?.to_str()?)?, path))
        })
        .collect();
    let sizes: Vec<(u32, u32)> = pictures.iter().map(|(size, _)| *size).collect();
    let chosen = package_choice(&sizes)?;
    pictures.into_iter().find(|(size, _)| *size == chosen).map(|(_, path)| path)
}

/// Whether Plasma's window color, in `kdeglobals` text, is dark, as Plasma decides which pictures of a
/// package to show (a gray below 192). The default Breeze colors are light.
fn kdeglobals_dark(kdeglobals: &str) -> bool {
    let Some(value) = kconfig_value(kdeglobals, "Colors:Window", "BackgroundNormal") else { return false };
    let channels: Vec<u32> = value.split(',').filter_map(|c| c.trim().parse().ok()).collect();

    match channels[..] {
        [r, g, b, ..] => (r * 11 + g * 16 + b * 5) / 32 < 192,
        _ => false,
    }
}

/// Plasma's colors are dark, by the user's `kdeglobals`.
pub(super) fn plasma_dark() -> bool {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let Some(path) = config.map(|dir| dir.join("kdeglobals")) else { return false };

    match std::fs::read_to_string(&path) {
        Ok(text) => kdeglobals_dark(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Plasma's colors were not read; the light pictures are sent");
            false
        }
    }
}
