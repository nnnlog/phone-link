//! The messages of new-photo notifications.
//!
//! It covers the `MEDIA` and `PERMISSIONS` requests the PC sends, how their replies are read, and
//! the two publications the phone sends (tag 6 `MediaChangeMessage`, tag 3
//! `PermissionsResponseMessage`, each in `PubSubPayload.additional`).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use prost::Message;

use phonelink_proto::permissions::v1::{
    FeaturePermissions, PermissionsRequestMessage, PermissionsRequestStatus, PermissionsResponseMessage,
};
use phonelink_proto::remotesnipping::v1::{
    MediaCapability, MediaChangeMessage, MediaChangeType, MediaMetadata, MediaRequestMessage, MediaRequestType,
    MediaResponseMessage,
};

/// The MSAEP tag of the phone's new-photo publication.
pub const NEW_PHOTO_TAG: i16 = 6;
/// The MSAEP tag of the phone's permission publication.
pub const PERMISSIONS_TAG: i16 = 3;

/// The trace-state names (`ypScenarioId`, `ypTriggerId`) of each request.
pub mod trace_name {
    pub const CAPABILITIES: (&str, &str) =
        ("CDEH.RemoteSnipping.RefreshDeviceCapabilities", "RemoteSnippingServiceStarted");
    pub const THUMBNAIL: (&str, &str) = ("CDEH.RemoteSnipping.GetThumbnailScenarioId", "NewPhotoEventReceived");
    pub const PHOTO: (&str, &str) = ("CDEH.RemoteSnipping.GetPhotoScenarioId", "NewPhotoToastClicked");
    pub const PERMISSION: (&str, &str) =
        ("CDEH.RemoteSnipping.RequestMediaPermissions", "SettingsItemHyperlinkClicked");
}

/// `MediaRequestMessage { request_type: CAPABILITIES }` (no `media_id`).
pub fn capabilities_request() -> Vec<u8> {
    MediaRequestMessage { request_type: MediaRequestType::Capabilities as i32, media_id: None }.encode_to_vec()
}

/// A fetch of one item's image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    Thumbnail,
    Photo,
}

impl Fetch {
    pub fn request_type(self) -> MediaRequestType {
        match self {
            Fetch::Thumbnail => MediaRequestType::Thumbnail,
            Fetch::Photo => MediaRequestType::Photo,
        }
    }

    pub fn trace_names(self) -> (&'static str, &'static str) {
        match self {
            Fetch::Thumbnail => trace_name::THUMBNAIL,
            Fetch::Photo => trace_name::PHOTO,
        }
    }
}

/// `MediaRequestMessage { request_type: THUMBNAIL | PHOTO, media_id }`.
pub fn fetch_request(fetch: Fetch, media_id: i64) -> Vec<u8> {
    MediaRequestMessage { request_type: fetch.request_type() as i32, media_id: Some(media_id) }.encode_to_vec()
}

/// `PermissionsRequestMessage { feature_permissions: [PHOTOS] }`.
pub fn photos_permission_request() -> Vec<u8> {
    PermissionsRequestMessage { feature_permissions: vec![FeaturePermissions::Photos as i32] }.encode_to_vec()
}

/// A `MediaResponseMessage` as the PC reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaReply {
    /// `capabilities` contains `NEW_MEDIA_NOTIFICATIONS`.
    pub new_media_notifications: bool,
    pub has_permissions: bool,
    /// Not acted on; logged.
    pub status: i32,
    pub thumbnail: Option<Vec<u8>>,
    pub photo: Option<Vec<u8>>,
}

/// Parses a `MEDIA` reply payload.
pub fn parse_media_reply(payload: &[u8]) -> Result<MediaReply, prost::DecodeError> {
    let m = MediaResponseMessage::decode(payload)?;
    let item = m.item.unwrap_or_default();
    Ok(MediaReply {
        new_media_notifications: m.capabilities.contains(&(MediaCapability::NewMediaNotifications as i32)),
        has_permissions: m.has_permissions,
        status: m.status,
        thumbnail: item.thumbnail,
        photo: item.photo,
    })
}

impl MediaReply {
    /// The bytes a fetch yields: the requested field when present (an empty one yields empty bytes).
    pub fn bytes(self, fetch: Fetch) -> Option<Vec<u8>> {
        match fetch {
            Fetch::Thumbnail => self.thumbnail,
            Fetch::Photo => self.photo,
        }
    }
}

/// What a tag-6 publication asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaChange {
    /// `NEW_PHOTO` with at least one item: the first one.
    NewPhoto(MediaMetadata),
    /// `NEW_PHOTO` without an item (malformed).
    NewPhotoWithoutItem,
    /// Any other `change_type` value.
    Other(i32),
}

pub fn parse_media_change(additional: &[u8]) -> Result<MediaChange, prost::DecodeError> {
    let m = MediaChangeMessage::decode(additional)?;
    if m.change_type != MediaChangeType::NewPhoto as i32 {
        return Ok(MediaChange::Other(m.change_type));
    }
    Ok(match m.changed_media.into_iter().next() {
        Some(first) => MediaChange::NewPhoto(first),
        None => MediaChange::NewPhotoWithoutItem,
    })
}

/// A tag-3 publication lists `PHOTOS` in `feature_permissions`.
pub fn publication_grants_photos(additional: &[u8]) -> Result<bool, prost::DecodeError> {
    let m = PermissionsResponseMessage::decode(additional)?;
    Ok(m.feature_permissions.contains(&(FeaturePermissions::Photos as i32)))
}

/// How the permission request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// `ACCEPTED` with `PHOTOS`: the phone allows photo access.
    Granted,
    /// `ACCEPTED` without `PHOTOS`: the phone shows its prompt.
    RequestSent,
    /// Anything else; the text says what.
    Failed(String),
}

/// Reads a permission reply payload.
pub fn permission_outcome(payload: &[u8]) -> PermissionOutcome {
    let m = match PermissionsResponseMessage::decode(payload) {
        Ok(m) => m,
        Err(e) => return PermissionOutcome::Failed(format!("the reply does not parse: {e}")),
    };

    if m.request_status != PermissionsRequestStatus::Accepted as i32 {
        return PermissionOutcome::Failed(format!("the phone answered request status {}", m.request_status));
    }
    if m.feature_permissions.contains(&(FeaturePermissions::Photos as i32)) {
        PermissionOutcome::Granted
    } else {
        PermissionOutcome::RequestSent
    }
}

/// The range of a valid `google.protobuf.Timestamp`: 0001-01-01T00:00:00Z ..= 9999-12-31T23:59:59Z.
const MIN_SECONDS: i64 = -62_135_596_800;
const MAX_SECONDS: i64 = 253_402_300_799;

/// The item's `date_modified` in nanoseconds since the Unix epoch, truncated to 100 ns; `None` when
/// absent or not a valid `Timestamp`.
pub fn date_modified_ns(item: &MediaMetadata) -> Option<i128> {
    let t = item.date_modified.as_ref()?;
    if !(MIN_SECONDS..=MAX_SECONDS).contains(&t.seconds) || !(0..=999_999_999).contains(&t.nanos) {
        return None;
    }
    Some(i128::from(t.seconds) * 1_000_000_000 + i128::from(t.nanos / 100 * 100))
}

/// `time` in nanoseconds since the Unix epoch (negative before it).
pub fn system_time_ns(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
        Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// `now − then < window`, signed: a `then` after `now` is within it.
pub fn within(now_ns: i128, then_ns: i128, window: Duration) -> bool {
    now_ns - then_ns < i128::try_from(window.as_nanos()).unwrap_or(i128::MAX)
}

#[cfg(test)]
mod tests;
