//! Files: sending files to the phone, receiving them, and the phone's files mounted at `~/PhoneLink`.
//!
//! Sending goes through the Send Files window, which shows its progress and outcome; received files
//! go to the received-files folder; the sidebar's card shows the transfers' progress.
//!
//! `query` names: `transfers`, `phoneFiles`, `files.settings`.
//! `perform` names: `files.*`, `phoneFiles.*`.
//!
//! The QML hears `areaEvent("files", "showSendWindow", {})` to open the Send Files window, and
//! `areaEvent("files", "sendFailure", {title, message})` for a failure the window shows.

mod file_size;
mod phone_mount;
mod receiving;
mod routing;
mod send_outcome;
mod sending;
mod settings_view;
mod state;
#[cfg(test)]
mod tests;
mod transfer_view;

pub(super) use state::State;
