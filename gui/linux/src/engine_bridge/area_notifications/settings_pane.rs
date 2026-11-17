//! The Notifications settings pane's actions: the access requests it sends to the phone, the apps'
//! settings it asks for when shown, and each app's choice.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use tracing::{info, warn};

use super::app_commands::app_request;
use super::failures::{app_outcome_later, error_text};
use super::state::PaneRequest;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;
use crate::user_text;
use crate::views::notifications::AppChoice;

impl EngineBridge {
    /// A request the settings pane sends to the phone; an outcome other than success is shown.
    pub(super) fn request_from_pane(mut self: Pin<&mut Self>, engine: Arc<Engine>, kind: PaneRequest) {
        if !self.as_mut().notifications_mut().pane_requests.insert(kind) {
            return;
        }
        self.as_mut().emit_changed();

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let api = engine.notifications();
            let result = match kind {
                PaneRequest::Access => api.request_access().await,
                PaneRequest::SensitiveContent => api.request_sensitive_access().await,
            };

            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().notifications_mut().pane_requests.remove(&kind);
                let text = match result {
                    Ok(outcome) => user_text::outcome_failure(&outcome)
                        .inspect(|_| info!(?kind, reason = ?outcome, "phone access was not granted")),
                    Err(e) => error_text(&e),
                };
                if let Some(text) = text {
                    bridge.as_mut().show_failure(Failure::new("Couldn't Request Access", text));
                }
                bridge.reload_notification_state();
            });
        });
    }

    /// The settings pane is shown: the phone is asked for its apps' settings (when it reports its apps),
    /// which arrive as a settings change.
    pub(super) fn open_settings_view(self: Pin<&mut Self>, engine: Arc<Engine>) {
        self.runtime.spawn(async move {
            match engine.notifications().open_settings_view().await {
                Ok(Some(outcome)) if user_text::outcome_failure(&outcome).is_some() => {
                    info!(reason = ?outcome, "phone app settings were not sent; the pane shows those last read");
                }
                Ok(_) | Err(FeatureApiError::NotStarted) => {}
                Err(e) => info!(error = %e, "phone app settings were not requested; the pane shows those last read"),
            }
        });
    }

    /// Sets how an app's notifications reach this computer. Changing whether they are listed asks the
    /// phone, which can take a moment: the pane shows the choice and waits meanwhile, then what the
    /// engine kept.
    pub(super) fn set_app_choice(mut self: Pin<&mut Self>, engine: Arc<Engine>, arg: &str) {
        let Some(app) = app_request("notifications.setAppChoice", arg) else { return };
        let Some(choice) = app.request["choice"].as_str().and_then(AppChoice::parse) else {
            warn!("notifications.setAppChoice: unknown choice");
            return;
        };
        if self.areas.notifications.pending_apps.contains_key(&app.package) {
            return;
        }

        let title = "Couldn't Change Notifications from %@";
        let current = match engine.clone().notifications().app_setting(app.package.clone()) {
            Ok(setting) => setting.flags,
            Err(e) => {
                if let Some(text) = error_text(&e) {
                    self.show_app_failure(title, &app.name, text);
                }
                return;
            }
        };
        let flags = choice.flags(current);
        if flags == current {
            return;
        }

        self.as_mut().notifications_mut().pending_apps.insert(app.package.clone(), flags);
        self.as_mut().emit_changed();

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.notifications().change_app_setting(app.package.clone(), flags).await;
            let package = app.package.clone();
            app_outcome_later(&thread, title, app.name, result);
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().notifications_mut().pending_apps.remove(&package);
                bridge.emit_changed();
            });
        });
    }
}
