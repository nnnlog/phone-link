//! The freedesktop.org databases that say which application opens what: the association between MIME
//! types and applications (`mimeapps.list` and the desktop entries' `MimeType=` key, after the
//! "Association between MIME types and applications" specification), a file name's MIME type
//! (shared-mime-info's `globs2`), and starting a desktop entry on a file (the Desktop Entry
//! specification's `Exec` key).
//!
//! Every lookup reads the files when it runs, so applications installed or removed meanwhile count.

mod associations;
mod entry;
mod exec;
mod key_file;
mod mime_types;
mod xdg;

pub use associations::Applications;
pub use entry::DesktopEntry;
pub use exec::{LaunchError, launch};
pub use mime_types::{is_media, mime_type_of_name, uri_mime_type};
pub use xdg::Xdg;

#[cfg(test)]
mod tests;
