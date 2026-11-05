//! The cloud device-proxy receiver — the device-proxy variant of `/DeviceResourceManager`, which takes
//! its App messages before the rest of inbound App dispatch.
//!
//! An App message whose `_route` header is `/DeviceProxyClient/TransportMiddleware` carries a JSON
//! command
//! `{"name":…,"parameters":{"commandContent":"<json of {resourceType, requestType, payload(base64), resourcePath}>"}}`
//! with headers `RequestId`, `ChunkSize` (KiB) and `ContentType`. The command is dispatched to the
//! same receiver registry as `/DeviceResourceManager` and answered with an App message: headers
//! `RequestId` (echo), `ContentType: application/json`, `ChunkId: "0"`, `ChunkCount: "1"`,
//! `FileSize`; body
//! `{"type":"SendCommandResponse","value":"<json of {responseType, payload(base64)}>"}` of at most
//! `ChunkSize × 1024` bytes. The reply echoes the inbound `connectionSessionId`, as a session-based
//! reply. The receiver's follow-up runs once the reply's send has finished, as on
//! `/DeviceResourceManager`.
//!
//! The engine's own choices: the command's enum fields are accepted as numbers or as
//! (case-insensitive) member names; `responseType` is written as a number; a reply larger than
//! `ChunkSize × 1024`, a missing `RequestId` or an unusable `ChunkSize` is logged and **no** reply
//! is sent; the command `name` is logged, not checked.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use tracing::{debug, warn};

use super::drm::{AnswerDelivery, Drm, DrmAnswer, DrmChannel, DrmDispatch, DrmRequest, finish_answer};
use super::priority;
use super::router::{InboundMessage, PlatformRouteHandler, RESPONSE_OPTIONS, Router};
use super::tap_decode::{self, BodyContext};
use super::version::parse_text_int;
use phonelink_core::tap::Direction;
use phonelink_core::types::{DcgSendStatus, HeaderMap, MessageType, OutboundMessage, TransportMessage};
use phonelink_proto::platform_sdk::v1::{
    DeviceResourceManagerMessage, DeviceResourceManagerResponseMessageType, ProtoDeviceResourceRequestType,
    ProtoDeviceResourceType,
};

pub const REQUEST_ID_HEADER: &str = "RequestId";
pub const CHUNK_SIZE_HEADER: &str = "ChunkSize";
pub const CONTENT_TYPE_HEADER: &str = "ContentType";
pub const CHUNK_ID_HEADER: &str = "ChunkId";
pub const CHUNK_COUNT_HEADER: &str = "ChunkCount";
pub const FILE_SIZE_HEADER: &str = "FileSize";
pub const REPLY_CONTENT_TYPE: &str = "application/json";
pub const REPLY_TYPE: &str = "SendCommandResponse";

#[derive(Debug, Deserialize)]
struct Command {
    #[serde(default)]
    name: Option<String>,
    parameters: Parameters,
}

#[derive(Debug, Deserialize)]
struct Parameters {
    #[serde(rename = "commandContent")]
    command_content: String,
}

#[derive(Debug, Deserialize)]
struct CommandContent {
    #[serde(rename = "resourceType", default)]
    resource_type: Option<EnumValue>,
    #[serde(rename = "requestType", default)]
    request_type: Option<EnumValue>,
    #[serde(default)]
    payload: Option<String>,
    #[serde(rename = "resourcePath", default)]
    resource_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum EnumValue {
    Number(i64),
    Name(String),
}

/// Member names of the DRM enumerations, with their wire values.
const RESOURCE_TYPE_NAMES: [(&str, i32); 11] = [
    ("Unspecified", ProtoDeviceResourceType::Unspecified as i32),
    ("FilesMetadata", ProtoDeviceResourceType::FilesMetadata as i32),
    ("CameraMetadata", ProtoDeviceResourceType::CameraMetadata as i32),
    ("Permissions", ProtoDeviceResourceType::Permissions as i32),
    ("Unknown", ProtoDeviceResourceType::Unknown as i32),
    ("Wallpaper", ProtoDeviceResourceType::Wallpaper as i32),
    ("Media", ProtoDeviceResourceType::Media as i32),
    ("RingMyPhone", ProtoDeviceResourceType::RingMyPhone as i32),
    ("LockMyPc", ProtoDeviceResourceType::LockMyPc as i32),
    ("FileShare", ProtoDeviceResourceType::FileShare as i32),
    ("RemoteControl", ProtoDeviceResourceType::RemoteControl as i32),
];

const REQUEST_TYPE_NAMES: [(&str, i32); 5] = [
    ("Unspecified", ProtoDeviceResourceRequestType::Unspecified as i32),
    ("Get", ProtoDeviceResourceRequestType::Get as i32),
    ("Update", ProtoDeviceResourceRequestType::Update as i32),
    ("Delete", ProtoDeviceResourceRequestType::Delete as i32),
    ("Sync", ProtoDeviceResourceRequestType::Sync as i32),
];

/// The wire value of an enum field: a number as is (out of `i32` range ⇒ `None`), a name by
/// case-insensitive match; a missing field is `0` (`UNSPECIFIED`).
fn enum_value(value: &Option<EnumValue>, names: &[(&str, i32)]) -> Option<i32> {
    match value {
        None => Some(0),
        Some(EnumValue::Number(n)) => i32::try_from(*n).ok(),
        Some(EnumValue::Name(name)) => names.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v),
    }
}

/// The reply body for a receiver's answer bytes.
pub fn reply_body(payload: &[u8]) -> Vec<u8> {
    let encoded = base64::prelude::BASE64_STANDARD.encode(payload);
    let value = format!(
        "{{\"responseType\":{},\"payload\":{}}}",
        DeviceResourceManagerResponseMessageType::Success as i32,
        serde_json::to_string(&encoded).expect("a string serializes")
    );

    format!(
        "{{\"type\":{},\"value\":{}}}",
        serde_json::to_string(REPLY_TYPE).expect("a string serializes"),
        serde_json::to_string(&value).expect("a string serializes")
    )
    .into_bytes()
}

/// Receives `/DeviceProxyClient/TransportMiddleware` App messages.
pub struct DeviceProxyReceiver {
    router: Arc<Router>,
    drm: Arc<Drm>,
}

impl DeviceProxyReceiver {
    pub fn new(router: Arc<Router>, drm: Arc<Drm>) -> Self {
        Self { router, drm }
    }

    fn parse(&self, inbound: &InboundMessage) -> Result<(Option<String>, DeviceResourceManagerMessage), BadCommand> {
        let command: Command = serde_json::from_slice(&inbound.message.payload).map_err(BadCommand::Command)?;
        let content: CommandContent =
            serde_json::from_str(&command.parameters.command_content).map_err(BadCommand::Content)?;

        let resource_type = enum_value(&content.resource_type, &RESOURCE_TYPE_NAMES)
            .ok_or_else(|| BadCommand::ResourceType(content.resource_type.clone()))?;
        let request_type = enum_value(&content.request_type, &REQUEST_TYPE_NAMES)
            .ok_or_else(|| BadCommand::RequestType(content.request_type.clone()))?;
        let payload = match content.payload.as_deref() {
            None | Some("") => Vec::new(),
            Some(b64) => base64::prelude::BASE64_STANDARD.decode(b64).map_err(BadCommand::Payload)?,
        };

        let wire = DeviceResourceManagerMessage {
            resource_type,
            request_type,
            payload,
            resource_path: content.resource_path.unwrap_or_default(),
        };
        Ok((command.name, wire))
    }
}

/// Why a device-proxy command was not understood.
#[derive(Debug, thiserror::Error)]
enum BadCommand {
    #[error("not a DeviceProxyCommand: {0}")]
    Command(#[source] serde_json::Error),
    #[error("commandContent is not a DRM command: {0}")]
    Content(#[source] serde_json::Error),
    #[error("unknown resourceType {0:?}")]
    ResourceType(Option<EnumValue>),
    #[error("unknown requestType {0:?}")]
    RequestType(Option<EnumValue>),
    #[error("payload is not base64: {0}")]
    Payload(#[source] base64::DecodeError),
}

#[async_trait]
impl PlatformRouteHandler for DeviceProxyReceiver {
    async fn handle(&self, inbound: InboundMessage) {
        let tap = self.router.tap().clone();
        tap.emit(tap_decode::message_event(Direction::In, &inbound.source, &inbound.message, BodyContext::Json));

        let headers = &inbound.message.headers;
        let Some(request_id) = headers.get(REQUEST_ID_HEADER).map(str::to_owned) else {
            warn!(partner = %inbound.source, "device-proxy command without RequestId; no reply");
            return;
        };
        let chunk_limit = match headers.get(CHUNK_SIZE_HEADER).and_then(parse_text_int) {
            Some(kib) if kib >= 0 => usize::try_from(kib).unwrap_or(usize::MAX).saturating_mul(1024),
            other => {
                warn!(partner = %inbound.source, raw_chunk_size = ?headers.get(CHUNK_SIZE_HEADER), parsed_chunk_size = ?other, "device-proxy command without a usable ChunkSize; no reply");
                return;
            }
        };

        let (name, wire) = match self.parse(&inbound) {
            Ok(parsed) => parsed,
            Err(why) => {
                warn!(partner = %inbound.source, %why, "device-proxy command not understood; no reply");
                return;
            }
        };

        debug!(partner = %inbound.source, raw_command_name = ?name, raw_content_type = ?headers.get(CONTENT_TYPE_HEADER), "device-proxy command");
        let request = match DrmRequest::from_wire(
            inbound.source.clone(),
            Some(request_id.clone()),
            wire,
            DrmChannel::DeviceProxy,
        ) {
            Ok(request) => request,
            Err(fault) => {
                warn!(partner = %inbound.source, %fault, "device-proxy command faulted while parsing; no reply");
                return;
            }
        };
        let path = request.path.clone();
        let DrmDispatch::Answer(DrmAnswer { payload, follow_up }) = self.drm.dispatch(request).await else {
            return;
        };

        let body = reply_body(&payload);
        if body.len() > chunk_limit {
            warn!(partner = %inbound.source, reply_len = body.len(), limit_bytes = chunk_limit, "device-proxy reply exceeds ChunkSize × 1024; not sent");
            finish_answer(follow_up, AnswerDelivery::NotSent, &inbound.source, &path).await;
            return;
        }

        let reply_headers = HeaderMap::from_pairs([
            (REQUEST_ID_HEADER, request_id.clone()),
            (CONTENT_TYPE_HEADER, REPLY_CONTENT_TYPE.to_owned()),
            (CHUNK_ID_HEADER, "0".to_owned()),
            (CHUNK_COUNT_HEADER, "1".to_owned()),
            (FILE_SIZE_HEADER, body.len().to_string()),
        ])
        .expect("distinct header keys");
        let message = TransportMessage::new(MessageType::App, reply_headers, body);
        tap.emit(tap_decode::message_event(Direction::Out, &inbound.source, &message, BodyContext::Json));

        let status = self
            .router
            .send_raw(OutboundMessage {
                target: inbound.source.clone(),
                message,
                priority: priority::DEVICE_PROXY_REPLY,
                options: RESPONSE_OPTIONS,
                connection_session_id: inbound.connection_session_id.clone(),
                trace: inbound.trace.clone(),
            })
            .await;

        let delivery = if status == DcgSendStatus::Success {
            AnswerDelivery::Sent
        } else {
            warn!(partner = %inbound.source, request_id, ?status, error = status.cause(), "device-proxy reply not delivered");
            AnswerDelivery::NotSent
        };
        finish_answer(follow_up, delivery, &inbound.source, &path).await;
    }
}

#[cfg(test)]
mod tests;
