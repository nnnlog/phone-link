//! The Nano channels inside a connection.
//!
//! It covers how the phone announces each one, the raw channel handle the connection gives its
//! owner, and the five channel kinds the PC uses — Message, Blob, Video (the PC receives), Input
//! (the PC sends) and Audio (the PC receives).
//!
//! Every multi-byte integer in a
//! channel packet is little-endian.

pub mod audio;
pub mod blob;
pub mod input;
pub mod message;
pub mod video;

use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::transport::SendMode;

/// Payload type of Message, Blob, Input and Video packets; Audio adds its packet type.
pub const PT_CHANNEL: u8 = 0x23;

/// Nano channel types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChannelType {
    VideoTarget = 0,
    VideoSource = 1,
    InputTarget = 2,
    InputSource = 3,
    Message = 4,
    Blob = 5,
    AudioTarget = 6,
    AudioSource = 7,
}

impl ChannelType {
    pub fn from_wire(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::VideoTarget,
            1 => Self::VideoSource,
            2 => Self::InputTarget,
            3 => Self::InputSource,
            4 => Self::Message,
            5 => Self::Blob,
            6 => Self::AudioTarget,
            7 => Self::AudioSource,
            _ => return None,
        })
    }

    /// The PC's counterpart of a type the phone created: 0↔1, 2↔3, 6↔7; 4 and 5 unchanged.
    pub fn counterpart(self) -> Self {
        match self {
            Self::VideoTarget => Self::VideoSource,
            Self::VideoSource => Self::VideoTarget,
            Self::InputTarget => Self::InputSource,
            Self::InputSource => Self::InputTarget,
            Self::AudioTarget => Self::AudioSource,
            Self::AudioSource => Self::AudioTarget,
            other => other,
        }
    }

    /// A type the PC wraps.
    pub fn implemented(self) -> bool {
        matches!(self, Self::VideoTarget | Self::InputSource | Self::Message | Self::Blob | Self::AudioTarget)
    }
}

/// Property keys of an announcement.
pub mod property {
    pub const SCENARIO_ID: &str = "ScenarioId";
    pub const CONSUMER_ID: &str = "ConsumerId";
    pub const SYNC_ID: &str = "syncId";
    /// The `ScenarioId` of the connection's own message channel.
    pub const DEVICE_CONNECTION: &str = "DeviceConnection";
}

/// A failure parsing an announcement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AnnouncementError {
    #[error("InvalidChannelInformation: {0:?}")]
    InvalidChannelInformation(String),
}

/// The class-name contract the phone announces a channel with:
/// `<class>,<channelId>,<type>[,<key=value&…>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    pub class: String,
    pub nano_channel_id: String,
    /// The type as the phone created it.
    pub created_type: u32,
    /// The PC's counterpart type, when the value is known.
    pub channel_type: Option<ChannelType>,
    pub properties: BTreeMap<String, String>,
}

impl Announcement {
    pub fn parse(key: &str) -> Result<Self, AnnouncementError> {
        let mut parts = key.splitn(4, ',');
        let (Some(class), Some(id), Some(kind)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(AnnouncementError::InvalidChannelInformation(key.to_string()));
        };

        let created_type =
            kind.trim().parse::<u32>().map_err(|_| AnnouncementError::InvalidChannelInformation(key.to_string()))?;

        let mut properties = BTreeMap::new();
        if let Some(props) = parts.next() {
            for pair in props.split('&').filter(|p| !p.is_empty()) {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                properties.insert(k.to_string(), v.to_string());
            }
        }

        Ok(Self {
            class: class.to_string(),
            nano_channel_id: id.to_string(),
            created_type,
            channel_type: ChannelType::from_wire(created_type).map(ChannelType::counterpart),
            properties,
        })
    }

    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    pub fn scenario_id(&self) -> Option<&str> {
        self.property(property::SCENARIO_ID)
    }

    pub fn consumer_id(&self) -> Option<&str> {
        self.property(property::CONSUMER_ID)
    }
}

/// A channel-level failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChannelError {
    #[error("the channel closed")]
    Closed,
    #[error("the Nano connection is gone")]
    ConnectionGone,
    #[error("malformed handshake: {0}")]
    Handshake(String),
    #[error("{0}")]
    Transport(String),
    #[error("{0}")]
    Invalid(String),
}

impl ChannelError {
    /// The channel or its connection is gone: a send failing so is a consequence of the close,
    /// which is logged where it happens.
    pub fn is_gone(&self) -> bool {
        matches!(self, ChannelError::Closed | ChannelError::ConnectionGone)
    }
}

/// Logs a channel packet the channel did not take: at `debug` when the channel or its connection is
/// gone (a close logged where it happens), else at `warn`.
pub(crate) fn log_not_sent(channel_kind: &'static str, error: &ChannelError) {
    if error.is_gone() {
        tracing::debug!(channel_kind, %error, "channel packet not sent; the channel is gone");
    } else {
        tracing::warn!(channel_kind, %error, "channel packet not sent");
    }
}

/// What the connection delivers to one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawInbound {
    /// Both sides sent Open.
    Opened,
    Data {
        payload_type: u8,
        payload: Vec<u8>,
    },
    /// The phone closed the channel, or the connection ended.
    Closed,
}

/// The connection side of a channel handle.
pub trait ChannelLink: Send + Sync {
    fn open(&self, id: u16) -> Result<(), ChannelError>;
    fn send(&self, id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ChannelError>;
    fn close(&self, id: u16);
    /// The largest payload one packet carries now.
    fn max_payload(&self) -> usize;
    /// How full the transport's send path is (0–1).
    fn send_queue_fill(&self) -> f64;
    /// The transport's sending rate (bit/s).
    fn bandwidth_bps(&self) -> f64;
}

/// One Nano channel as the connection hands it to its owner.
pub struct RawChannel {
    pub id: u16,
    pub announcement: Announcement,
    link: Arc<dyn ChannelLink>,
    inbound: mpsc::UnboundedReceiver<RawInbound>,
}

impl std::fmt::Debug for RawChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawChannel").field("id", &self.id).field("announcement", &self.announcement).finish()
    }
}

impl RawChannel {
    pub fn new(
        id: u16,
        announcement: Announcement,
        link: Arc<dyn ChannelLink>,
        inbound: mpsc::UnboundedReceiver<RawInbound>,
    ) -> Self {
        Self { id, announcement, link, inbound }
    }

    /// Sends Open and waits until both sides opened. Data that arrives first stays queued.
    pub async fn open(&mut self) -> Result<Vec<RawInbound>, ChannelError> {
        self.link.open(self.id)?;

        let mut early = Vec::new();
        loop {
            match self.inbound.recv().await {
                Some(RawInbound::Opened) => return Ok(early),
                Some(RawInbound::Closed) | None => return Err(ChannelError::Closed),
                Some(data) => early.push(data),
            }
        }
    }

    pub fn sender(&self) -> ChannelSender {
        ChannelSender { id: self.id, link: self.link.clone() }
    }

    pub async fn recv(&mut self) -> Option<RawInbound> {
        self.inbound.recv().await
    }

    /// Splits the handle into its sending side and its inbound queue.
    pub fn split(self) -> (ChannelSender, Announcement, mpsc::UnboundedReceiver<RawInbound>) {
        (ChannelSender { id: self.id, link: self.link }, self.announcement, self.inbound)
    }

    pub fn close(&self) {
        self.link.close(self.id);
    }
}

/// The sending side of a channel; cheap to clone.
#[derive(Clone)]
pub struct ChannelSender {
    id: u16,
    link: Arc<dyn ChannelLink>,
}

impl ChannelSender {
    pub fn id(&self) -> u16 {
        self.id
    }

    pub fn send(&self, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ChannelError> {
        self.link.send(self.id, mode, payload_type, body)
    }

    pub fn max_payload(&self) -> usize {
        self.link.max_payload()
    }

    pub fn send_queue_fill(&self) -> f64 {
        self.link.send_queue_fill()
    }

    pub fn bandwidth_bps(&self) -> f64 {
        self.link.bandwidth_bps()
    }

    pub fn close(&self) {
        self.link.close(self.id);
    }
}

/// `u32 type, u32 bodyLength` framing of Message, Blob and Input packets.
pub fn frame(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    out
}

/// Splits an 8-byte-header packet into its type and body.
pub fn unframe(packet: &[u8]) -> Result<(u32, &[u8]), ChannelError> {
    let mut r = Reader::new(packet);
    let kind = r.u32()?;
    let len = r.u32()? as usize;
    Ok((kind, r.take(len)?))
}

/// Little-endian reader of channel packets.
pub struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], ChannelError> {
        let out = self
            .data
            .get(self.at..self.at.checked_add(n).ok_or_else(|| ChannelError::Invalid("length overflow".into()))?)
            .ok_or_else(|| ChannelError::Invalid("truncated channel packet".into()))?;
        self.at += n;
        Ok(out)
    }
    pub fn u8(&mut self) -> Result<u8, ChannelError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, ChannelError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2")))
    }
    pub fn u32(&mut self) -> Result<u32, ChannelError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    pub fn i32(&mut self) -> Result<i32, ChannelError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("4")))
    }
    pub fn u64(&mut self) -> Result<u64, ChannelError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().expect("8")))
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.at
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.data[self.at..];
        self.at = self.data.len();
        out
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    #![allow(clippy::unwrap_used, reason = "test support")]
    //! An in-memory channel pair: what one end sends, the other receives (unit tests, and the
    //! integration tests that run a feature over channels without a connection).
    use phonelink_core::sync::MutexExt;
    use std::sync::Mutex;

    use super::{Announcement, ChannelError, ChannelLink, RawChannel, RawInbound, SendMode};

    use std::sync::Arc;

    use tokio::sync::mpsc;

    pub struct Loopback {
        peer: Mutex<Option<mpsc::UnboundedSender<RawInbound>>>,
        pub sent: Mutex<Vec<(SendMode, u8, Vec<u8>)>>,
        pub max: usize,
    }

    impl ChannelLink for Loopback {
        fn open(&self, _id: u16) -> Result<(), ChannelError> {
            Ok(())
        }
        fn send(&self, _id: u16, mode: SendMode, payload_type: u8, body: Vec<u8>) -> Result<(), ChannelError> {
            self.sent.lock_or_panic().push((mode, payload_type, body.clone()));
            if let Some(peer) = self.peer.lock_or_panic().as_ref() {
                let _ = peer.send(RawInbound::Data { payload_type, payload: body });
            }
            Ok(())
        }
        fn close(&self, _id: u16) {
            if let Some(peer) = self.peer.lock_or_panic().take() {
                let _ = peer.send(RawInbound::Closed);
            }
        }
        fn max_payload(&self) -> usize {
            self.max
        }
        fn send_queue_fill(&self) -> f64 {
            0.0
        }
        fn bandwidth_bps(&self) -> f64 {
            80_000_000.0
        }
    }

    /// Two opened channels wired to each other, announced as `announcement`.
    pub fn pair(announcement: &str, max: usize) -> ((RawChannel, Arc<Loopback>), (RawChannel, Arc<Loopback>)) {
        let (tx_a, rx_a) = mpsc::unbounded_channel();
        let (tx_b, rx_b) = mpsc::unbounded_channel();
        tx_a.send(RawInbound::Opened).unwrap();
        tx_b.send(RawInbound::Opened).unwrap();

        let a = Arc::new(Loopback { peer: Mutex::new(Some(tx_b)), sent: Mutex::default(), max });
        let b = Arc::new(Loopback { peer: Mutex::new(Some(tx_a)), sent: Mutex::default(), max });

        let ann = Announcement::parse(announcement).unwrap();
        ((RawChannel::new(1, ann.clone(), a.clone(), rx_a), a), (RawChannel::new(1, ann, b.clone(), rx_b), b))
    }
}

#[cfg(test)]
mod tests;
