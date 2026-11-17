//! The engine's OS edges that belong to no feature: the secure store in the Secret Service, the
//! computer's name and region, and a notifier that writes to standard error; and the error an edge
//! reports for a failure the engine has no variant of its own for.

mod host;
mod secret_store;
mod standins;

pub use host::LinuxHost;
use phonelink_engine::edge::EdgeError;
pub use secret_store::SecretServiceStore;
pub use standins::StderrNotifier;

/// An edge's failure that the engine has no variant of its own for, with what failed.
pub fn unexpected(message: impl Into<String>) -> EdgeError {
    EdgeError::Unexpected { message: message.into() }
}
