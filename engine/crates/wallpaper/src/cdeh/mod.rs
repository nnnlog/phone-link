//! The Cross Device Experience Host (CDEH) contract.
//!
//! It covers the phone wallpaper pull (`GET` by type `WALLPAPER` with `WallpaperRequestMessage`),
//! the `/wallpaper` receiver that answers the Mac's desktop picture, and the MSAEP tag-5
//! subscription, whose observer does nothing.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::edge::{PcWallpaper, PcWallpaperAnswers, PcWallpaperSource, pc_wallpaper_response};
use super::store::{CrossDeviceChange, HOME_SCREEN, LOCK_SCREEN};
use phonelink_core::types::{DeviceInfoItem, TraceContext};
use phonelink_proto::device_info::v1::{
    WallpaperRequestMessage, WallpaperRequestMetadata, WallpaperResponseMessage, WallpaperResponseStatus, WallpaperType,
};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{
    DrmAddress, DrmReceiver, DrmReceiverError, DrmRequest, DrmSendRequest, Publication, PublicationObserver,
};

/// The receiver path of the Mac's wallpaper.
pub const PC_WALLPAPER_PATH: &str = "/wallpaper";
/// The MSAEP tag subscribed with an observer that does nothing.
pub const WALLPAPER_TAG: i16 = 5;

/// The phone's platform is Android: its device-list `osName` is `Android` (ASCII case and surrounding
/// white space ignored) or the number `2`.
pub fn is_android(info: &DeviceInfoItem) -> bool {
    info.os_name.as_deref().map(str::trim).is_some_and(|os| os.eq_ignore_ascii_case("Android") || os == "2")
}

/// The pull's payload: the stored home-screen and lock-screen ids, `-1` for a type with none.
pub fn pull_payload(home_id: Option<i64>, lock_id: Option<i64>) -> WallpaperRequestMessage {
    WallpaperRequestMessage {
        current_wallpapers: vec![
            WallpaperRequestMetadata { id: home_id.unwrap_or(-1), wallpaper_type: WallpaperType::Homescreen as i32 },
            WallpaperRequestMetadata { id: lock_id.unwrap_or(-1), wallpaper_type: WallpaperType::Lockscreen as i32 },
        ],
    }
}

/// The pull through CDEH's request wrapper: type-addressed `WALLPAPER`, `GET`, no forced session
/// start, bounded by the platform request timeout.
pub fn pull_request(
    peer: &PeerDevice,
    payload: &WallpaperRequestMessage,
    phone_enabled: bool,
    timeout: Duration,
    cancel: CancellationToken,
) -> DrmSendRequest {
    DrmSendRequest {
        target: peer.client.clone(),
        address: DrmAddress::Type(ProtoDeviceResourceType::Wallpaper),
        request_type: ProtoDeviceResourceRequestType::Get,
        payload: payload.encode_to_vec(),
        client_version: peer.info.client_version.clone(),
        phone_enabled,
        force_session_start: false,
        timeout,
        trace: TraceContext::new_root(Vec::new()),
        cancel,
    }
}

/// Why a pull reply changes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PullReplyError {
    #[error("the reply does not decode as WallpaperResponseMessage: {0}")]
    Decode(String),
    #[error("an entry's wallpaper_type {0} is neither HOMESCREEN nor LOCKSCREEN")]
    WallpaperType(i32),
}

/// What a pull reply does to the stored copy: a decode failure or an entry of another type changes
/// nothing; a status other than `OK` deletes every row; `OK` replaces the rows of the entries that
/// carry `payload`.
pub(crate) fn read_pull_reply(payload: &[u8]) -> Result<CrossDeviceChange, PullReplyError> {
    let reply = WallpaperResponseMessage::decode(payload).map_err(|e| PullReplyError::Decode(e.to_string()))?;

    let mut rows = Vec::new();
    for entry in &reply.wallpapers {
        let wallpaper_type = match WallpaperType::try_from(entry.wallpaper_type) {
            Ok(WallpaperType::Homescreen) => HOME_SCREEN,
            Ok(WallpaperType::Lockscreen) => LOCK_SCREEN,
            _ => return Err(PullReplyError::WallpaperType(entry.wallpaper_type)),
        };
        if let Some(bytes) = &entry.payload {
            rows.push((wallpaper_type, entry.id, bytes.clone()));
        }
    }

    if reply.status != WallpaperResponseStatus::Ok as i32 {
        return Ok(CrossDeviceChange::DeleteAll);
    }
    Ok(CrossDeviceChange::Upsert(rows))
}

/// The tag-5 subscriber, which ignores what arrives.
pub(crate) struct IgnoredPublications;

impl PublicationObserver for IgnoredPublications {
    fn on_publication(&self, publication: &Publication) {
        debug!(partner = %publication.source, "MSAEP tag-5 publication ignored");
    }
}

/// The `/wallpaper` receiver: answers every dispatched request — whatever its source, request type,
/// query or payload, none of which is parsed — with the Mac's desktop picture.
pub(crate) struct PcWallpaperReceiver {
    answers: Arc<PcWallpaperAnswers>,
}

impl PcWallpaperReceiver {
    pub(crate) fn new(source: Arc<dyn PcWallpaperSource>) -> Self {
        Self { answers: Arc::new(PcWallpaperAnswers::new(source)) }
    }
}

#[async_trait]
impl DrmReceiver for PcWallpaperReceiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        debug!(partner = %request.source, raw_path = %request.path, "the phone asks for the Mac's wallpaper");
        let answers = self.answers.clone();
        let answer = match tokio::task::spawn_blocking(move || answers.answer()).await {
            Ok(answer) => answer,
            Err(e) => {
                warn!(error = %e, "reading the Mac's desktop picture panicked; answered as unreadable");
                pc_wallpaper_response(PcWallpaper::Unreadable)
            }
        };
        Ok(Some(answer.encode_to_vec()))
    }
}

#[cfg(test)]
mod tests;
