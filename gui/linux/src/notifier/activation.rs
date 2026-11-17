//! The activation token of the last click on a notification, kept for the window activation the click
//! causes.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use phonelink_core::sync::MutexExt;

/// How long the activation token of a click stays usable for bringing a window forward.
pub(super) const TOKEN_LIFETIME: Duration = Duration::from_secs(10);

/// The activation token of the last click on a notification, for the window activation it causes.
pub(super) static ACTIVATION_TOKEN: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Keeps `token`, of a click on a notification now, in place of an earlier one.
pub(super) fn keep_activation_token(token: String) {
    *ACTIVATION_TOKEN.lock_or_recover() = Some((token, Instant::now()));
}

/// Takes the activation token of a notification clicked in the last few seconds, for the window
/// activation the click causes (desktop.h's `setActivationToken` hands it to Qt, so that the compositor
/// brings the window forward); `None` without a fresh one.
pub(crate) fn take_activation_token() -> Option<String> {
    fresh_token(Instant::now())
}

/// Takes the token of the last click; `None` when there is none or it is older than [`TOKEN_LIFETIME`]
/// at `now`.
pub(super) fn fresh_token(now: Instant) -> Option<String> {
    let (token, at) = ACTIVATION_TOKEN.lock_or_recover().take()?;
    (now.saturating_duration_since(at) <= TOKEN_LIFETIME).then_some(token)
}
