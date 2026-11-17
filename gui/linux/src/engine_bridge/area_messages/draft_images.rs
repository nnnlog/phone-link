//! Images read for a draft.
//!
//! They come from picked or dropped files, dropped or pasted image data, and the clipboard.

use std::path::{Path, PathBuf};

use phonelink_clipboard::pasteboard::{ClipboardPasteboard, uti};
use phonelink_core::edge::EdgeError;
use phonelink_messaging::edge::FileFamily;

use super::draft::DraftImage;
use crate::images;
use crate::messaging_edges::file_types;

/// Why images were not read for a draft.
#[derive(Debug, thiserror::Error)]
pub(super) enum DraftImageError {
    /// A picked, dropped or pasted file was not read.
    #[error("{}: {source}", .path.display())]
    Read { path: PathBuf, source: std::io::Error },
    /// Pasted or dropped image data was not written to the cache, where its thumbnail shows from.
    #[error("the image was not stored")]
    NotStored,
    /// A picked or dropped URL names no local file.
    #[error("{url} is not a file")]
    NotAFile { url: String },
    /// Dropped image data is not base64.
    #[error("the dropped image does not decode: {0}")]
    Undecodable(#[source] base64::DecodeError),
    /// The clipboard was not read.
    #[error("{0}")]
    Clipboard(#[from] EdgeError),
}

/// The images among the clipboard's file URLs (a `text/uri-list`).
pub(super) fn image_paths(uri_list: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(uri_list)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(images::local_path)
        .map(PathBuf::from)
        .filter(|path| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            file_types()
                .database()
                .type_of_file_name(&name)
                .is_some_and(|t| file_types().database().family_of(&t) == FileFamily::Image)
        })
        .collect()
}

/// A picked, dropped or pasted file read as a draft image (without its id).
///
/// A file whose name says it is no image is not read: the draft refuses it.
pub(super) fn read_image_file(path: &Path) -> Result<DraftImage, DraftImageError> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let content_type = file_types().database().type_of_file_name(&name);
    let mut image = DraftImage { id: 0, name, content_type, bytes: Vec::new(), url: images::file_url(path) };
    if image.is_image() {
        image.bytes = std::fs::read(path).map_err(|source| DraftImageError::Read { path: path.to_owned(), source })?;
    }
    Ok(image)
}

/// Image data (pasted or dropped) as a draft image, its file written to the cache for the thumbnail.
pub(super) fn data_image(bytes: Vec<u8>, content_type: Option<String>) -> Result<DraftImage, DraftImageError> {
    let url = images::by_content("drafts", &bytes).ok_or(DraftImageError::NotStored)?;
    Ok(DraftImage { id: 0, name: String::new(), content_type, bytes, url })
}

/// The clipboard's images for the composer.
///
/// They are copied image files, else image data when the clipboard holds no text; `None` when it
/// holds neither (Paste then pastes text).
pub(super) fn clipboard_images(
    pasteboard: &dyn ClipboardPasteboard,
) -> Result<Option<Vec<DraftImage>>, DraftImageError> {
    let types = pasteboard.types()?;
    let has = |name: &str| types.iter().any(|t| t == name);
    if has(uti::FILE_URL) {
        let list = pasteboard.data(uti::FILE_URL.to_owned())?.unwrap_or_default();
        let paths = image_paths(&list);
        if !paths.is_empty() {
            return paths.iter().map(|p| read_image_file(p)).collect::<Result<Vec<_>, _>>().map(Some);
        }
    }
    if has(uti::PLAIN_TEXT) {
        return Ok(None);
    }

    for (name, mime) in [(uti::PNG, "image/png"), (uti::TIFF, "image/tiff")] {
        if has(name)
            && let Some(bytes) = pasteboard.data(name.to_owned())?
        {
            return data_image(bytes, Some(mime.to_owned())).map(|image| Some(vec![image]));
        }
    }
    Ok(None)
}
