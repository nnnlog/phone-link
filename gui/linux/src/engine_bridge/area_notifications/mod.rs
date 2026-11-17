//! Notifications: the phone's notification list and what stands in for it while the phone or this
//! computer is not set up for it, the actions on a notification (dismiss, pin, reply, its buttons) and
//! on its app (turn off with Undo, turn on, mute), pausing every notification, the Notifications
//! settings pane (this computer's switches, each app's choice, the access the phone gives), and what a
//! click on a desktop notification opens.
//!
//! `query` names: `notifications.*`. `perform` names: `notifications.*`.
//!
//! Every change the views show is announced as the area event `changed`; `knownApps` hands the apps
//! the settings pane lists to `NotificationsHost.qml`, which keeps them between runs and gives them back
//! with `notifications.restoreKnownApps`.

mod app_commands;
mod events;
mod failures;
mod known_apps;
mod list_actions;
mod queries;
mod reading;
mod routing;
mod settings_pane;
mod state;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use tracing::info;

pub(super) use state::State;

/// The selected phone's id; `None` (logged) when it cannot be read.
fn phone_id(engine: &Arc<Engine>) -> Option<String> {
    match engine.clone().phone().info() {
        Ok(info) => Some(info.id),
        Err(e) => {
            info!(error = %e, "the selected phone was not read");
            None
        }
    }
}
