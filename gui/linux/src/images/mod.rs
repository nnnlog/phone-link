//! The app's cache of picture files, under `$XDG_CACHE_HOME/phonelink`: the pictures the window shows
//! by `file://` URL, named by key or by content, QR codes, and the URLs and paths between them.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tracing::warn;

/// `$XDG_CACHE_HOME/phonelink` (or `~/.cache/phonelink`): `images/` (pictures the window shows, and
/// message attachments opened in other applications), `photo-files/` (photos handed to other
/// applications) and `drag/` (files dragged out of a mirrored app).
pub fn cache_dir() -> PathBuf {
    cache_dir_in(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
}

/// [`cache_dir`] for these values of `XDG_CACHE_HOME` and `HOME`. A relative path in either is ignored,
/// as the XDG base directory rules say; with neither, the folder is in the temporary folder.
fn cache_dir_in(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    xdg_cache_home
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(".cache")).filter(|dir| dir.is_absolute()))
        .unwrap_or_else(std::env::temp_dir)
        .join("phonelink")
}

/// The 64-bit FNV-1a hash of `bytes`: a stable name for a cached file made from them.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3))
}

/// A `file://` URL for `path`, percent-encoding every byte but unreserved characters and `/`, so names
/// with spaces, `#`, `%` or `?` stay one path.
pub fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => url.push(char::from(*byte)),
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }

    url
}

/// The local path of a `file://` URL as QML's `url.toString()` writes it: Qt leaves characters such as
/// `%`, `#`, `?`, `[` and `]` percent-encoded there, so the path is decoded. `None` for another scheme.
pub fn local_path(url: &str) -> Option<String> {
    let path = url.strip_prefix("file://")?;
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex =
            bytes.get(index + 1..index + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[index], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    Some(String::from_utf8_lossy(&decoded).into_owned())
}

/// `key` as a file name: every character but ASCII letters, digits, `.` and `-` turned to `_`, and a
/// name of dots alone too.
fn safe_name(key: &str) -> String {
    let name: String =
        key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    if name.chars().all(|c| c == '.') { name.replace('.', "_") } else { name }
}

/// The cached file for `key` in `kind`, written from `load` the first time it is asked for.
pub fn cached(kind: &str, key: &str, load: impl FnOnce() -> Option<Vec<u8>>) -> Option<String> {
    let path = cache_dir().join("images").join(kind).join(safe_name(key));
    if path.exists() {
        return Some(file_url(&path));
    }
    let bytes = load()?;
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(file_url(&path))
}

/// A file named by the content's hash, for images that change under one key.
pub fn by_content(kind: &str, bytes: &[u8]) -> Option<String> {
    cached(kind, &format!("{:016x}", fnv1a(bytes)), || Some(bytes.to_vec()))
}

/// A PNG file of `text` as a QR code, cached by content; `None` when the text does not fit a QR code.
pub fn qr_code(text: &str) -> Option<String> {
    by_content("qr", &qr_png(text)?)
}

/// `text` as a QR code at error correction level L, the least redundant, so it has the fewest modules:
/// black modules on white, 8 pixels each, with the standard quiet zone, as PNG.
fn qr_png(text: &str) -> Option<Vec<u8>> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
        .inspect_err(|e| warn!(text_bytes = text.len(), error = %e, "no QR code for the text"))
        .ok()?;
    let image = code.render::<image::Luma<u8>>().module_dimensions(8, 8).build();
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .inspect_err(|e| warn!(error = %e, "the QR code was not encoded"))
        .ok()?;
    Some(png.into_inner())
}

/// Forgets the images of `kind` (the phone changed them: a contact's photo, an app's icon).
pub fn clear_kind(kind: &str) {
    remove(&cache_dir().join("images").join(kind));
}

/// Forgets every picture and opened attachment (`images/`) and every file dragged out of a mirrored app
/// (`drag/`): the phone or the account changed, or the user signed out.
pub fn clear_all() {
    remove(&cache_dir().join("images"));
    remove(&cache_dir().join("drag"));
}

/// Removes `dir` and what it holds; a folder that is not there is already removed.
fn remove(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => warn!(folder = %dir.display(), error = %e, "a cache folder was not removed"),
    }
}

#[cfg(test)]
mod tests;
