//! The photo files Phone Link hands to other applications (open, open with, copy, drag out) and the
//! files the user saves.
//!
//! Handed-on files live in `$XDG_CACHE_HOME/phonelink/photo-files`: a photo's under its id, named by
//! its save-as name — a later action replaces it, so an application still using it keeps a valid path —
//! and a clicked new-photo notification's under `new-photos`. A photo's files go when it leaves the
//! gallery or changes, and every file goes when the app quits; a run that did not quit cleanly leaves
//! them behind, so the first write of a run removes them.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Once;

use phonelink_engine::api::photos::PhotoFileName;
use tracing::{error, info, warn};

use crate::l10n::{fill, translate};
use crate::{images, user_text};

/// Where this run's handed-on files are.
fn folder() -> PathBuf {
    images::cache_dir().join("photo-files")
}

/// Removes every handed-on file (the app quits).
pub(super) fn remove_all() {
    remove(&folder());
}

/// Removes the files of photo `id` (it left the gallery, or changed).
pub(super) fn remove_photo(id: i64) {
    remove(&folder().join(id.to_string()));
}

fn remove(path: &Path) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(folder = %path.display(), error = %e, "a folder of handed-on photo files was not removed"),
    }
}

/// The folder for a file of this run: the first one removes what earlier runs left.
fn run_folder(sub: &str) -> Result<PathBuf, String> {
    static EARLIER_RUNS: Once = Once::new();
    EARLIER_RUNS.call_once(remove_all);

    let dir = folder().join(sub);
    std::fs::create_dir_all(&dir).map_err(|e| not_written(&dir, &e))?;
    Ok(dir)
}

/// What the window shows for a file or folder that was not written: the OS's reason is logged, as it is
/// no sentence for the window.
fn not_written(path: &Path, error: &std::io::Error) -> String {
    warn!(path = %path.display(), error = %error, "a photo file was not written");
    user_text::something_went_wrong()
}

/// The save-as name as one file name, or why it can't be one (in the words the window shows). The
/// engine takes the stem and extension from the phone's file name, so they hold no path separator; a
/// name that is not a single file name is refused.
pub(super) fn file_name(name: &PhotoFileName) -> Result<String, String> {
    let file_name = format!("{}{}", name.stem, name.extension);
    if file_name.is_empty() || file_name == "." || file_name == ".." || file_name.contains(['/', '\0']) {
        return Err(fill(translate("The photo's name \u{201c}%@\u{201d} can't be used as a file name."), &[file_name]));
    }
    Ok(file_name)
}

/// The save-as name for `number` 1, and for a later number the name with " <number>" before the
/// extension (`IMG_0001 2.jpg`).
fn numbered_file_name(name: &PhotoFileName, number: u32) -> Result<String, String> {
    if number <= 1 {
        return file_name(name);
    }
    file_name(&PhotoFileName { stem: format!("{} {number}", name.stem), extension: name.extension.clone() })
}

/// Writes `data` into `folder` under the first of the photo's numbered names the folder doesn't hold;
/// never over an existing file.
pub(super) fn write_numbered(data: &[u8], name: &PhotoFileName, folder: &Path) -> Result<PathBuf, String> {
    for number in 1.. {
        let path = folder.join(numbered_file_name(name, number)?);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                return file.write_all(data).map(|()| path.clone()).map_err(|e| {
                    let _ = std::fs::remove_file(&path);
                    not_written(&path, &e)
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(not_written(&path, &e)),
        }
    }
    unreachable!("the numbers run out only after every name was taken")
}

/// Writes `data` to `path` through a temporary file beside it, so the file is never half written.
pub(super) fn write_atomically(path: &Path, data: &[u8]) -> Result<(), String> {
    let Some(file_name) = path.file_name() else {
        error!(path = %path.display(), "a photo file's path names no file");
        return Err(user_text::something_went_wrong());
    };

    let temporary = path.with_file_name(format!(".{}.phonelink-{}", file_name.to_string_lossy(), std::process::id()));
    std::fs::write(&temporary, data).map_err(|e| not_written(&temporary, &e))?;

    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        not_written(path, &e)
    })
}

/// Writes photo `id`'s full image for an application under its save-as name.
pub(super) fn write_photo(id: i64, file_name: &str, data: &[u8]) -> Result<PathBuf, String> {
    let dir = run_folder(&id.to_string())?;
    let path = dir.join(file_name);
    write_atomically(&path, data)?;
    Ok(path)
}

/// The image format of `bytes`, by their signature, as a file extension: the formats a new-photo
/// notification's photo comes in. `None` for bytes that are no such image (SVG included).
fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    // ISO base media files (HEIF, AVIF) name their brand after `ftyp`.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return match &bytes[8..12] {
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" => Some("heic"),
            b"mif1" | b"msf1" => Some("heif"),
            b"avif" | b"avis" => Some("avif"),
            _ => None,
        };
    }

    let format = image::guess_format(bytes).ok()?;
    match format {
        image::ImageFormat::Png => Some("png"),
        image::ImageFormat::Jpeg => Some("jpg"),
        image::ImageFormat::Gif => Some("gif"),
        image::ImageFormat::WebP => Some("webp"),
        image::ImageFormat::Tiff => Some("tiff"),
        image::ImageFormat::Bmp => Some("bmp"),
        other => {
            info!(format = ?other, "a new photo's bytes are in an image format that is not opened");
            None
        }
    }
}

/// Writes a clicked new-photo notification's image, named after its media id and image format;
/// `None`, writing nothing, when the bytes are no image.
pub(super) fn write_new_photo(media_id: i64, bytes: &[u8]) -> Result<Option<PathBuf>, String> {
    let Some(extension) = image_extension(bytes) else { return Ok(None) };
    let dir = run_folder("new-photos")?;
    let path = dir.join(format!("{media_id}.{extension}"));
    write_atomically(&path, bytes)?;
    Ok(Some(path))
}

/// Why `xdg-open` did not open a file or URI.
#[derive(Debug, thiserror::Error)]
pub(super) enum OpenError {
    #[error("xdg-open did not run: {0}")]
    NotRun(#[source] std::io::Error),
    /// The launcher ran but found no application that opened it.
    #[error("xdg-open ended with {0}")]
    Ended(ExitStatus),
}

/// Opens `target` (a path or a URI) with the application the desktop chooses for it (`xdg-open`),
/// waiting for the launcher; `Err` when none opened it.
pub(super) fn open_externally(target: &str) -> Result<(), OpenError> {
    let status = std::process::Command::new("xdg-open")
        .arg(target)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(OpenError::NotRun)?;
    if status.success() { Ok(()) } else { Err(OpenError::Ended(status)) }
}

#[cfg(test)]
mod tests;
