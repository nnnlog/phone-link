//! Screen mirroring: the phone's screen and apps in windows of their own, their input, the Phone
//! Screen settings, opening a notification on the phone, and the computer's audio outputs.
//!
//! `query` names: `mirror.*`. `perform` names: `mirror.*` (`mirror.display` is routed before the
//! engine exists). Other parts call `mirror.launch` (a package name, or `{"package", "taskId"}` for a
//! recent task) and `mirror.launchNotification` (a notification's key).

mod deferred;
mod drag;
mod events;
mod failures;
mod input;
mod request;
mod routing;
mod session_actions;
#[cfg(test)]
mod tests;

pub(super) use routing::set_display;

/// What the mirroring part keeps between calls.
#[derive(Default)]
pub(super) struct State {
    /// Whether the engine is told when the computer's audio outputs change (from the first mirroring
    /// event on).
    outputs_watched: bool,
}
