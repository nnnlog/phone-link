//! Decoded views of platform messages for the decode seam ([`phonelink_core::tap`], layer
//! [`TapLayer::Platform`]): route, ordered headers, and the body decoded as JSON where it is KVP or one
//! of the platform-route protobufs (`phonelink_proto::platform_sdk`).

use prost::Message;
use serde_json::{Value, json};

use super::kvp::Kvp;
use super::route;
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use phonelink_core::types::{DcgClient, MessageType, TransportMessage, header};
use phonelink_proto::platform_sdk::v1 as pb;

/// How to interpret a message body.
pub(crate) enum BodyContext<'a> {
    /// A request (or one-way message): decode by its own `_route`.
    Request,
    /// A response: decode by the route of the request it answers.
    ResponseTo(&'a str),
    /// A JSON body that is not KVP (device-proxy commands and replies).
    Json,
    /// Nothing known about the body.
    Opaque,
}

pub(crate) fn message_event(
    direction: Direction,
    partner: &DcgClient,
    message: &TransportMessage,
    body: BodyContext<'_>,
) -> TapEvent {
    let summary = json!({
        "message_type": match message.message_type {
            MessageType::App => "App",
            MessageType::Platform => "Platform",
        },
        "route": message.route(),
        "headers": message.headers.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "body": decode_body(message, &body),
    });

    TapEvent::new(TapLayer::Platform, direction, Some(partner.to_string()), summary, Some(&message.payload))
}

/// A tap event that is not a message (session transitions).
pub(crate) fn note_event(direction: Direction, partner: &DcgClient, summary: Value) -> TapEvent {
    TapEvent::new(TapLayer::Platform, direction, Some(partner.to_string()), summary, None)
}

fn decode_body(message: &TransportMessage, body: &BodyContext<'_>) -> Value {
    let payload = message.payload.as_slice();
    if let BodyContext::Json = body {
        return serde_json::from_slice::<Value>(payload)
            .unwrap_or_else(|e| json!({ "undecodable": e.to_string(), "len": payload.len() }));
    }

    let content_type = message.headers.get(header::MS_CONTENT_TYPE);
    match content_type {
        Some(header::CONTENT_TYPE_JSON) => match Kvp::read(content_type, payload) {
            Ok(kvp) => kvp_json(&kvp),
            Err(e) => json!({ "undecodable": e.to_string(), "len": payload.len() }),
        },
        Some(header::CONTENT_TYPE_BINARY) => {
            decode_protobuf(message, body).unwrap_or_else(|| json!({ "len": payload.len() }))
        }
        None => decode_protobuf(message, body)
            .or_else(|| Kvp::read(None, payload).ok().filter(|_| !payload.is_empty()).map(|k| kvp_json(&k)))
            .unwrap_or_else(|| json!({ "len": payload.len() })),
        Some(other) => json!({ "content_type": other, "len": payload.len() }),
    }
}

fn kvp_json(kvp: &Kvp) -> Value {
    Value::Object(kvp.iter().map(|(k, v)| (k.to_owned(), Value::String(v.to_owned()))).collect())
}

fn decode_protobuf(message: &TransportMessage, body: &BodyContext<'_>) -> Option<Value> {
    let bytes = message.payload.as_slice();
    match body {
        BodyContext::Request => match message.route()? {
            route::SESSION_VALIDATION => pb::ProtoPlatformSessionValidationRequest::decode(bytes)
                .ok()
                .map(|m| json!({ "platform_capabilities": capabilities(&m.platform_capabilities) })),
            route::CONTEXT_PUBLISH => pb::MsaepMessage::decode(bytes).ok().map(|m| msaep_json(&m)),
            route::DEVICE_RESOURCE_MANAGER => {
                pb::DeviceResourceManagerMessage::decode(bytes).ok().map(|m| drm_request_json(&m))
            }
            route::PLATFORM_DISCONNECT_REQUEST => pb::PlatformDisconnectRequest::decode(bytes).ok().map(|m| {
                json!({
                    "reason": enum_name(m.reason, |v| pb::PlatformDisconnectRequestReason::try_from(v).ok().map(|e| e.as_str_name())),
                    "trace_context": m.trace_context.map(|t| json!({
                        "trace_id": t.trace_id,
                        "parent_id": t.parent_id,
                        "correlation_id": t.correlation_id,
                        "trace_state": t.trace_state,
                    })),
                })
            }),
            _ => None,
        },
        BodyContext::ResponseTo(request_route) => match *request_route {
            route::SESSION_VALIDATION => pb::ProtoPlatformSessionValidationResponse::decode(bytes).ok().map(|m| {
                json!({
                    "platform_capabilities": capabilities(&m.platform_capabilities),
                    "persistent_messaging_channel_version": m.persistent_messaging_channel_version,
                    "nano_transport_preference_version": m.nano_transport_preference_version,
                })
            }),
            route::DEVICE_RESOURCE_MANAGER => {
                pb::DeviceResourceManagerResponseMessage::decode(bytes).ok().map(|m| {
                    json!({
                        "response_type": m.response_type.map(|v| enum_name(v, |v| {
                            pb::DeviceResourceManagerResponseMessageType::try_from(v).ok().map(|e| e.as_str_name())
                        })),
                        "payload_hex": hex::encode(&m.payload),
                    })
                })
            }
            _ => None,
        },
        BodyContext::Json | BodyContext::Opaque => None,
    }
}

fn enum_name(value: i32, name: impl Fn(i32) -> Option<&'static str>) -> Value {
    match name(value) {
        Some(n) => Value::String(n.to_owned()),
        None => json!(value),
    }
}

fn capabilities(values: &[i32]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|v| enum_name(*v, |v| pb::ProtoPlatformCapabilities::try_from(v).ok().map(|e| e.as_str_name())))
            .collect(),
    )
}

fn msaep_json(m: &pb::MsaepMessage) -> Value {
    let payload = match pb::PubSubPayload::decode(m.payload.as_slice()) {
        Ok(p) => json!({ "data_hex": hex::encode(&p.data), "additional_hex": hex::encode(&p.additional) }),
        Err(e) => json!({ "undecodable": e.to_string(), "hex": hex::encode(&m.payload) }),
    };

    json!({
        "message_tag": m.message_tag,
        "dcg_client_id": m.dcg_client_id,
        "message_id": m.message_id,
        "platform_major_version": m.platform_major_version,
        "platform_minor_version": m.platform_minor_version,
        "payload": payload,
    })
}

fn drm_request_json(m: &pb::DeviceResourceManagerMessage) -> Value {
    json!({
        "resource_type": enum_name(m.resource_type, |v| pb::ProtoDeviceResourceType::try_from(v).ok().map(|e| e.as_str_name())),
        "request_type": enum_name(m.request_type, |v| pb::ProtoDeviceResourceRequestType::try_from(v).ok().map(|e| e.as_str_name())),
        "resource_path": m.resource_path,
        "payload_hex": hex::encode(&m.payload),
    })
}
