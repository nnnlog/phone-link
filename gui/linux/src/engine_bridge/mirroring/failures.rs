//! What the user sees when a `mirror.*` action fails.

use std::pin::Pin;

use phonelink_engine::api::FeatureApiError;
use tracing::debug;

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Failure;

/// The title of what did not happen when a `mirror.*` action fails.
///
/// It is `None` for the app's own bookkeeping and the input (one comes per mouse move or key press),
/// which are only logged.
pub(super) fn failure_title(name: &str) -> Option<&'static str> {
    let title = match name {
        "mirror.openPhoneScreen" => "Couldn't Open Your Phone's Screen",
        "mirror.launch" => "Couldn't Open the App",
        "mirror.launchNotification" => "Couldn't Open on Your Phone",
        "mirror.dismiss" => "Couldn't Dismiss the Error",
        "mirror.retry" | "mirror.allowAndRetry" => "Couldn't Connect",
        "mirror.wake" => "Couldn't Wake Your Phone",
        "mirror.resendPermission" | "mirror.audioPermission" | "mirror.enableAudio" | "mirror.nearbyRequestPin" => {
            "Couldn't Ask Your Phone"
        }
        "mirror.resume" => "Couldn't Resume the App",
        "mirror.reopen" => "Couldn't Reopen the App",
        "mirror.trueTouch"
        | "mirror.continueWithoutAudio"
        | "mirror.nearbySkipUnlock"
        | "mirror.nearbySkipVerification" => "Couldn't Continue",
        "mirror.moveToComputer" => "Couldn't Respond to Your Phone",
        "mirror.switchLayout" | "mirror.confirmLayout" => "Couldn't Change the Layout",
        "mirror.volume" => "Couldn't Change the Volume",
        "mirror.keyboardSettings" => "Couldn't Open Keyboard Settings",
        "mirror.nearbyPin" => "Couldn't Send the Code",
        "mirror.setting" => "Couldn't Change the Setting",
        _ => return None,
    };
    Some(title)
}

impl EngineBridge {
    /// Shows why a mirroring action failed; the app's own bookkeeping and the input are only logged.
    pub(super) fn report_mirror_failure(self: Pin<&mut Self>, name: &str, error: &FeatureApiError) {
        match failure_title(name) {
            Some(title) => self.show_failure(Failure::feature(title, error)),
            None => debug!(name, %error, "a mirroring action failed"),
        }
    }
}
