//! A message's attachments: their kind and the file name each is opened or saved under.

use phonelink_messaging::edge::FileFamily;

use crate::l10n::translated;

/// What an attachment is, by its family: the view's icon and title.
pub fn attachment_kind(family: FileFamily) -> &'static str {
    match family {
        FileFamily::Image => "image",
        FileFamily::Video => "video",
        FileFamily::Audio => "audio",
        FileFamily::VCard => "contact",
        FileFamily::Text => "text",
        FileFamily::Document | FileFamily::Other => "other",
    }
}

/// A MIME type without its parameters, lowercased.
pub fn bare_content_type(content_type: &str) -> String {
    content_type.split(';').next().unwrap_or_default().trim().to_lowercase()
}

/// The file name an attachment is opened or saved under.
///
/// It is its own name without any folder, else "Attachment", with `extension` (leading dot included)
/// when the name has none.
pub fn part_file_name(name: &str, extension: Option<&str>) -> String {
    let base: String =
        name.rsplit(['/', '\\']).next().unwrap_or_default().chars().filter(|c| !c.is_control()).collect();
    let base = if base.trim().is_empty() || base == "." || base == ".." { translated("Attachment") } else { base };
    let has_extension = base.rsplit_once('.').is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty());
    match extension {
        Some(extension) if !has_extension => format!("{base}{extension}"),
        _ => base,
    }
}
