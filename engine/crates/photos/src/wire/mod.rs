//! The photos wire vocabulary.
//!
//! It covers keys, content types, flags and trace-state names; the readers of the inbound
//! `mediaInfo` messages (metadata and items, phone → PC) and of the `/media/v1` replies; the
//! builders of the `/media/v1` requests (thumbnail fetch, full-resolution fetch, media sync,
//! delete; PC → phone).
//!
//! Every reader validates the whole message before anything is applied and reports a defect as a
//! fixed English phrase naming the rule the message broke (never content from the message), which
//! becomes the reason of a rejection (the app's rule for rejecting malformed content).

use std::collections::HashSet;
use std::time::Duration;

use phonelink_app::Malformed;
use phonelink_app::consts::key as app_key;
use phonelink_app::{AppRequest, RequestOptions};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::TraceContext;

/// The route of every photos request.
pub const ROUTE_MEDIA: &str = "/media/v1";

/// `contentType` strings photos uses.
pub mod content_type {
    /// Inbound media-list sync; outbound media sync request.
    pub const MEDIA_INFO: &str = "mediaInfo";
    /// Outbound thumbnail request.
    pub const THUMBNAILS: &str = "thumbnails";
    /// Outbound full-resolution request.
    pub const PHOTOS: &str = "photos";
}

/// `ValueSet` keys of the photos messages.
pub mod key {
    // mediaInfo metadata message and its reply.
    pub const MEDIA_IDS: &str = "mediaIds";
    pub const MEDIA_TIMES: &str = "mediaTimes";
    pub const CHECKSUMS: &str = "checksums";
    pub const MEDIA_SETTINGS: &str = "mediaSettings";
    pub const MEDIA_CAPABILITIES: &str = "mediaCapabilities";
    pub const MEDIA_HAS_DELETE_PERMISSION: &str = "mediaHasDeletePermission";
    pub const SEQUENCE_NUMBER: &str = "sequenceNumber";
    pub const ITEM_COUNT: &str = "itemCount";
    pub const MEDIA_REQUESTED_IDS: &str = "mediaRequestedIds";
    pub const MESSAGE: &str = "message";
    // Item arrays (item messages and the thumbnail reply).
    pub const MEDIA_NAMES: &str = "mediaNames";
    pub const MEDIA_TAKEN_TIMES: &str = "mediaTakenTimes";
    pub const MEDIA_ORIENTATIONS: &str = "mediaOrientations";
    pub const MEDIA_MIME_TYPES: &str = "mediaMimeTypes";
    pub const MEDIA_HEIGHTS: &str = "mediaHeights";
    pub const MEDIA_WIDTHS: &str = "mediaWidths";
    pub const MEDIA_SIZES: &str = "mediaSizes";
    pub const MEDIA_ACTIONS: &str = "mediaActions";
    pub const MEDIA_URIS: &str = "mediaURIs";
    pub const MEDIA_THUMBNAILS: &str = "mediaThumbnails";
    pub const MEDIA_THUMBNAIL_SIZES: &str = "mediaThumbnailSizes";
    // Full-resolution reply.
    pub const CONTENT: &str = "content";
    // Media sync request.
    pub const RELATED_CORRELATION_IDS: &str = "relatedCorrelationIds";
    pub const SUBCONTENT_TYPES: &str = "subcontentTypes";
    // Delete request and reply.
    pub const REQUEST_TYPE: &str = "requestType";
    pub const MEDIA_SUCCESSFULLY_DELETED_IDS: &str = "mediaSuccessfullyDeletedIds";
    pub const MEDIA_UNSUCCESSFULLY_DELETED_IDS: &str = "mediaUnsuccessfullyDeletedIds";
    // Connect contribution.
    pub const MEDIA_INFO_SYNC: &str = "MediaInfoSync";
    pub const PHOTOS_LEGACY_SYNC: &str = "PhotosLegacySync";
    pub const MEDIA_INFO_SNAPSHOT_SIZE: &str = "MediaInfoSnapshotSize";
    pub const AUTO_HYDRATED_THUMBNAILS_COUNT: &str = "AutoHydratedThumbnailsCount";
    /// The sequence key (request `sequenceNumbers.media_info`, response top-level `media_info`).
    pub const MEDIA_INFO: &str = "media_info";
    pub const PERMISSIONS: &str = "permissions";
    pub const PERMISSION_PHOTOS: &str = "photos";
    pub const PERMISSION_PHOTOS_FULL: &str = "photos_full";
    pub const PERMISSION_PHOTOS_DELETE: &str = "photos_delete";
    pub const CAPABILITIES: &str = "capabilities";
    /// `capabilities.photos`.
    pub const CAPABILITY_PHOTOS: &str = "photos";
    pub const DCIM_DIRECTORY: &str = "dcim_directory";
}

/// `PhotosCapabilities` flags (the feature gates).
pub mod capability {
    pub const NONE: u32 = 0;
    pub const MEDIA_DELETE: u32 = 1;
    pub const PERMISSION_REQUIRED_MEDIA_DELETE: u32 = 2;
    /// **iOS-only**.
    pub const ICLOUD_PHOTOS: u32 = 4;
}

/// The hub trace-state names of the photos requests, Phone Link's own: the `ypTriggerId` of each entry
/// point and the `ypScenarioId` of each request.
pub mod trace_name {
    /// A gallery item asks for its thumbnail (Phone Link's spelling).
    pub const THUMBNAIL_REQUESTED: &str = "ThumnailRequested";
    /// A grid layout pass with optimized thumbnails.
    pub const MEDIA_VIEWED_IN_NODE: &str = "MediaViewedInNode";
    /// Scenario of every thumbnail request.
    pub const HYDRATE_THUMBNAILS: &str = "HydrateThumbnails";
    /// A grid click or an item's context menu (on Windows also the deep link, which the app has no
    /// entry point for).
    pub const IMAGE_REQUESTED: &str = "ImageRequested";
    /// The full-screen viewer's selection.
    pub const MEDIA_VIEWED_IN_GALLERY: &str = "MediaViewedInGallery";
    pub const ON_COPY: &str = "OnCopy";
    pub const ON_SAVE_AS: &str = "OnSaveAs";
    pub const ON_SHARE: &str = "OnShare";
    /// Open and open-with.
    pub const ON_OPEN: &str = "OnOpen";
    pub const ON_DRAG: &str = "OnDrag";
    /// Activating the new-photo notification.
    pub const TOAST_CLICK: &str = "OnRemoteSnippingToastClick";
    /// Scenario of every full-resolution request (all go through the dispatcher: the messaging
    /// picker's direct fetch is not built).
    pub const HYDRATE_PHOTO: &str = "HydratePhoto";
    /// "Allow photo access on the phone", and the trigger of its media sync.
    pub const ERROR_ACTION_INVOKED: &str = "ErrorActionInvoked";
    pub const PHOTOS_DEVICE_PERMISSION: &str = "PhotosDevicePermission";
    /// The full-access reminder's action, and the trigger of its media sync.
    pub const FULL_PERMISSION_REQUEST_INVOKED: &str = "PhotosFullPermissionRequestInvoked";
    pub const PHOTOS_DEVICE_FULL_PERMISSION: &str = "PhotosDeviceFullPermission";
    /// Scenario of the media sync request.
    pub const SYNC_ALL_MEDIA: &str = "SyncAllMedia";
    /// The delete pre-check and the "missing permission" prompt.
    pub const PERMISSION_REQUIRED: &str = "PermissionRequired";
    pub const PHOTOS_DELETE_PERMISSION: &str = "PhotosDeletePermission";
    pub const DELETE_INVOKED: &str = "DeleteInvoked";
    pub const DELETE_MEDIA: &str = "DeleteMedia";
}

/// `mediaActions` value of an item removed on the phone; any other value adds or updates.
pub const ACTION_REMOVED: i32 = 2;
/// The delete request's `requestType`.
pub const DELETE_REQUEST_TYPE: i32 = 0;

/// The `mediaSettings` of a metadata message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaSettings {
    /// `mediaCapabilities`; absent → `None` (0).
    pub capabilities: u32,
    /// `mediaHasDeletePermission`, when present.
    pub has_delete_permission: Option<bool>,
}

/// A valid metadata message, its id list cut to the first occurrence of every id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    pub correlation_vector: String,
    /// The phone's complete current id list in its order, each id once.
    pub ids: Vec<i64>,
    /// `mediaTimes`, aligned with `ids`.
    pub times: Vec<i64>,
    /// `checksums`, aligned with `ids`, when the message has them.
    pub checksums: Option<Vec<i64>>,
    /// `mediaSettings` when it is an `Object` (another type is ignored).
    pub settings: Option<MediaSettings>,
    /// `sequenceNumber` (`Int64`; `-1` counts as none).
    pub sequence_number: Option<i64>,
    /// How many repeated ids were ignored.
    pub repeated: usize,
}

/// Reads a metadata message. A missing required key, an array of another type, an array shorter
/// than `mediaIds` and a `mediaSettings` member of another type make it malformed.
pub fn parse_metadata(payload: &ValueSet) -> Result<Metadata, Malformed> {
    let correlation_vector = payload
        .str(app_key::CORRELATION_VECTOR)
        .ok_or(Malformed::reason("metadata message without a correlationVector"))?;
    let ids =
        payload.i64_array(key::MEDIA_IDS).ok_or(Malformed::reason("metadata mediaIds missing or not an Int64Array"))?;
    let times = payload
        .i64_array(key::MEDIA_TIMES)
        .ok_or(Malformed::reason("metadata mediaTimes missing or not an Int64Array"))?;

    if times.len() < ids.len() {
        return Err(Malformed::reason("metadata mediaTimes shorter than mediaIds"));
    }

    let checksums = match payload.get(key::CHECKSUMS) {
        None => None,
        Some(Value::Int64Array(c)) if c.len() < ids.len() => {
            return Err(Malformed::reason("metadata checksums shorter than mediaIds"));
        }
        Some(Value::Int64Array(c)) => Some(c.as_slice()),
        Some(_) => return Err(Malformed::reason("metadata checksums not an Int64Array")),
    };
    let settings = match payload.get(key::MEDIA_SETTINGS) {
        Some(Value::Object(s)) => Some(parse_settings(s)?),
        // Absent, or a value of another type: ignored.
        _ => None,
    };
    let sequence_number = payload.i64(key::SEQUENCE_NUMBER).filter(|s| *s != -1);

    let mut seen = HashSet::with_capacity(ids.len());
    let mut out = Metadata {
        correlation_vector: correlation_vector.to_string(),
        ids: Vec::with_capacity(ids.len()),
        times: Vec::with_capacity(ids.len()),
        checksums: checksums.map(|_| Vec::with_capacity(ids.len())),
        settings,
        sequence_number,
        repeated: 0,
    };

    for (i, id) in ids.iter().enumerate() {
        if !seen.insert(*id) {
            out.repeated += 1;
            continue;
        }
        out.ids.push(*id);
        out.times.push(times[i]);
        if let (Some(dst), Some(src)) = (out.checksums.as_mut(), checksums) {
            dst.push(src[i]);
        }
    }
    Ok(out)
}

fn parse_settings(s: &ValueSet) -> Result<MediaSettings, Malformed> {
    let capabilities = match s.get(key::MEDIA_CAPABILITIES) {
        None => capability::NONE,
        Some(Value::Int32(v)) => *v as u32,
        Some(_) => return Err(Malformed::reason("mediaSettings mediaCapabilities not an Int32")),
    };
    let has_delete_permission = match s.get(key::MEDIA_HAS_DELETE_PERMISSION) {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(Malformed::reason("mediaSettings mediaHasDeletePermission not a Bool")),
    };
    Ok(MediaSettings { capabilities, has_delete_permission })
}

/// One added or updated item of an item message or a thumbnail reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaItem {
    pub id: i64,
    pub name: String,
    /// Last-modified time, Unix ms.
    pub modified_ms: i64,
    /// Date taken, Unix ms.
    pub taken_ms: i64,
    pub orientation: String,
    pub mime_type: String,
    /// Pixels, stored as 32-bit (the `Int64` truncated).
    pub height: i32,
    pub width: i32,
    /// Full file size, bytes.
    pub size: i64,
    /// `""` when the message has no `mediaURIs`.
    pub uri: String,
    /// `0` when the message has no `checksums`.
    pub checksum: i64,
    /// The item's inline thumbnail, when its slice is not empty.
    pub thumbnail: Option<Vec<u8>>,
}

/// The items of one item message or thumbnail reply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemBatch {
    /// Added or updated items, in message order.
    pub items: Vec<MediaItem>,
    /// Ids reported removed on the phone (`mediaActions` `2`).
    pub removed: Vec<i64>,
}

fn required<'a, T>(
    v: Option<&'a [T]>,
    n: usize,
    missing: &'static str,
    short: &'static str,
) -> Result<&'a [T], Malformed> {
    let v = v.ok_or(Malformed::reason(missing))?;
    if v.len() < n { Err(Malformed::reason(short)) } else { Ok(v) }
}

/// An optional array: absent or of another type counts as absent; present but shorter than
/// `mediaIds` is malformed.
fn optional<'a, T>(v: Option<&'a [T]>, n: usize, short: &'static str) -> Result<Option<&'a [T]>, Malformed> {
    match v {
        Some(v) if v.len() < n => Err(Malformed::reason(short)),
        other => Ok(other),
    }
}

/// Reads the parallel item arrays.
///
/// The required arrays must be present with their type and every used array at least as long as
/// `mediaIds`; an optional array of another type counts as absent; `mediaThumbnails` and
/// `mediaThumbnailSizes` are used only together, the slices of the non-removed items with a size
/// `> 0` taken in index order, and a slice beyond `mediaThumbnails` is malformed.
///
/// The entries at a removed item's index are not read.
pub fn parse_items(payload: &ValueSet) -> Result<ItemBatch, Malformed> {
    let ItemArrays {
        ids,
        names,
        times,
        taken,
        orientations,
        mime_types,
        heights,
        widths,
        sizes,
        actions,
        uris,
        checksums,
        thumbnails,
    } = item_arrays(payload)?;

    let n = ids.len();
    let mut batch = ItemBatch::default();
    let mut offset = 0usize;

    for i in 0..n {
        if actions[i] == ACTION_REMOVED {
            batch.removed.push(ids[i]);
            continue;
        }

        let thumbnail = match thumbnails {
            Some((blob, sizes)) if sizes[i] > 0 => {
                let end = usize::try_from(sizes[i])
                    .ok()
                    .and_then(|len| offset.checked_add(len))
                    .filter(|end| *end <= blob.len())
                    .ok_or(Malformed::reason("thumbnail slice beyond mediaThumbnails"))?;
                let slice = blob[offset..end].to_vec();
                offset = end;
                Some(slice)
            }
            _ => None,
        };

        batch.items.push(MediaItem {
            id: ids[i],
            name: names[i].clone(),
            modified_ms: times[i],
            taken_ms: taken[i],
            orientation: orientations[i].clone(),
            mime_type: mime_types[i].clone(),
            height: heights[i] as i32,
            width: widths[i] as i32,
            size: sizes[i],
            uri: uris.map(|u| u[i].clone()).unwrap_or_default(),
            checksum: checksums.map_or(0, |c| c[i]),
            thumbnail,
        });
    }
    Ok(batch)
}

/// The parallel item arrays of an item batch, checked against `mediaIds`.
struct ItemArrays<'a> {
    ids: &'a [i64],
    names: &'a [String],
    times: &'a [i64],
    taken: &'a [i64],
    orientations: &'a [String],
    mime_types: &'a [String],
    heights: &'a [i64],
    widths: &'a [i64],
    sizes: &'a [i64],
    actions: &'a [i32],
    uris: Option<&'a [String]>,
    checksums: Option<&'a [i64]>,
    /// `mediaThumbnails` with `mediaThumbnailSizes`, when both are present.
    thumbnails: Option<(&'a [u8], &'a [i64])>,
}

/// Reads and checks the parallel item arrays, in the order [`parse_items`] documents.
fn item_arrays(payload: &ValueSet) -> Result<ItemArrays<'_>, Malformed> {
    let ids = payload.i64_array(key::MEDIA_IDS).ok_or(Malformed::reason("mediaIds missing or not an Int64Array"))?;
    let n = ids.len();

    let names = required(
        payload.string_array(key::MEDIA_NAMES),
        n,
        "mediaNames missing or not a StringArray",
        "mediaNames shorter than mediaIds",
    )?;
    let times = required(
        payload.i64_array(key::MEDIA_TIMES),
        n,
        "mediaTimes missing or not an Int64Array",
        "mediaTimes shorter than mediaIds",
    )?;
    let taken = required(
        payload.i64_array(key::MEDIA_TAKEN_TIMES),
        n,
        "mediaTakenTimes missing or not an Int64Array",
        "mediaTakenTimes shorter than mediaIds",
    )?;
    let orientations = required(
        payload.string_array(key::MEDIA_ORIENTATIONS),
        n,
        "mediaOrientations missing or not a StringArray",
        "mediaOrientations shorter than mediaIds",
    )?;
    let mime_types = required(
        payload.string_array(key::MEDIA_MIME_TYPES),
        n,
        "mediaMimeTypes missing or not a StringArray",
        "mediaMimeTypes shorter than mediaIds",
    )?;
    let heights = required(
        payload.i64_array(key::MEDIA_HEIGHTS),
        n,
        "mediaHeights missing or not an Int64Array",
        "mediaHeights shorter than mediaIds",
    )?;
    let widths = required(
        payload.i64_array(key::MEDIA_WIDTHS),
        n,
        "mediaWidths missing or not an Int64Array",
        "mediaWidths shorter than mediaIds",
    )?;
    let sizes = required(
        payload.i64_array(key::MEDIA_SIZES),
        n,
        "mediaSizes missing or not an Int64Array",
        "mediaSizes shorter than mediaIds",
    )?;
    let actions = required(
        payload.i32_array(key::MEDIA_ACTIONS),
        n,
        "mediaActions missing or not an Int32Array",
        "mediaActions shorter than mediaIds",
    )?;

    let uris = optional(payload.string_array(key::MEDIA_URIS), n, "mediaURIs shorter than mediaIds")?;
    let checksums = optional(payload.i64_array(key::CHECKSUMS), n, "checksums shorter than mediaIds")?;
    let thumbnails = match (payload.bytes(key::MEDIA_THUMBNAILS), payload.i64_array(key::MEDIA_THUMBNAIL_SIZES)) {
        (Some(_), Some(sizes)) if sizes.len() < n => {
            return Err(Malformed::reason("mediaThumbnailSizes shorter than mediaIds"));
        }
        (Some(blob), Some(sizes)) => Some((blob, sizes)),
        _ => None,
    };

    Ok(ItemArrays {
        ids,
        names,
        times,
        taken,
        orientations,
        mime_types,
        heights,
        widths,
        sizes,
        actions,
        uris,
        checksums,
        thumbnails,
    })
}

/// One id's slice of a full-resolution reply (possibly empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullImage {
    pub id: i64,
    pub bytes: Vec<u8>,
}

/// Reads a full-resolution reply: `mediaIds`, `mediaSizes` (at least as long) and `content` are
/// required; each size must be `≥ 0` and the slices, taken in index order, must lie within
/// `content`.
pub fn parse_full_reply(payload: &ValueSet) -> Result<Vec<FullImage>, Malformed> {
    let ids = payload.i64_array(key::MEDIA_IDS).ok_or(Malformed::reason("mediaIds missing or not an Int64Array"))?;
    let sizes = required(
        payload.i64_array(key::MEDIA_SIZES),
        ids.len(),
        "mediaSizes missing or not an Int64Array",
        "mediaSizes shorter than mediaIds",
    )?;
    let content = payload.bytes(key::CONTENT).ok_or(Malformed::reason("content missing or not a UInt8Array"))?;

    let mut offset = 0usize;
    let mut out = Vec::with_capacity(ids.len());
    for (i, id) in ids.iter().enumerate() {
        let end = usize::try_from(sizes[i])
            .ok()
            .and_then(|len| offset.checked_add(len))
            .filter(|end| *end <= content.len())
            .ok_or(Malformed::reason("full-resolution slice negative or beyond content"))?;
        out.push(FullImage { id: *id, bytes: content[offset..end].to_vec() });
        offset = end;
    }
    Ok(out)
}

/// The two id arrays of a delete reply, when both are `Int64Array`s.
pub fn parse_delete_reply(payload: &ValueSet) -> Option<(Vec<i64>, Vec<i64>)> {
    let ok = payload.i64_array(key::MEDIA_SUCCESSFULLY_DELETED_IDS)?;
    let failed = payload.i64_array(key::MEDIA_UNSUCCESSFULLY_DELETED_IDS)?;
    Some((ok.to_vec(), failed.to_vec()))
}

/// The answer to a metadata message.
pub fn metadata_reply(
    correlation_vector: &str,
    contract_version: f64,
    outgoing: &[i64],
    requested: &[i64],
) -> ValueSet {
    ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, correlation_vector)
        .with(app_key::CONTENT_TYPE, content_type::MEDIA_INFO)
        .with(app_key::CONTRACT_VERSION, contract_version)
        .with(key::ITEM_COUNT, outgoing.len() as i32)
        .with(key::MEDIA_IDS, outgoing.to_vec())
        .with(key::MEDIA_REQUESTED_IDS, requested.to_vec())
        .with(app_key::RESULT, 0i32)
}

/// The answer to a rejected item message: `{ result: Int32 1, message: String }`.
pub fn item_rejection_answer(message: String) -> ValueSet {
    ValueSet::new().with(app_key::RESULT, 1i32).with(key::MESSAGE, message)
}

/// The feature's own keys of a fetch: `correlationVector` (the trace's correlation id),
/// `contentType`, `isuserinitiated` (written over by the legacy path with the option, `false`) and
/// `mediaIds`.
fn media_request(content_type: &str, ids: &[i64], trace: TraceContext, trigger: &str, scenario: &str) -> AppRequest {
    let payload = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with(app_key::CONTENT_TYPE, content_type)
        .with(app_key::IS_USER_INITIATED, false)
        .with(key::MEDIA_IDS, ids.to_vec());

    AppRequest::legacy(ROUTE_MEDIA, payload)
        .with_options(RequestOptions::default().named(trigger, scenario))
        .with_trace(trace)
}

/// A thumbnail request for 1–25 ids, default options, named after the trigger that started the
/// thumbnail worker.
pub fn thumbnails_request(ids: &[i64], trace: TraceContext, trigger: &str) -> AppRequest {
    media_request(content_type::THUMBNAILS, ids, trace, trigger, trace_name::HYDRATE_THUMBNAILS)
}

/// A full-resolution request for one id, default options; its retries reuse `trace` and therefore
/// the `correlationVector`. It is named after the trigger that queued the id.
pub fn photos_request(id: i64, trace: TraceContext, trigger: &str) -> AppRequest {
    media_request(content_type::PHOTOS, &[id], trace, trigger, trace_name::HYDRATE_PHOTO)
}

/// The media sync request, default options; it keeps the trigger of the permission request it
/// follows.
pub fn sync_request(trace: TraceContext, trigger: &str) -> AppRequest {
    let cv = trace.correlation_id.clone();
    let payload = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, cv.clone())
        .with(app_key::CONTENT_TYPE, content_type::MEDIA_INFO)
        .with(key::RELATED_CORRELATION_IDS, vec![cv])
        .with(key::SUBCONTENT_TYPES, vec![content_type::MEDIA_INFO]);

    AppRequest::legacy(ROUTE_MEDIA, payload)
        .with_options(RequestOptions::default().named(trigger, trace_name::SYNC_ALL_MEDIA))
        .with_trace(trace)
}

/// The delete request: no `contentType`; default options with the delete timeout.
pub fn delete_request(ids: &[i64], checksums: &[i64], trace: TraceContext, timeout: Duration) -> AppRequest {
    let payload = ValueSet::new()
        .with(app_key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with(key::REQUEST_TYPE, DELETE_REQUEST_TYPE)
        .with(key::MEDIA_IDS, ids.to_vec())
        .with(key::CHECKSUMS, checksums.to_vec());

    AppRequest::legacy(ROUTE_MEDIA, payload)
        .with_options(
            RequestOptions::default().with_timeout(timeout).named(trace_name::DELETE_INVOKED, trace_name::DELETE_MEDIA),
        )
        .with_trace(trace)
}

#[cfg(test)]
mod tests;
