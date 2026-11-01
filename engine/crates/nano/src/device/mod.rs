//! The device connection.
//!
//! It covers a Nano connection to one phone whose `DeviceConnection` message channel is open,
//! opened over the phone's context handle and shared by every feature on that handle; and the
//! platform channels created over it — the creation request and the phone's result, the matching of
//! the Nano channels the phone announces, and opening them.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};
use futures_util::stream::{FuturesUnordered, StreamExt};
use prost::Message as _;
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use super::channel::audio::AudioTarget;
use super::channel::blob::BlobChannel;
use super::channel::input::InputSource;
use super::channel::message::MessageChannel;
use super::channel::video::VideoTarget;
use super::channel::{ChannelError, ChannelType, RawChannel, property};
use super::cloud_relay::RelaySource;
use super::connect::{AttemptError, ContextExchange, InitializeError, device_attempts, initialize_context, race};
#[cfg(feature = "test-support")]
use super::connection::CloseReason;
use super::connection::NanoConnection;
use super::netif::HostAddress;
use phonelink_core::config::NanoConfig;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext, new_guid_lower};
use phonelink_proto::platform_sdk::v1::{
    ChannelAcceptanceStatus, ProtoCreatePlatformChannelRequest, ProtoCreatePlatformChannelStatus,
    ProtoPeerPlatformChannelCreationResult, ProtoPlatformChannelType,
};
use phonelink_transport::platform::ContextHandle;
use phonelink_transport::platform::context_message::proto_trace;

pub const CREATE_REQUEST_TARGET: &str = "/Channel/ProtoCreatePlatformChannelRequest";
pub const CREATION_RESULT_TARGET: &str = "/Channel/ProtoPeerPlatformChannelCreationResult";

/// Scenario ids.
pub mod scenario {
    pub const INTERNAL_MESSAGING: &str = "internalMessaging_F52ED1B1-E828-4C13-A61F-75E48526424F";
    pub const APP_REMOTING: &str = "appRemoting_0525179B-03D2-4E10-B9F3-1AFF564114AB";
    pub const MIRRORING_WORKFLOW_MESSAGE: &str = "mirroringWorkflowMessage_2A0B6A54-458A-4174-99B1-723B88103FAA";
    pub const FILE_SHARE: &str = "fileShare_FDF73A34-0C30-4CC0-8165-87FC30DB1E35";
    pub const FILE_SHARE_2: &str = "fileShare_2B0F3A34-0C30-4CC0-8165-87FC30DB1E35";
    pub const FILE_TRANSFER: &str = "fileTransfer_1EAA053C-0354-4DF8-AA05-888312CCC57E";
    pub const FILES_REQUEST_TRANSFER: &str = "filesRequestTransfer_3GAC175E-1576-6FA0-CC27-000534EEE79G";
    pub const THUMBNAIL_TRANSFER: &str = "thumbnailTransfer_2FAB064D-0465-5EF9-BB16-999423DDD68F";
    pub const STREAMING: &str = "streaming_8D6510FF-D41C-4EDF-9577-8902F6184EE7";
}

/// Why opening a device connection failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeviceConnectionError {
    #[error("the context handle was dropped")]
    ContextDropped,
    #[error(transparent)]
    Initialize(#[from] InitializeError),
    #[error("no attempt opened: {0}")]
    Attempt(AttemptError),
    #[error("channel open timeout")]
    ChannelOpenTimeout,
}

/// Why creating a platform channel failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformChannelError {
    #[error("CreatePlatformChannelRequestTimedOut")]
    CreateRequestTimedOut,
    #[error("DeviceConnectionFailed")]
    DeviceConnectionFailed,
    #[error("ErrorNoRemoteScenarioAvailable ({0:?})")]
    NoRemoteScenarioAvailable(AcceptanceReason),
    #[error("ErrorRemoteScenarioRejectedChannel ({0:?})")]
    RemoteScenarioRejectedChannel(AcceptanceReason),
    #[error("PartnerChannelCreationFailure ({0:?})")]
    PartnerChannelCreationFailure(AcceptanceReason),
    #[error("the Nano channels were not announced or opened in time")]
    Timeout,
    #[error("a second channel of one kind for this platform channel")]
    DuplicateChannel,
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

/// `ChannelAcceptanceStatus` as the phone gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceReason {
    Success,
    PermissionRevoked,
    AppDisabled,
    MobileDataDisabled,
    NotSupported,
    InvalidScenario,
    NoCallbackProvided,
    Unspecified,
}

impl AcceptanceReason {
    fn from_wire(v: i32) -> Self {
        match ChannelAcceptanceStatus::try_from(v) {
            Ok(ChannelAcceptanceStatus::Success) => Self::Success,
            Ok(ChannelAcceptanceStatus::PermissionRevoked) => Self::PermissionRevoked,
            Ok(ChannelAcceptanceStatus::AppDisabled) => Self::AppDisabled,
            Ok(ChannelAcceptanceStatus::MobileDataDisabled) => Self::MobileDataDisabled,
            Ok(ChannelAcceptanceStatus::NotSupported) => Self::NotSupported,
            Ok(ChannelAcceptanceStatus::InvalidScenario) => Self::InvalidScenario,
            Ok(ChannelAcceptanceStatus::NoCallbackProvided) => Self::NoCallbackProvided,
            _ => Self::Unspecified,
        }
    }
}

/// Platform channel types the PC requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformChannelType {
    NamedStreams,
    VideoStreaming,
    InternalMessaging,
    Messaging,
    AppRemote,
}

impl PlatformChannelType {
    fn wire(self) -> ProtoPlatformChannelType {
        match self {
            Self::NamedStreams => ProtoPlatformChannelType::NamedStreamsChannel,
            Self::VideoStreaming => ProtoPlatformChannelType::VideoStreamingChannel,
            Self::InternalMessaging => ProtoPlatformChannelType::InternalMessagingChannel,
            Self::Messaging => ProtoPlatformChannelType::MessagingChannel,
            Self::AppRemote => ProtoPlatformChannelType::AppRemoteChannel,
        }
    }

    /// The Nano channels the phone creates for it.
    pub fn expected(self) -> &'static [ChannelType] {
        match self {
            Self::NamedStreams => &[ChannelType::Message, ChannelType::Blob],
            Self::VideoStreaming => &[ChannelType::Message, ChannelType::VideoTarget],
            Self::InternalMessaging | Self::Messaging => &[ChannelType::Message],
            Self::AppRemote => &[
                ChannelType::Message,
                ChannelType::VideoTarget,
                ChannelType::InputSource,
                ChannelType::Blob,
                ChannelType::AudioTarget,
            ],
        }
    }

    /// The scope prefix of its messages: `/<scenario>/NamedStreams`, `/<scenario>/Messaging`,
    /// `/<scenario>/VideoStreaming`, or `/InternalMessaging`.
    pub fn scope_prefix(self, scenario: &str) -> String {
        match self {
            Self::NamedStreams => format!("/{scenario}/NamedStreams"),
            Self::Messaging | Self::AppRemote => format!("/{scenario}/Messaging"),
            Self::VideoStreaming => format!("/{scenario}/VideoStreaming"),
            Self::InternalMessaging => "/InternalMessaging".into(),
        }
    }
}

/// A platform channel's opened Nano channels.
pub struct PlatformChannel {
    pub kind: PlatformChannelType,
    pub scenario_id: String,
    pub channel_id: String,
    pub message: MessageChannel,
    pub blob: Option<BlobChannel>,
    pub video: Option<VideoTarget>,
    pub input: Option<InputSource>,
    pub audio: Option<AudioTarget>,
}

impl std::fmt::Debug for PlatformChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformChannel").field("kind", &self.kind).field("scenario", &self.scenario_id).finish()
    }
}

impl PlatformChannel {
    /// Sends feature bytes on `<prefix><sub>` (`sub` with its leading `/`).
    pub fn send(&self, sub: &str, body: &[u8]) -> Result<(), ChannelError> {
        self.message.send(&format!("{}{sub}", self.kind.scope_prefix(&self.scenario_id)), body)
    }

    /// Feature bytes on `<prefix><sub>`.
    pub fn register(&self, sub: &str) -> mpsc::UnboundedReceiver<super::channel::message::InboundMessage> {
        self.message.register(format!("{}{sub}", self.kind.scope_prefix(&self.scenario_id)))
    }

    /// Closes every child.
    pub fn close(&self) {
        self.message.close();
        if let Some(b) = &self.blob {
            b.close();
        }
        if let Some(v) = &self.video {
            v.close();
        }
        if let Some(i) = &self.input {
            i.close();
        }
        if let Some(a) = &self.audio {
            a.close();
        }
    }
}

/// A feature's messages on one sub-scope of a platform channel.
pub type Listener = mpsc::UnboundedReceiver<super::channel::message::InboundMessage>;

/// A creation request waiting for the phone's result.
struct PendingResult {
    channel_id: String,
    /// `<contextSourceId>_<trace_id>`: the match when the result carries no known channel id.
    trace_key: String,
    tx: oneshot::Sender<ProtoPeerPlatformChannelCreationResult>,
}

type PendingResults = Arc<Mutex<Vec<PendingResult>>>;

/// A pending platform channel waiting for its Nano channels.
struct Claim {
    scenario: String,
    consumer: String,
    expected: Vec<ChannelType>,
    tx: mpsc::UnboundedSender<Result<RawChannel, PlatformChannelError>>,
}

#[derive(Default)]
struct Dispatch {
    claims: Vec<Claim>,
    /// Channels no platform channel claimed; kept, neither opened nor closed.
    unclaimed: Vec<RawChannel>,
}

impl Dispatch {
    fn offer(&mut self, channel: RawChannel) {
        let a = &channel.announcement;
        let Some(kind) = a.channel_type else {
            warn!(created_type = a.created_type, "Nano channel of an unknown type left unclaimed");
            return self.unclaimed.push(channel);
        };

        if !kind.implemented() {
            // It is left unclaimed like a channel nothing matches.
            warn!(?kind, "Nano channel of a type the PC does not wrap left unclaimed");
            return self.unclaimed.push(channel);
        }

        let scenario = a.scenario_id().unwrap_or_default().to_string();
        let consumer = a.consumer_id().map(str::to_string);
        debug!(%scenario, ?kind, consumer = ?consumer, "Nano channel announced");

        let found = self
            .claims
            .iter_mut()
            .position(|c| c.scenario == scenario && consumer.as_ref().is_none_or(|id| *id == c.consumer));

        match found {
            Some(i) => {
                let claim = &mut self.claims[i];
                match claim.expected.iter().position(|t| *t == kind) {
                    Some(at) => {
                        claim.expected.remove(at);
                        let _ = claim.tx.send(Ok(channel));
                        if claim.expected.is_empty() {
                            self.claims.remove(i);
                        }
                    }
                    None => {
                        let _ = claim.tx.send(Err(PlatformChannelError::DuplicateChannel));
                        self.unclaimed.push(channel);
                    }
                }
            }
            None => self.unclaimed.push(channel),
        }
    }
}

/// An open device connection.
pub struct DeviceConnection {
    pub partner: DcgClient,
    pub connection: NanoConnection,
    message: MessageChannel,
    dispatch: Arc<Mutex<Dispatch>>,
    results: PendingResults,
    closed: watch::Receiver<bool>,
    config: NanoConfig,
}

impl std::fmt::Debug for DeviceConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceConnection").field("partner", &self.partner).finish()
    }
}

impl DeviceConnection {
    /// Completes an opened connection: waits for the phone's `DeviceConnection` message channel and
    /// opens it; later channels go to the platform channels that claim them.
    pub async fn complete(
        partner: DcgClient,
        connection: NanoConnection,
        config: NanoConfig,
    ) -> Result<Self, AttemptError> {
        let mut channels = connection.channels();
        let dispatch = Arc::new(Mutex::new(Dispatch::default()));
        let mut early = Vec::new();

        let raw = loop {
            let Some(channel) = channels.recv().await else {
                return Err(AttemptError::Completion("the connection closed before its message channel".into()));
            };

            let own = channel.announcement.channel_type == Some(ChannelType::Message)
                && channel.announcement.scenario_id() == Some(property::DEVICE_CONNECTION);
            if own {
                break channel;
            }
            early.push(channel);
        };

        let message = MessageChannel::open(raw).await.map_err(|e| AttemptError::Completion(e.to_string()))?;

        {
            let mut d = dispatch.lock_or_panic();
            for c in early {
                d.offer(c);
            }
        }

        let d = dispatch.clone();
        tokio::spawn(async move {
            while let Some(channel) = channels.recv().await {
                let is_second_own = channel.announcement.channel_type == Some(ChannelType::Message)
                    && channel.announcement.scenario_id() == Some(property::DEVICE_CONNECTION);
                if is_second_own {
                    warn!("second DeviceConnection message channel ignored");
                    continue;
                }

                d.lock_or_panic().offer(channel);
            }
        });

        let results: PendingResults = Arc::default();
        let mut inbound = message.register(CREATION_RESULT_TARGET);
        let r = results.clone();
        let source = partner.id.clone();

        tokio::spawn(async move {
            while let Some(m) = inbound.recv().await {
                let result = match ProtoPeerPlatformChannelCreationResult::decode(m.body.as_slice()) {
                    Ok(result) => result,
                    Err(e) => {
                        warn!(error = %e, "platform channel creation result that does not parse dropped");
                        continue;
                    }
                };

                let trace_key = result.trace_context.as_ref().map(|t| format!("{source}_{}", t.trace_id));
                let mut pending = r.lock_or_panic();
                let at = pending
                    .iter()
                    .position(|p| !result.channel_id.is_empty() && p.channel_id == result.channel_id)
                    .or_else(|| pending.iter().position(|p| Some(&p.trace_key) == trace_key.as_ref()));

                match at.map(|i| pending.remove(i)) {
                    Some(p) => {
                        let _ = p.tx.send(result);
                    }
                    // A request that timed out or failed forgets its wait: its result comes late.
                    None => {
                        debug!(channel_id = %result.channel_id, "platform channel creation result for no pending request dropped")
                    }
                }
            }
        });

        let closed = watch_closed(partner.clone(), &message, &connection);
        info!(partner = %partner, "device connection open");
        Ok(Self { partner, connection, message, dispatch, results, closed, config })
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow() || self.connection.is_closed()
    }

    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    pub fn close(&self) {
        self.message.close();
        self.connection.close();
    }

    /// Creates platform channel `kind` for `scenario` and opens its Nano channels; `trace` is the
    /// operation's trace context (the request carries a child's ids, without its trace state).
    pub async fn create_platform_channel(
        &self,
        kind: PlatformChannelType,
        scenario: &str,
        additional_properties: HashMap<String, String>,
        trace: &TraceContext,
    ) -> Result<PlatformChannel, PlatformChannelError> {
        Ok(self.create_platform_channel_listening(kind, scenario, additional_properties, &[], trace).await?.0)
    }

    /// [`DeviceConnection::create_platform_channel`], with the feature's messages on each of `subs`
    /// (`<prefix><sub>`, as [`PlatformChannel::register`]) received from the moment the Message child
    /// opens: the phone may send as soon as its side is open.
    pub async fn create_platform_channel_listening(
        &self,
        kind: PlatformChannelType,
        scenario: &str,
        additional_properties: HashMap<String, String>,
        subs: &[&str],
        trace: &TraceContext,
    ) -> Result<(PlatformChannel, HashMap<String, Listener>), PlatformChannelError> {
        if self.is_closed() {
            return Err(PlatformChannelError::DeviceConnectionFailed);
        }

        let channel_id = new_guid_lower();
        // A child of the operation's context; the request carries its ids only.
        let trace = TraceContext { trace_state: Vec::new(), ..trace.child() };
        let mut claimed = self.claim_channels(kind, scenario, &channel_id);
        let result_rx = self.expect_creation_result(&channel_id, &trace);

        let request = ProtoCreatePlatformChannelRequest {
            r#type: kind.wire() as i32,
            scenario_id: scenario.to_string(),
            channel_id: channel_id.clone(),
            trace_context: Some(proto_trace(&trace)),
            additional_properties,
        };
        if self.message.send(CREATE_REQUEST_TARGET, &request.encode_to_vec()).is_err() {
            self.forget_channel(&channel_id);
            return Err(PlatformChannelError::DeviceConnectionFailed);
        }

        let timeout = self.config.platform_channel_creation_timeout;
        let result = match self.await_creation_result(timeout, result_rx).await {
            Ok(r) => r,
            Err(e) => {
                self.forget_channel(&channel_id);
                return Err(e);
            }
        };

        if let Some(e) = creation_failure(&result) {
            self.forget_channel(&channel_id);
            return Err(e);
        }

        let opened =
            tokio::time::timeout(timeout, self.collect_and_open(kind, scenario, &channel_id, &mut claimed, subs)).await;
        match opened {
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => {
                self.forget_channel(&channel_id);
                Err(e)
            }
            Err(_) => {
                self.forget_channel(&channel_id);
                Err(PlatformChannelError::Timeout)
            }
        }
    }

    /// Claims the Nano channels the phone announces for `channel_id`, offering the ones announced
    /// before the claim existed; the receiver of the claimed channels.
    fn claim_channels(
        &self,
        kind: PlatformChannelType,
        scenario: &str,
        channel_id: &str,
    ) -> mpsc::UnboundedReceiver<Result<RawChannel, PlatformChannelError>> {
        let (claim_tx, claimed) = mpsc::unbounded_channel();
        let mut d = self.dispatch.lock_or_panic();
        d.claims.push(Claim {
            scenario: scenario.to_string(),
            consumer: channel_id.to_string(),
            expected: kind.expected().to_vec(),
            tx: claim_tx,
        });

        // Channels announced before the claim existed.
        let waiting = std::mem::take(&mut d.unclaimed);
        for c in waiting {
            d.offer(c);
        }

        claimed
    }

    /// Registers the wait for the phone's creation result of `channel_id`.
    fn expect_creation_result(
        &self,
        channel_id: &str,
        trace: &TraceContext,
    ) -> oneshot::Receiver<ProtoPeerPlatformChannelCreationResult> {
        let (result_tx, result_rx) = oneshot::channel();
        self.results.lock_or_panic().push(PendingResult {
            channel_id: channel_id.to_string(),
            trace_key: format!("{}_{}", self.partner.id, trace.trace_id),
            tx: result_tx,
        });
        result_rx
    }

    /// Waits up to `timeout` for the creation result, or until the connection closes.
    async fn await_creation_result(
        &self,
        timeout: Duration,
        result_rx: oneshot::Receiver<ProtoPeerPlatformChannelCreationResult>,
    ) -> Result<ProtoPeerPlatformChannelCreationResult, PlatformChannelError> {
        let mut closed = self.closed.clone();
        let result = tokio::select! {
            r = tokio::time::timeout(timeout, result_rx) => r,
            _ = closed.wait_for(|c| *c) => return Err(PlatformChannelError::DeviceConnectionFailed),
        };

        match result {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err(PlatformChannelError::DeviceConnectionFailed),
            Err(_) => Err(PlatformChannelError::CreateRequestTimedOut),
        }
    }

    /// Drops the pending result and the claim of a platform channel whose creation failed.
    fn forget_channel(&self, channel_id: &str) {
        self.results.lock_or_panic().retain(|p| p.channel_id != channel_id);
        self.dispatch.lock_or_panic().claims.retain(|c| c.consumer != channel_id);
    }

    async fn collect_and_open(
        &self,
        kind: PlatformChannelType,
        scenario: &str,
        channel_id: &str,
        claimed: &mut mpsc::UnboundedReceiver<Result<RawChannel, PlatformChannelError>>,
        subs: &[&str],
    ) -> Result<(PlatformChannel, HashMap<String, Listener>), PlatformChannelError> {
        // Each child is opened as soon as it is announced: the phone creates a platform channel's
        // children one after another, the next once the previous one is open.
        let prefix = kind.scope_prefix(scenario);
        let targets: Vec<String> = subs.iter().map(|sub| format!("{prefix}{sub}")).collect();
        let expected = kind.expected().len();
        let mut announced: HashSet<ChannelType> = HashSet::new();
        let mut opening: FuturesUnordered<BoxFuture<'static, Result<OpenedChild, ChannelError>>> =
            FuturesUnordered::new();
        let (mut message, mut listening) = (None, HashMap::new());
        let (mut blob, mut video, mut input, mut audio) = (None, None, None, None);

        while announced.len() < expected || !opening.is_empty() {
            tokio::select! {
                c = claimed.recv(), if announced.len() < expected => {
                    let c = c.ok_or(PlatformChannelError::DeviceConnectionFailed)??;
                    let t = c.announcement.channel_type.expect("claimed channels have a known type");
                    if !announced.insert(t) {
                        return Err(PlatformChannelError::DuplicateChannel);
                    }
                    opening.push(open_child(t, c, targets.clone()));
                }
                Some(opened) = opening.next(), if !opening.is_empty() => match opened? {
                    OpenedChild::Message(m, l) => (message, listening) = (Some(m), l),
                    OpenedChild::Blob(b) => blob = Some(b),
                    OpenedChild::Video(v) => video = Some(v),
                    OpenedChild::Input(i) => input = Some(i),
                    OpenedChild::Audio(a) => audio = Some(a),
                },
            }
        }

        let message = message.expect("every platform channel has a message channel");
        debug!(?kind, %scenario, "platform channel open");

        // Keyed by the feature's own sub-scope.
        let mut listening = listening;
        let listeners = subs
            .iter()
            .map(|sub| (sub.to_string(), listening.remove(&format!("{prefix}{sub}")).expect("registered above")))
            .collect();

        let channel = PlatformChannel {
            kind,
            scenario_id: scenario.to_string(),
            channel_id: channel_id.to_string(),
            message,
            blob,
            video,
            input,
            audio,
        };
        Ok((channel, listeners))
    }
}

/// One opened child of a platform channel.
enum OpenedChild {
    Message(MessageChannel, HashMap<String, Listener>),
    Blob(BlobChannel),
    Video(VideoTarget),
    Input(InputSource),
    Audio(AudioTarget),
}

/// Opens the child `channel` of type `t`; a Message child listens to `targets` from its opening on.
fn open_child(
    t: ChannelType,
    channel: RawChannel,
    targets: Vec<String>,
) -> BoxFuture<'static, Result<OpenedChild, ChannelError>> {
    async move {
        Ok(match t {
            ChannelType::Message => {
                let (m, l) = MessageChannel::open_listening(channel, &targets).await?;
                OpenedChild::Message(m, l)
            }
            ChannelType::Blob => OpenedChild::Blob(BlobChannel::open(channel).await?),
            ChannelType::VideoTarget => OpenedChild::Video(VideoTarget::open(channel).await?),
            ChannelType::InputSource => OpenedChild::Input(InputSource::open(channel).await?),
            ChannelType::AudioTarget => OpenedChild::Audio(AudioTarget::open(channel).await?),
            other => unreachable!("{other:?} is not a type a platform channel expects"),
        })
    }
    .boxed()
}

/// Losing the message channel closes the device connection: the watch becomes `true` once the
/// `DeviceConnection` message channel or the Nano connection closed.
fn watch_closed(partner: DcgClient, message: &MessageChannel, connection: &NanoConnection) -> watch::Receiver<bool> {
    let (closed_tx, closed) = watch::channel(false);
    let mut message_closed = message.closed();
    let mut connection_closed = connection.closed();

    tokio::spawn(async move {
        tokio::select! {
            _ = message_closed.wait_for(|c| *c) => {}
            _ = connection_closed.wait_for(Option::is_some) => {}
        }

        // A closed Nano connection logs its own close (and closes the message channel with it);
        // only a message channel that closed on its own is this layer's to tell.
        if connection_closed.borrow().is_some() {
            debug!(%partner, "device connection closed with its Nano connection");
        } else {
            info!(%partner, "device connection closed; its DeviceConnection message channel closed");
        }

        let _ = closed_tx.send(true);
    });
    closed
}

type Opening = Shared<BoxFuture<'static, Result<Arc<DeviceConnection>, DeviceConnectionError>>>;

/// One device connection per partner: a second request reuses the open one, concurrent requests join
/// the one opening.
pub struct DeviceConnections {
    config: NanoConfig,
    relay: Arc<dyn RelaySource>,
    hosts: Option<Vec<HostAddress>>,
    /// The `cs_initialize_message` transport preference (0 = none).
    transport_preference: i32,
    open: Mutex<HashMap<DcgClient, Weak<DeviceConnection>>>,
    opening: Mutex<HashMap<DcgClient, Opening>>,
}

impl DeviceConnections {
    pub fn new(config: NanoConfig, relay: Arc<dyn RelaySource>, hosts: Option<Vec<HostAddress>>) -> Arc<Self> {
        Arc::new(Self {
            config,
            relay,
            hosts,
            transport_preference: 0,
            open: Mutex::default(),
            opening: Mutex::default(),
        })
    }

    /// The device connection on `handle`'s phone: the open one, the one opening, or a new one.
    pub async fn get_or_open(
        self: &Arc<Self>,
        handle: &ContextHandle,
    ) -> Result<Arc<DeviceConnection>, DeviceConnectionError> {
        if handle.is_dropped() {
            return Err(DeviceConnectionError::ContextDropped);
        }

        let partner = handle.partner.clone();
        if let Some(c) = self.open.lock_or_panic().get(&partner).and_then(Weak::upgrade) {
            if !c.is_closed() {
                return Ok(c);
            }
        }

        let opening = {
            let mut opening = self.opening.lock_or_panic();
            match opening.get(&partner) {
                Some(o) => o.clone(),
                None => {
                    let me = self.clone();
                    let h = handle.clone();
                    let fut: BoxFuture<'static, _> = async move { me.open_now(h).await.map(Arc::new) }.boxed();
                    let shared = fut.shared();
                    opening.insert(partner.clone(), shared.clone());
                    shared
                }
            }
        };

        let result = opening.await;
        self.opening.lock_or_panic().remove(&partner);
        if let Ok(c) = &result {
            self.open.lock_or_panic().insert(partner, Arc::downgrade(c));
        }
        result
    }

    async fn open_now(&self, handle: ContextHandle) -> Result<DeviceConnection, DeviceConnectionError> {
        let dropped = handle.dropped();
        let run = async {
            // The phone's Initialize ack has its own bound; the device-connection timeout starts
            // after it and covers the Nano connection through the open `DeviceConnection` channel.
            initialize_context(&handle, self.transport_preference, self.config.initialize_ack_timeout).await?;

            let connect = async {
                let attempts = device_attempts(self.relay.as_ref()).await;
                debug!(attempt_count = attempts.len(), "device connection race");
                let exchange = Arc::new(ContextExchange { handle: handle.clone() });
                let partner = handle.partner.clone();
                let config = self.config.clone();
                race(attempts, exchange, self.hosts.clone(), self.config.attempt_timeout, |c| {
                    DeviceConnection::complete(partner.clone(), c, config.clone())
                })
                .await
                .map_err(DeviceConnectionError::Attempt)
            };

            tokio::time::timeout(self.config.device_connection_timeout, connect)
                .await
                .unwrap_or(Err(DeviceConnectionError::ChannelOpenTimeout))
        };

        tokio::select! {
            r = run => r,
            _ = dropped.cancelled() => Err(DeviceConnectionError::ContextDropped),
        }
    }
}

#[cfg(feature = "test-support")]
/// `CloseReason` of a connection the device-connection path treats as lost.
pub fn is_lost(reason: &CloseReason) -> bool {
    !matches!(reason, CloseReason::UserEnded | CloseReason::UserCanceled)
}

/// The error of a creation result other than `Success`.
fn creation_failure(result: &ProtoPeerPlatformChannelCreationResult) -> Option<PlatformChannelError> {
    let reason = AcceptanceReason::from_wire(result.result_reason);
    match ProtoCreatePlatformChannelStatus::try_from(result.result_code) {
        Ok(ProtoCreatePlatformChannelStatus::Success) => None,
        Ok(ProtoCreatePlatformChannelStatus::NoRemoteScenarioAvailable) => {
            Some(PlatformChannelError::NoRemoteScenarioAvailable(reason))
        }
        Ok(ProtoCreatePlatformChannelStatus::RemoteScenarioRejectedChannel) => {
            Some(PlatformChannelError::RemoteScenarioRejectedChannel(reason))
        }
        _ => Some(PlatformChannelError::PartnerChannelCreationFailure(reason)),
    }
}

#[cfg(test)]
mod tests;
