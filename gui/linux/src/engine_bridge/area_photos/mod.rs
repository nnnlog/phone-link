//! Photos and apps: the phone's recent photos (the grid and its selection, the viewer, the file
//! actions — open, open with, copy, save, drag out — delete, the photo permission requests and the
//! Photos settings), new-photo notifications (the photos feature's own and the phone's new-photo
//! notifications, which open the photo in the default viewer), and the phone's apps (recent apps,
//! favorites, every app, search, icons, stored names).
//!
//! `query` names: `photos`, `apps`, `appIcon`, `photos.*`, `apps.*`, `snipping.*`.
//! `perform` names: `photos.*`, `apps.*`, `snipping.*`.
//!
//! The QML hears `areaEvent("photos", …)`: `ui` (the selection, the focus, the viewer's photo, the
//! photos waiting on the phone; the payload is `query("photos.ui")`), `prompt` (a delete confirmation
//! or the "allow deleting" question), `saveDialog` (the save panel for a photo, with its file name) and
//! `fullImage` (photo `id`'s full image is stored).

mod action_file;
mod apps;
mod apps_section;
mod copying;
mod delete;
mod dragging;
mod events;
mod files;
mod gallery;
mod grid;
mod opening;
mod routing;
mod saving;
mod selection;
mod snipping;
mod state;
#[cfg(test)]
mod tests;
mod work;

use phonelink_engine::api::FeatureApiError;

pub(super) use state::State;

/// Whether `error` says only that the engine has not started or that there is no phone: a read the user
/// did not ask for that fails this way is not worth a log line.
fn quiet(error: &FeatureApiError) -> bool {
    matches!(error, FeatureApiError::NotStarted | FeatureApiError::NoPhone)
}
