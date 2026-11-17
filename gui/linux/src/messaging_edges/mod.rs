//! The messaging feature's image and file-type edges on Linux: fitting image attachments to the
//! carrier's limits, and MIME types' families and file name extensions.
//!
//! Images are read and written with the `image` crate: PNG, JPEG, GIF (every frame), WebP, TIFF and
//! BMP are decoded; PNG, JPEG, GIF, TIFF and BMP are written. Other image formats (HEIC, AVIF) are
//! recognized but not transformed, so the engine sends them as they are.
//!
//! An image keeps its ICC profile in the formats that carry one (JPEG, PNG and TIFF), except an image
//! with alpha written as JPEG: it is drawn over white in sRGB and carries no profile.
//!
//! MIME types come from the shared MIME-info database (freedesktop.org): its types, aliases and
//! sub-class relations under `mime/` of `$XDG_DATA_HOME` and each `$XDG_DATA_DIRS` entry, the file name
//! globs (`globs2`) and each type's own definition (`<media>/<subtype>.xml`), whose first glob is its
//! preferred extension. Where macOS answers otherwise for a common attachment type, its answer is
//! taken (`file_types::mac_answers`); a type the database neither knows nor aliases is other. Without the
//! database a type's family follows its top-level media type only and no other extension is known;
//! that is logged once when it is loaded.

mod attachment_imaging;
mod file_types;

pub use attachment_imaging::ImageCrateImaging;
pub use file_types::file_types;
