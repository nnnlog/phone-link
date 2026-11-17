//! Open With for the Photos section: the applications that open a photo's type, the default first,
//! and starting one of them on the photo.

use std::path::Path;

use tracing::warn;

use super::NameCollator;
use super::desktop::{Applications, DesktopEntry, LaunchError, Xdg, launch};

/// Why the chosen application did not open a photo.
#[derive(Debug, thiserror::Error)]
pub enum OpenWithError {
    #[error("no application {0} is installed")]
    NotInstalled(String),
    #[error("{0}")]
    Launch(#[from] LaunchError),
}

/// An application that opens a photo's type, for the Open With menu.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenWithApp {
    /// Its desktop file ID.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// The applications that open `mime`, as Open With lists them: the default first, then the others by
/// name in the user's language.
pub fn open_with_apps(mime: &str, collator: &NameCollator) -> Vec<OpenWithApp> {
    let xdg = Xdg::from_env();
    let apps = Applications::scan(&xdg, &crate::l10n::messages_locale().unwrap_or_default());
    let default = apps.default_for(mime);
    let mut others: Vec<DesktopEntry> = apps
        .associated(mime)
        .into_iter()
        .filter(|entry| default.as_ref().is_none_or(|default| default.id != entry.id))
        .collect();
    others.sort_by(|a, b| collator.compare(&a.name, &b.name));
    let app = |entry: DesktopEntry, is_default: bool| OpenWithApp { id: entry.id, name: entry.name, is_default };
    default.map(|entry| app(entry, true)).into_iter().chain(others.into_iter().map(|entry| app(entry, false))).collect()
}

/// Starts the application of desktop file ID `id` on `file`.
pub fn open_with(id: &str, file: &Path, file_url: &str) -> Result<(), OpenWithError> {
    let xdg = Xdg::from_env();
    let apps = Applications::scan(&xdg, &crate::l10n::messages_locale().unwrap_or_default());
    let entry = apps.entry(id).ok_or_else(|| OpenWithError::NotInstalled(id.to_owned()))?;
    launch(&entry, file, file_url)
        .inspect_err(|e| warn!(app = %id, error = %e, "an application did not open a photo"))
        .map_err(OpenWithError::from)
}
