//! What macOS answers for the common attachment types where the shared MIME-info database answers
//! otherwise, the aliases macOS declares, and the types macOS takes for documents.

use phonelink_messaging::edge::FileFamily;

#[cfg(test)]
mod tests;

/// What macOS answers for a type (`UTType(mimeType:)`): its family and preferred extension.
pub(super) struct MacAnswer {
    pub(super) family: FileFamily,
    pub(super) extension: Option<&'static str>,
}

const fn answer(family: FileFamily, extension: &'static str) -> MacAnswer {
    MacAnswer { family, extension: Some(extension) }
}

/// A type macOS does not declare: other, without an extension.
const UNDECLARED: MacAnswer = MacAnswer { family: FileFamily::Other, extension: None };

/// macOS's answers for the common attachment types whose family or extension the shared MIME-info
/// database gives otherwise, read with `UTType(mimeType:)` on macOS 27. A type is looked up as it is
/// named, then as the type it is an alias of; an alias macOS takes for another type than the database
/// does is listed itself.
const MAC_ANSWERS: [(&str, MacAnswer); 48] = [
    ("image/jpeg", answer(FileFamily::Image, ".jpeg")),
    ("image/tiff", answer(FileFamily::Image, ".tiff")),
    ("image/heif", answer(FileFamily::Image, ".heif")),
    ("image/heic", answer(FileFamily::Image, ".heic")),
    ("image/heic-sequence", answer(FileFamily::Image, ".heics")),
    ("image/heif-sequence", answer(FileFamily::Image, ".heifs")),
    ("video/quicktime", answer(FileFamily::Video, ".mov")),
    ("video/mpeg", answer(FileFamily::Video, ".mpg")),
    ("video/x-m4v", answer(FileFamily::Video, ".m4v")),
    ("audio/mp4", answer(FileFamily::Audio, ".mp4")),
    ("audio/x-m4a", answer(FileFamily::Audio, ".m4a")),
    ("audio/midi", answer(FileFamily::Audio, ".midi")),
    ("audio/ogg", answer(FileFamily::Audio, ".ogg")),
    ("audio/webm", answer(FileFamily::Video, ".webm")),
    ("text/vcard", answer(FileFamily::VCard, ".vcf")),
    ("text/calendar", answer(FileFamily::Text, ".ics")),
    ("text/x-vcalendar", answer(FileFamily::Text, ".vcs")),
    ("text/rtf", answer(FileFamily::Text, ".rtf")),
    ("application/vnd.ms-powerpoint", answer(FileFamily::Document, ".ppt")),
    ("application/x-iwork-keynote-sffkey", answer(FileFamily::Document, ".key")),
    ("application/x-iwork-numbers-sffnumbers", answer(FileFamily::Document, ".numbers")),
    ("application/x-iwork-pages-sffpages", answer(FileFamily::Other, ".pages")),
    ("image/pjpeg", UNDECLARED),
    ("image/x-bmp", UNDECLARED),
    ("image/x-ms-bmp", UNDECLARED),
    ("image/x-icon", UNDECLARED),
    ("image/vnd.wap.wbmp", UNDECLARED),
    ("video/3gp", UNDECLARED),
    ("video/mp2t", UNDECLARED),
    ("video/x-matroska", UNDECLARED),
    ("audio/amr-wb", UNDECLARED),
    ("audio/imelody", UNDECLARED),
    ("audio/x-imelody", UNDECLARED),
    ("audio/m4a", UNDECLARED),
    ("audio/x-flac", UNDECLARED),
    ("audio/x-matroska", UNDECLARED),
    ("audio/x-ms-wma", UNDECLARED),
    ("audio/mobile-xmf", UNDECLARED),
    ("application/rtf", UNDECLARED),
    ("application/smil+xml", UNDECLARED),
    ("application/ogg", UNDECLARED),
    ("application/ics", UNDECLARED),
    ("application/x-pdf", UNDECLARED),
    ("application/vnd.ms-word", UNDECLARED),
    ("application/x-msword", UNDECLARED),
    ("application/vnd.apple.keynote", UNDECLARED),
    ("application/vnd.apple.numbers", UNDECLARED),
    ("application/vnd.apple.pages", UNDECLARED),
];

/// Aliases macOS declares and the database does not: they name the type they alias.
pub(super) const MAC_ALIASES: [(&str, &str); 8] = [
    ("image/jpg", "image/jpeg"),
    ("audio/mpeg3", "audio/mpeg"),
    ("audio/x-mpeg3", "audio/mpeg"),
    ("audio/mpg", "audio/mpeg"),
    ("audio/wave", "audio/vnd.wave"),
    ("audio/aiff", "audio/x-aiff"),
    ("video/mpg", "video/mpeg"),
    ("video/x-mpg", "video/mpeg"),
];

/// What the messaging feature calls documents: the types macOS takes for a PDF, a presentation, a
/// spreadsheet or a word-processing document (Word's and `OpenDocument`'s text), as named. `OpenDocument`
/// presentations and spreadsheets are none of these on macOS, and an RTF file is text.
pub(super) const DOCUMENT_TYPES: [&str; 28] = [
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.ms-powerpoint",
    "application/mspowerpoint",
    "application/vnd.ms-powerpoint.presentation.macroenabled.12",
    "application/vnd.ms-powerpoint.slideshow.macroenabled.12",
    "application/vnd.ms-powerpoint.template.macroenabled.12",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
    "application/vnd.openxmlformats-officedocument.presentationml.template",
    "application/vnd.oasis.opendocument.presentation-template",
    "application/vnd.stardivision.impress",
    "application/vnd.sun.xml.impress",
    "application/vnd.sun.xml.impress.template",
    "application/x-iwork-keynote-sffkey",
    "application/vnd.ms-excel",
    "application/msexcel",
    "application/vnd.ms-excel.sheet.macroenabled.12",
    "application/vnd.ms-excel.template.macroenabled.12",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
    "application/vnd.oasis.opendocument.spreadsheet-template",
    "application/vnd.stardivision.calc",
    "application/vnd.sun.xml.calc",
    "application/vnd.sun.xml.calc.template",
    "application/x-iwork-numbers-sffnumbers",
];

/// macOS's answer for `mime` (lowercase), when it is one of [`MAC_ANSWERS`].
pub(super) fn mac_answer(mime: &str) -> Option<&'static MacAnswer> {
    MAC_ANSWERS.iter().find(|(name, _)| *name == mime).map(|(_, answer)| answer)
}
