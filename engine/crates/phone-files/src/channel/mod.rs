//! The feature's `NamedStreams` channels and how their failures are named.
//!
//! Each operation opens one short-lived channel against the phone's context handle. A per-phone
//! breaker sends the metadata and thumbnail requests over `/DeviceResourceManager` for a while after
//! the channel kept failing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::time::Instant;

use phonelink_core::types::TraceContext;
use phonelink_features::shared::nano_channels::{ChannelOpenError, NanoShareChannels, TransferChannel};
use phonelink_nano::connect::InitializeError;
use phonelink_nano::device::{
    AcceptanceReason, DeviceConnectionError, Listener, PlatformChannelError, PlatformChannelType,
};
use phonelink_nano::named_streams::NamedStreams;
use phonelink_transport::platform::{ContextHandle, DrmSendOutcome};

use super::wire::Status;

/// An open channel and the messages it receives on the targets it was opened with.
pub struct FilesChannel {
    pub channel: Arc<TransferChannel>,
    pub listeners: HashMap<String, Listener>,
}

impl FilesChannel {
    /// The messages on `target` (one of the targets the channel was opened with).
    pub fn take(&mut self, target: &str) -> Listener {
        self.listeners.remove(target).expect("a target the channel was opened with")
    }
}

/// Creates and opens the feature's `NamedStreams` channels.
#[async_trait]
pub trait FilesChannels: Send + Sync {
    /// Creates the `NamedStreams` channel `scenario` against `handle`'s phone and opens it, listening to
    /// each of `targets` from its opening on; `trace` is the operation's trace context.
    async fn open(
        &self,
        handle: &ContextHandle,
        scenario: &str,
        targets: &[&str],
        trace: &TraceContext,
    ) -> Result<FilesChannel, ChannelOpenError>;
}

#[async_trait]
impl FilesChannels for NanoShareChannels {
    async fn open(
        &self,
        handle: &ContextHandle,
        scenario: &str,
        targets: &[&str],
        trace: &TraceContext,
    ) -> Result<FilesChannel, ChannelOpenError> {
        let device = self.connections(handle.partner.env).get_or_open(handle).await?;
        let (channel, listeners) = device
            .create_platform_channel_listening(
                PlatformChannelType::NamedStreams,
                scenario,
                HashMap::new(),
                targets,
                trace,
            )
            .await?;
        let streams = NamedStreams::new(channel, self.config().platform_channel_creation_timeout)
            .map_err(PlatformChannelError::Channel)?;
        Ok(FilesChannel { channel: TransferChannel::new(streams, Some(device)), listeners })
    }
}

/// Why an operation over a channel (or the request around it) failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("no context handle for the phone")]
    NoContextHandle,
    #[error("the phone's context handle is no longer valid")]
    InvalidContextHandle,
    #[error("the channel was not created in time")]
    ChannelCreationTimedOut,
    #[error("the Mac is locked")]
    UserSessionLocked,
    #[error("the phone does not transfer on a metered connection")]
    MeteredSyncDisabled,
    #[error("the phone needs the files permission")]
    PermissionRequired,
    #[error("Link to Windows is turned off on the phone")]
    LtwOff,
    #[error("the channel could not be set up")]
    FailedPlatformError,
    #[error("the channel closed")]
    FailedTransferClosed,
    #[error("the connection to the phone was lost")]
    FailedConnectionLost,
    #[error("the file was not found on the phone")]
    FileNotFound,
    #[error("cancelled")]
    UserCancelled,
    /// The same file is already being downloaded.
    #[error("the file is already being downloaded")]
    InProgress,
    /// What failed, as the code states it.
    #[error("the transfer failed: {0}")]
    Failed(String),
    /// The phone answered with a status other than `SUCCESS`.
    #[error("the phone answered {0:?}")]
    Status(Status),
    /// The `/DeviceResourceManager` request did not get a successful reply.
    #[error("the request failed: {0:?}")]
    Request(DrmSendOutcome),
}

impl TransferError {
    /// The breaker counts this failure.
    pub fn is_recorded(&self) -> bool {
        matches!(
            self,
            Self::Failed(_)
                | Self::FailedPlatformError
                | Self::FailedTransferClosed
                | Self::FailedConnectionLost
                | Self::ChannelCreationTimedOut
        )
    }

    /// A content transfer that failed this way is tried once more.
    pub fn retries_content(&self) -> bool {
        matches!(self, Self::NoContextHandle | Self::InvalidContextHandle | Self::FailedConnectionLost)
    }
}

/// The failure a channel that did not open stands for. `content` adds the content channel's
/// connection-loss and handle cases.
pub fn open_error(e: &ChannelOpenError, metered: bool, content: bool) -> TransferError {
    match e.acceptance() {
        Some(AcceptanceReason::PermissionRevoked) => TransferError::PermissionRequired,
        Some(AcceptanceReason::AppDisabled) => TransferError::LtwOff,
        Some(_) if metered => TransferError::MeteredSyncDisabled,
        Some(_) => TransferError::FailedPlatformError,
        None => match e {
            ChannelOpenError::Channel(PlatformChannelError::CreateRequestTimedOut)
            | ChannelOpenError::Device(DeviceConnectionError::ChannelOpenTimeout) => {
                TransferError::ChannelCreationTimedOut
            }
            _ if metered => TransferError::MeteredSyncDisabled,
            ChannelOpenError::Channel(PlatformChannelError::DeviceConnectionFailed)
            | ChannelOpenError::Device(DeviceConnectionError::Attempt(_))
                if content =>
            {
                TransferError::FailedConnectionLost
            }
            ChannelOpenError::Device(
                DeviceConnectionError::ContextDropped
                | DeviceConnectionError::Initialize(
                    InitializeError::ContextDropped | InitializeError::Send(_) | InitializeError::Timeout,
                ),
            ) if content => TransferError::InvalidContextHandle,
            _ => TransferError::FailedPlatformError,
        },
    }
}

/// Per phone: opens after `threshold` consecutive recorded failures, stays open `open_for`, then
/// admits one probe whose outcome closes or re-opens it.
#[derive(Debug)]
pub struct Breaker {
    threshold: u32,
    open_for: Duration,
    failures: u32,
    open_until: Option<Instant>,
    probing: bool,
}

impl Breaker {
    pub fn new(threshold: u32, open_for: Duration) -> Self {
        Self { threshold, open_for, failures: 0, open_until: None, probing: false }
    }

    /// An attempt may go over the channel now.
    pub fn attempt(&mut self) -> bool {
        match self.open_until {
            None => true,
            Some(until) if Instant::now() < until => false,
            Some(_) if self.probing => false,
            Some(_) => {
                self.probing = true;
                true
            }
        }
    }

    pub fn success(&mut self) {
        self.failures = 0;
        self.open_until = None;
        self.probing = false;
    }

    pub fn failure(&mut self) {
        self.failures += 1;
        if self.probing || self.failures >= self.threshold {
            self.open_until = Some(Instant::now() + self.open_for);
            self.probing = false;
        }
    }

    /// The outcome of an attempt the breaker admitted: a success, a recorded failure, or neither;
    /// whether it opened or closed the breaker (a probe that re-opens it changes nothing).
    pub fn record(&mut self, outcome: &Result<(), TransferError>) -> Option<BreakerChange> {
        let was_open = self.open_until.is_some();
        match outcome {
            Ok(()) => self.success(),
            Err(e) if e.is_recorded() => self.failure(),
            Err(_) => {
                // Not recorded: a probe that ends this way leaves the breaker open for another probe.
                self.probing = false;
            }
        }

        match (was_open, self.open_until.is_some()) {
            (false, true) => Some(BreakerChange::Opened),
            (true, false) => Some(BreakerChange::Closed),
            _ => None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.threshold, self.open_for);
    }
}

/// How an outcome changed the breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerChange {
    /// It opened from closed: requests go over `/DeviceResourceManager`.
    Opened,
    /// It closed from open: requests go over the channel again.
    Closed,
}

#[cfg(test)]
mod tests;
