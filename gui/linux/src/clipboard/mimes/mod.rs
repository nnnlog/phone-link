//! What the clipboard holds by MIME type, as the engine's types (UTIs) name it: which MIME types
//! stand for which type, the text encodings of the X11 targets, and copied files.

use std::collections::HashMap;
use std::sync::Arc;

use phonelink_clipboard::pasteboard::{ClipboardPasteboardData, uti};
use phonelink_engine::edge::EdgeError;

#[cfg(test)]
mod tests;

/// The X11 `STRING` target, which holds ISO Latin-1 text (ICCCM); the others hold UTF-8.
const LATIN1_TEXT: &str = "STRING";
const TEXT_MIMES: [&str; 4] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", LATIN1_TEXT];
pub(super) const URI_LIST: &str = "text/uri-list";
/// KDE's convention (also set by `KeePassXC`) for clipboard content that history must not keep.
const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";
const PASSWORD_HINT_VALUE: &[u8] = b"secret";

/// The MIME types that stand for `uti_name`, the preferred first.
fn mimes_for(uti_name: &str) -> &'static [&'static str] {
    match uti_name {
        uti::PLAIN_TEXT => &TEXT_MIMES,
        uti::HTML => &["text/html"],
        uti::PNG => &["image/png"],
        uti::TIFF => &["image/tiff"],
        uti::FILE_URL => &[URI_LIST],
        uti::CONCEALED | uti::TRANSIENT => &[PASSWORD_HINT_MIME],
        _ => &[],
    }
}

/// The types the engine reads from the clipboard, in the order it prefers them.
const READABLE_UTIS: [&str; 6] = [uti::PNG, uti::TIFF, uti::HTML, uti::PLAIN_TEXT, uti::FILE_URL, uti::CONCEALED];

/// UTF-8 text as Latin-1, when every character of it is in Latin-1.
fn latin1(utf8: &[u8]) -> Option<Vec<u8>> {
    std::str::from_utf8(utf8).ok()?.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect()
}

/// Text read as `mime`, in UTF-8: `STRING`'s Latin-1 converted, the others as they came.
pub(super) fn utf8_text(mime: &str, bytes: Vec<u8>) -> Vec<u8> {
    if mime == LATIN1_TEXT {
        bytes.iter().map(|byte| char::from(*byte)).collect::<String>().into_bytes()
    } else {
        bytes
    }
}

/// The data a write offers, by MIME type, from `representations` in order, and why the write stopped
/// before its end: a type the clipboard has no MIME type for. Text is offered as `STRING` only when it
/// is all Latin-1.
pub(super) fn mime_data(
    representations: Vec<ClipboardPasteboardData>,
) -> (HashMap<String, Arc<Vec<u8>>>, Option<String>) {
    let mut data: HashMap<String, Arc<Vec<u8>>> = HashMap::new();
    for representation in representations {
        let mimes = mimes_for(&representation.uti);
        if mimes.is_empty() {
            return (data, Some(format!("the clipboard did not take {}", representation.uti)));
        }
        let bytes =
            Arc::new(if mimes == [PASSWORD_HINT_MIME] { PASSWORD_HINT_VALUE.to_vec() } else { representation.data });
        for mime in mimes {
            let bytes = match *mime {
                LATIN1_TEXT => match latin1(&bytes) {
                    Some(latin1) => Arc::new(latin1),
                    None => continue,
                },
                _ => bytes.clone(),
            };
            data.entry((*mime).to_owned()).or_insert(bytes);
        }
    }

    (data, None)
}

/// The first MIME type of `offered` that stands for `uti_name`, in the order the clipboard prefers.
pub(super) fn first_offered<'a>(uti_name: &str, offered: &'a [String]) -> Option<&'a str> {
    mimes_for(uti_name).iter().find_map(|m| offered.iter().find(|o| o == m).map(String::as_str))
}

/// The types a clipboard that offers `mimes` holds, in the order the engine reads them.
fn utis_of(mimes: &[String]) -> Vec<String> {
    READABLE_UTIS
        .into_iter()
        .filter(|u| mimes_for(u).iter().any(|m| mimes.iter().any(|offered| offered == m)))
        .map(str::to_owned)
        .collect()
}

/// A `text/uri-list` (RFC 2483: a URI a line, `#` lines are comments) names a local file: a `file:`
/// URI. A browser offers copied web links as a list too.
fn holds_file_uri(list: &[u8]) -> bool {
    String::from_utf8_lossy(list)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .any(|line| line.get(..5).is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:")))
}

/// The types of a clipboard that offers `mimes`: copied files only when its URI list, read by
/// `uri_list`, names one.
pub(super) fn utis_with_files(
    mimes: &[String],
    uri_list: impl FnOnce() -> Result<Option<Vec<u8>>, EdgeError>,
) -> Result<Vec<String>, EdgeError> {
    let mut utis = utis_of(mimes);
    if let Some(at) = utis.iter().position(|u| u == uti::FILE_URL)
        && !uri_list()?.is_some_and(|list| holds_file_uri(&list))
    {
        utis.remove(at);
    }
    Ok(utis)
}
