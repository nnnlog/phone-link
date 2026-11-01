//! Where a stream's `VideoStreaming` channel comes from: [`CameraChannels`] creates and opens one against
//! the phone's context handle.
//!
//! [`NanoCameraChannels`] does it over the Nano device connection shared
//! with the other features on the handle; the tests hand in channels of their own.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use phonelink_core::types::TraceContext;
use phonelink_features::shared::nano_channels::{ChannelOpenError, NanoShareChannels};
use phonelink_nano::channel::video::{VideoEvent, VideoFrame, VideoTarget};
use phonelink_nano::device::{DeviceConnection, PlatformChannel, PlatformChannelError, PlatformChannelType};
use phonelink_transport::platform::ContextHandle;

use super::wire;

/// What arrives on an open stream channel.
#[derive(Debug)]
pub enum StreamInbound {
    /// A control message on `scope` (the channel's prefix stripped).
    Message { scope: String, body: Vec<u8> },
    /// A reassembled video frame.
    Frame(VideoFrame),
    /// The channel or its device connection closed.
    Closed,
}

/// Why a control message was not sent; its text is the cause's.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StreamSendError(#[source] pub Box<dyn std::error::Error + Send + Sync>);

/// An open stream channel.
pub trait StreamChannel: Send + Sync {
    /// Sends `body` on control scope `scope`.
    fn send(&self, scope: &str, body: &[u8]) -> Result<(), StreamSendError>;
    /// Starts the video: the handshake's `StartStream`, or as soon as the handshake completes.
    fn start_video(&self);
    /// Acknowledges `frame_id` as presented at `time_us` (monotonic µs).
    fn frame_presented(&self, frame_id: u32, time_us: u64);
    /// The device connection's real-time metrics (JSON).
    fn realtime_metrics(&self) -> Option<String>;
    /// Closes the channel without a message.
    fn close(&self);
}

/// An open stream channel and what arrives on it.
pub struct OpenedStream {
    pub channel: Arc<dyn StreamChannel>,
    pub inbound: mpsc::UnboundedReceiver<StreamInbound>,
}

/// Why a stream channel could not be had.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamOpenError {
    /// The channel was not created: no device connection, or the phone did not accept it.
    #[error("creation: {0}")]
    Creation(ChannelOpenError),
    /// The phone accepted the channel, but its Nano channels did not open.
    #[error("open: {0}")]
    Open(ChannelOpenError),
}

impl StreamOpenError {
    /// The phone's acceptance status, when it refused the channel.
    pub fn acceptance(&self) -> Option<phonelink_nano::device::AcceptanceReason> {
        match self {
            Self::Creation(e) | Self::Open(e) => e.acceptance(),
        }
    }
}

/// Creates and opens a stream's `VideoStreaming` channel.
#[async_trait]
pub trait CameraChannels: Send + Sync {
    /// Creates the stream channel against `handle`'s phone and opens it, receiving its control
    /// messages from the moment it opened; `trace` is the stream's trace context.
    async fn open(&self, handle: &ContextHandle, trace: &TraceContext) -> Result<OpenedStream, StreamOpenError>;
}

/// [`CameraChannels`] over the Nano device connection.
pub struct NanoCameraChannels {
    nano: Arc<NanoShareChannels>,
}

impl NanoCameraChannels {
    pub fn new(nano: Arc<NanoShareChannels>) -> Arc<Self> {
        Arc::new(Self { nano })
    }
}

#[async_trait]
impl CameraChannels for NanoCameraChannels {
    async fn open(&self, handle: &ContextHandle, trace: &TraceContext) -> Result<OpenedStream, StreamOpenError> {
        let device = self
            .nano
            .connections(handle.partner.env)
            .get_or_open(handle)
            .await
            .map_err(|e| StreamOpenError::Creation(e.into()))?;

        let opened = device
            .create_platform_channel_listening(
                PlatformChannelType::VideoStreaming,
                wire::SCENARIO,
                HashMap::new(),
                wire::scope::INBOUND,
                trace,
            )
            .await;

        let (mut channel, listeners) = match opened {
            Ok(c) => c,
            // The phone accepted the channel; its children did not open.
            Err(
                e @ (PlatformChannelError::Timeout
                | PlatformChannelError::DuplicateChannel
                | PlatformChannelError::Channel(_)),
            ) => {
                return Err(StreamOpenError::Open(e.into()));
            }
            Err(e) => return Err(StreamOpenError::Creation(e.into())),
        };

        let video = channel.video.take().expect("a VideoStreaming channel has a video target");
        Ok(NanoStream::start(channel, video, listeners, device))
    }
}

/// What the video task is asked to do.
enum VideoCommand {
    Start,
    Presented(u32, u64),
    Close,
}

/// A stream channel over Nano: the platform channel, its video target run by a task, and the device
/// connection it holds open.
struct NanoStream {
    channel: PlatformChannel,
    video: mpsc::UnboundedSender<VideoCommand>,
    device: Arc<DeviceConnection>,
}

impl NanoStream {
    fn start(
        channel: PlatformChannel,
        mut video: VideoTarget,
        listeners: HashMap<String, phonelink_nano::device::Listener>,
        device: Arc<DeviceConnection>,
    ) -> OpenedStream {
        let (tx, inbound) = mpsc::unbounded_channel();
        for (scope, mut listener) in listeners {
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Some(m) = listener.recv().await {
                    if tx.send(StreamInbound::Message { scope: scope.clone(), body: m.body }).is_err() {
                        return;
                    }
                }
            });
        }

        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let frames = tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(VideoCommand::Start) => video.start(),
                        Some(VideoCommand::Presented(id, at)) => video.frame_presented(id, at),
                        Some(VideoCommand::Close) | None => {
                            video.close();
                            return;
                        }
                    },
                    event = video.next_event() => match event {
                        Some(VideoEvent::Frame(f)) => {
                            if frames.send(StreamInbound::Frame(f)).is_err() {
                                return;
                            }
                        }
                        Some(VideoEvent::Ready { format, .. }) => debug!(?format, "camera video ready"),
                        Some(VideoEvent::FormatChanged(format)) => debug!(?format, "camera video format changed"),
                        Some(VideoEvent::BitrateChanged(bps)) => debug!(bps, "camera video bitrate changed"),
                        Some(VideoEvent::Closed) | None => {
                            let _ = frames.send(StreamInbound::Closed);
                            return;
                        }
                    },
                }
            }
        });

        let mut message_closed = channel.message.closed();
        let mut device_closed = device.closed();
        tokio::spawn(async move {
            tokio::select! {
                _ = message_closed.wait_for(|c| *c) => {}
                _ = device_closed.wait_for(|c| *c) => {}
            }
            let _ = tx.send(StreamInbound::Closed);
        });

        let stream = Arc::new(NanoStream { channel, video: commands, device });
        OpenedStream { channel: stream, inbound }
    }
}

impl StreamChannel for NanoStream {
    fn send(&self, scope: &str, body: &[u8]) -> Result<(), StreamSendError> {
        self.channel.send(scope, body).map_err(|e| StreamSendError(Box::new(e)))
    }

    fn start_video(&self) {
        if self.video.send(VideoCommand::Start).is_err() {
            warn!("camera video closed before its Start; the video does not start");
        }
    }

    fn frame_presented(&self, frame_id: u32, time_us: u64) {
        let _ = self.video.send(VideoCommand::Presented(frame_id, time_us));
    }

    fn realtime_metrics(&self) -> Option<String> {
        Some(self.device.connection.realtime_metrics())
    }

    fn close(&self) {
        let _ = self.video.send(VideoCommand::Close);
        self.channel.close();
    }
}
