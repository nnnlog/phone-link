//! Desktop notifications through the freedesktop.org notification server: the engine's notifications
//! and the app's own, and what the user does with them, handed back to the engine or to the app.

mod activation;
mod content;
mod freedesktop;
mod local;
mod server;
mod shown;
#[cfg(test)]
mod tests;
mod watch;

pub(crate) use activation::take_activation_token;
pub(crate) use freedesktop::FreedesktopNotifier;
pub(crate) use local::LocalNotification;
