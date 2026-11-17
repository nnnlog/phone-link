//! How the notifications part shows a failed action: dropped before the engine starts, else in the
//! window, with the app's name in the title for an app's command.

use std::pin::Pin;

use cxx_qt::CxxQtThread;
use cxx_qt_lib::QString;
use phonelink_engine::api::{FeatureApiError, RequestOutcome};
use tracing::{debug, info, warn};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;
use crate::l10n::{fill, translate};
use crate::user_text;

/// Why an action the user started failed, for the window; `None` before the engine starts, when the action
/// is dropped.
pub(super) fn error_text(error: &FeatureApiError) -> Option<String> {
    if matches!(error, FeatureApiError::NotStarted) {
        debug!("a notification action was dropped; the engine has not started");
        return None;
    }
    Some(user_text::feature_error(error))
}

/// Shows why a request about an app failed (from any thread): its outcome on the phone, or the error.
pub(super) fn app_outcome_later(
    thread: &CxxQtThread<EngineBridge>,
    title: &'static str,
    app: String,
    result: Result<RequestOutcome, FeatureApiError>,
) {
    let text = match result {
        Ok(outcome) => user_text::outcome_failure(&outcome)
            .inspect(|_| info!(reason = ?outcome, "an app's notification setting was not changed on the phone")),
        Err(e) => error_text(&e),
    };
    if let Some(text) = text {
        let _ = thread.queue(move |bridge| bridge.show_app_failure(title, &app, text));
    }
}

impl EngineBridge {
    /// Shows why an action the user started failed; before the engine starts the action is only dropped.
    pub(super) fn report(self: Pin<&mut Self>, error: &FeatureApiError, title: &'static str) {
        if let Some(text) = error_text(error) {
            self.show_failure(Failure::new(title, text));
        }
    }

    /// A failure whose title names an app: `title` is an English key with one `%@`.
    pub(super) fn show_app_failure(self: Pin<&mut Self>, title: &str, app: &str, message: String) {
        warn!(failure_kind = title, app, text = %message, "a failure about an app is shown");
        let title = fill(translate(title), &[app.to_owned()]);
        self.failure(QString::from(title.as_str()), QString::from(message.as_str()));
    }

    /// A change of a setting the user made: its failure is shown.
    pub(super) fn setting_changed(self: Pin<&mut Self>, result: Result<(), FeatureApiError>) {
        if let Err(e) = result {
            self.report(&e, "Couldn't Change the Setting");
        }
    }
}
