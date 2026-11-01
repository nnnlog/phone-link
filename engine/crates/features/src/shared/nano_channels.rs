//! Where a transfer's `NamedStreams` channel comes from: [`ShareChannels`] creates and opens one against
//! the phone's context handle.
//!
//! [`NanoShareChannels`] does it over the Nano device connection (one per
//! phone, shared with the other features on the handle, phone files among them); the tests hand in
//! channels of their own.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use phonelink_core::config::NanoConfig;
use phonelink_core::seams::TokenSource;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{Environment, TraceContext};
use phonelink_nano::cloud_relay::CloudRelay;
use phonelink_nano::device::{
    AcceptanceReason, DeviceConnection, DeviceConnectionError, DeviceConnections, Listener, PlatformChannelError,
    PlatformChannelType,
};
use phonelink_nano::named_streams::NamedStreams;
use phonelink_transport::auth::AuthService;
use phonelink_transport::platform::ContextHandle;

/// The channel message target of the control messages.
pub const CONTROL_TARGET: &str = "/fileTransfer/platform";

/// Why a channel could not be created or opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelOpenError {
    /// No device connection to the phone.
    #[error("device connection: {0}")]
    Device(#[from] DeviceConnectionError),
    /// The phone refused the channel, or it did not open.
    #[error("platform channel: {0}")]
    Channel(#[from] PlatformChannelError),
}

impl ChannelOpenError {
    /// The phone's acceptance status, when it refused the channel.
    pub fn acceptance(&self) -> Option<AcceptanceReason> {
        match self {
            Self::Channel(
                PlatformChannelError::NoRemoteScenarioAvailable(r)
                | PlatformChannelError::RemoteScenarioRejectedChannel(r)
                | PlatformChannelError::PartnerChannelCreationFailure(r),
            ) => Some(*r),
            _ => None,
        }
    }

    /// The phone refused the channel (as opposed to a channel that did not open).
    pub fn is_rejection(&self) -> bool {
        self.acceptance().is_some()
    }
}

/// A transfer's channel, holding the device connection it runs over: the connection closes once no
/// channel or other user holds it.
pub struct TransferChannel {
    streams: Arc<NamedStreams>,
    device: Option<Arc<DeviceConnection>>,
}

impl TransferChannel {
    /// `streams` over `device` (`None` when nothing else must be kept open for it).
    pub fn new(streams: Arc<NamedStreams>, device: Option<Arc<DeviceConnection>>) -> Arc<Self> {
        Arc::new(Self { streams, device })
    }

    pub fn streams(&self) -> &Arc<NamedStreams> {
        &self.streams
    }

    /// The device connection the channel runs over has closed.
    pub fn device_closed(&self) -> bool {
        self.device.as_ref().is_some_and(|d| d.is_closed())
    }

    /// The device connection's cumulative metrics (JSON), when the channel runs over one.
    pub fn metrics(&self) -> Option<String> {
        self.device.as_ref().map(|d| d.connection.cumulative_metrics())
    }
}

impl std::ops::Deref for TransferChannel {
    type Target = NamedStreams;

    fn deref(&self) -> &NamedStreams {
        &self.streams
    }
}

/// An open transfer channel and the control messages it receives.
pub struct OpenedChannel {
    pub channel: Arc<TransferChannel>,
    /// The messages on [`CONTROL_TARGET`], received from the moment the channel opened.
    pub control: Listener,
}

/// Creates and opens a transfer's `NamedStreams` channel.
#[async_trait]
pub trait ShareChannels: Send + Sync {
    /// Creates the `NamedStreams` channel `scenario` against `handle`'s phone and opens it, listening to
    /// the control target from its opening on; `trace` is the transfer's trace context.
    async fn open(
        &self,
        handle: &ContextHandle,
        scenario: &str,
        trace: &TraceContext,
    ) -> Result<OpenedChannel, ChannelOpenError>;
}

/// [`ShareChannels`] over the Nano device connection, with the CDEH identity's `CloudRelay` servers.
pub struct NanoShareChannels {
    config: NanoConfig,
    auth: Arc<AuthService>,
    connections: Mutex<HashMap<Environment, Arc<DeviceConnections>>>,
}

impl NanoShareChannels {
    pub fn new(config: NanoConfig, auth: Arc<AuthService>) -> Arc<Self> {
        Arc::new(Self { config, auth, connections: Mutex::default() })
    }

    /// The Nano settings the channels are opened with.
    pub fn config(&self) -> &NanoConfig {
        &self.config
    }

    /// The device connections of `env`, shared by every feature that opens channels through this.
    pub fn connections(&self, env: Environment) -> Arc<DeviceConnections> {
        let mut connections = self.connections.lock_or_panic();
        connections
            .entry(env)
            .or_insert_with(|| {
                let relay = CloudRelay::new(self.auth.rest().clone(), self.auth.clone() as Arc<dyn TokenSource>, env);
                DeviceConnections::new(self.config.clone(), Arc::new(relay), None)
            })
            .clone()
    }
}

#[async_trait]
impl ShareChannels for NanoShareChannels {
    async fn open(
        &self,
        handle: &ContextHandle,
        scenario: &str,
        trace: &TraceContext,
    ) -> Result<OpenedChannel, ChannelOpenError> {
        let device = self.connections(handle.partner.env).get_or_open(handle).await?;

        let (channel, mut listeners) = device
            .create_platform_channel_listening(
                PlatformChannelType::NamedStreams,
                scenario,
                HashMap::new(),
                &[CONTROL_TARGET],
                trace,
            )
            .await?;

        let control = listeners.remove(CONTROL_TARGET).expect("listened to");
        let streams = NamedStreams::new(channel, self.config.platform_channel_creation_timeout)
            .map_err(PlatformChannelError::Channel)?;
        Ok(OpenedChannel { channel: TransferChannel::new(streams, Some(device)), control })
    }
}
