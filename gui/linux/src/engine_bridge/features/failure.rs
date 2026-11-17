//! A failed action as the user sees it, and showing it in the window.

use std::pin::Pin;

use cxx_qt::CxxQtThread;
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use tracing::warn;

use crate::engine_bridge::EngineBridge;
use crate::l10n::translate;
use crate::user_text;

/// A failed action as the user sees it: a title naming what did not happen, and why.
///
/// The title is an English key, translated when the failure is shown; the message is already
/// translated.
pub(in crate::engine_bridge) struct Failure {
    pub(in crate::engine_bridge) title: &'static str,
    pub(in crate::engine_bridge) message: String,
}

impl Failure {
    pub(in crate::engine_bridge) fn new(title: &'static str, message: String) -> Self {
        Self { title, message }
    }

    pub(in crate::engine_bridge) fn feature(title: &'static str, error: &FeatureApiError) -> Self {
        Self::new(title, user_text::feature_error(error))
    }

    /// A setting the user changed that `error` kept from being saved.
    pub(in crate::engine_bridge) fn setting(error: &FeatureApiError) -> Self {
        Self::feature("Couldn't Change the Setting", error)
    }
}

/// Shows `failure` from a thread other than the GUI's.
pub(in crate::engine_bridge) fn fail_later(thread: &CxxQtThread<EngineBridge>, failure: Failure) {
    let _ = thread.queue(move |bridge| bridge.show_failure(failure));
}

impl EngineBridge {
    /// Shows a failed action in the window, and logs it.
    pub(in crate::engine_bridge) fn show_failure(self: Pin<&mut Self>, failure: Failure) {
        warn!(failure_kind = failure.title, text = %failure.message, "a failure is shown");
        self.failure(QString::from(translate(failure.title)), QString::from(failure.message.as_str()));
    }
}
