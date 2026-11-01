//! `clipboard.v1` as Cross Device writes it on the wire: the schema, the announcement message, the
//! content answer and correlation ids.
//!
//! The prost types generated from
//! `proto/CrossDevice.Clipboard.Protocol/clipboard/v1/clipboard.proto` already follow its write
//! rules: a proto3 scalar or string at its default is omitted, the two `optional` fields of
//! `ClipboardItem` are written whenever they are `Some` (also when empty), the `created_time`
//! message whenever it is `Some` (a zero `seconds` or `nanos` omitted inside it), and fields go out
//! in field-number order. The builders below decide which fields are set; the PC never sets
//! `error_type` or `error_detail`.

use std::time::{SystemTime, UNIX_EPOCH};

use prost::Message;
use prost_types::Timestamp;

use phonelink_proto::clipboard::v1::{
    ClipboardItem, ClipboardItemType, ClipboardRequestMessage, ClipboardRequestType, ClipboardResponseMessage,
    ClipboardResponseStatus,
};
use phonelink_proto::platform_sdk::v1::PubSubPayload;

/// MSAEP `message_tag` of clipboard announcements, in both directions.
pub const TAG: i16 = 9;

/// The `/DeviceResourceManager` resource path of every clipboard request, in both directions.
pub const RESOURCE_PATH: &str = "/clipboard";

/// A `ClipboardRequestMessage`; an empty `correlation_id` is omitted.
pub fn request(request_type: ClipboardRequestType, correlation_id: &str) -> Vec<u8> {
    ClipboardRequestMessage { request_type: request_type as i32, correlation_id: correlation_id.to_owned() }
        .encode_to_vec()
}

/// The content pull: `{ CONTENT, the announcement's correlation_id }`, the id omitted when the
/// announcement had none.
pub fn content_request(correlation_id: &str) -> Vec<u8> {
    request(ClipboardRequestType::Content, correlation_id)
}

/// A feature-state request: `{ FEATURE_ON | FEATURE_OFF | FEATURE_DISABLE }`, no correlation id.
pub fn feature_state_request(request_type: ClipboardRequestType) -> Vec<u8> {
    request(request_type, "")
}

/// The PC's announcement: `PubSubPayload { data = ClipboardResponseMessage { status = CLIPBOARD_CHANGE,
/// correlation_id } }`, `additional` empty and therefore omitted, no item.
pub fn announcement(correlation_id: &str) -> PubSubPayload {
    let message = ClipboardResponseMessage {
        status: ClipboardResponseStatus::ClipboardChange as i32,
        correlation_id: correlation_id.to_owned(),
        ..Default::default()
    };
    PubSubPayload { data: message.encode_to_vec(), additional: Vec::new() }
}

/// A status answer (receiver rows 1–9 and `INVALID_CONTENT`): no item, no error fields; the
/// correlation id is omitted when `None` or empty.
pub fn status_answer(status: ClipboardResponseStatus, correlation_id: Option<&str>) -> Vec<u8> {
    ClipboardResponseMessage {
        status: status as i32,
        correlation_id: correlation_id.unwrap_or_default().to_owned(),
        ..Default::default()
    }
    .encode_to_vec()
}

/// A content answer: `{ clipboard_item = [item], status = OK, correlation_id }`.
pub fn content_answer(item: ClipboardItem, correlation_id: &str) -> Vec<u8> {
    ClipboardResponseMessage {
        clipboard_item: vec![item],
        status: ClipboardResponseStatus::Ok as i32,
        correlation_id: correlation_id.to_owned(),
        ..Default::default()
    }
    .encode_to_vec()
}

/// A `TEXT_PLAIN` or `TEXT_HTML` item: `text` = the string **and** `image_bytes` set to empty
/// (`1a 00` on the wire).
pub fn text_item(item_type: ClipboardItemType, text: String, created_time: Timestamp) -> ClipboardItem {
    debug_assert!(matches!(item_type, ClipboardItemType::TextPlain | ClipboardItemType::TextHtml));
    ClipboardItem {
        item_type: item_type as i32,
        text: Some(text),
        image_bytes: Some(Vec::new()),
        created_time: Some(created_time),
    }
}

/// An `IMAGE` item: `text` set to empty (`12 00` on the wire) **and** `image_bytes` = the PNG (empty
/// for a zero-sized image).
pub fn image_item(png: Vec<u8>, created_time: Timestamp) -> ClipboardItem {
    ClipboardItem {
        item_type: ClipboardItemType::Image as i32,
        text: Some(String::new()),
        image_bytes: Some(png),
        created_time: Some(created_time),
    }
}

/// `created_time` from a wall-clock time, at 100 ns resolution (as Cross Device writes it).
pub fn timestamp(time: SystemTime) -> Timestamp {
    const TICK_NANOS: u32 = 100;
    match time.duration_since(UNIX_EPOCH) {
        Ok(since) => Timestamp {
            seconds: i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
            nanos: (since.subsec_nanos() / TICK_NANOS * TICK_NANOS) as i32,
        },
        Err(before) => {
            // Before 1970: a negative `seconds` with non-negative `nanos`.
            let before = before.duration();
            let nanos = before.subsec_nanos() / TICK_NANOS * TICK_NANOS;
            let seconds = -i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            if nanos == 0 {
                Timestamp { seconds, nanos: 0 }
            } else {
                Timestamp { seconds: seconds - 1, nanos: (1_000_000_000 - nanos) as i32 }
            }
        }
    }
}

#[cfg(test)]
mod tests;
