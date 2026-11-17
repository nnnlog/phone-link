//! Blocking file work, run off the async runtime's workers.

use tracing::error;

use crate::user_text;

/// Blocking file work that stopped before it finished: its task panicked or was cancelled.
///
/// The window reads it as something that went wrong; [`blocking`] logs the stop where it sees it.
#[derive(Debug, thiserror::Error)]
#[error("{}", user_text::something_went_wrong())]
pub(super) struct WorkStopped;

impl From<WorkStopped> for String {
    fn from(stopped: WorkStopped) -> Self {
        stopped.to_string()
    }
}

/// Runs blocking file work off the async runtime's workers.
pub(super) async fn blocking<T, E>(work: impl FnOnce() -> Result<T, E> + Send + 'static) -> Result<T, E>
where
    T: Send + 'static,
    E: From<WorkStopped> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.map_err(|e| {
        error!(error = %e, "a photo's file work stopped");
        E::from(WorkStopped)
    })?
}
