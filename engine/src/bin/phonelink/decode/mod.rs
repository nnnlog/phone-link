//! `phonelink decode` — offline decoding of captured bytes at each seam, printed as JSON. Used to
//! read raw tap dumps; it never touches the network.

use anyhow_like::{Result, bail};
use serde_json::{Value as Json, json};

use phonelink_engine::codec::header;
use phonelink_engine::codec::{dcg_packet, envelope, hub, valueset};

/// Minimal error plumbing so the CLI has no extra dependency.
pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, String>;
    macro_rules! bail {
        ($($t:tt)*) => { return Err(format!($($t)*)) };
    }
    pub(crate) use bail;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Layer {
    /// One or more VarInt-prefixed `SignalR` hub frames (a WebSocket message after the handshake).
    Frames,
    /// One hub message payload (a `MessagePack` array, no length prefix).
    Hub,
    /// One multiplex packet (a `MessagePack` map).
    Packet,
    /// One binary inner envelope (a reassembled message).
    Envelope,
    /// One `pbvalueset` `ValueSet`.
    Valueset,
}

/// `@path` reads a binary file; anything else is hex (whitespace ignored), or base64 with `base64`.
pub fn read_input(input: &str, base64: bool) -> Result<Vec<u8>> {
    if let Some(path) = input.strip_prefix('@') {
        return std::fs::read(path).map_err(|e| format!("{path}: {e}"));
    }

    let compact: String = input.split_whitespace().collect();
    if base64 {
        use base64::Engine as _;
        base64::prelude::BASE64_STANDARD.decode(compact).map_err(|e| e.to_string())
    } else {
        hex::decode(compact).map_err(|e| e.to_string())
    }
}

pub fn decode(layer: Layer, bytes: &[u8]) -> Result<Json> {
    match layer {
        Layer::Frames => {
            let mut buf = bytes.to_vec();
            let frames = hub::drain_frames(&mut buf).map_err(|e| e.to_string())?;
            let mut out: Vec<Json> = frames.iter().map(|f| hub_message(f)).collect();
            if !buf.is_empty() {
                out.push(json!({ "incomplete_tail_hex": hex::encode(&buf) }));
            }
            Ok(Json::Array(out))
        }
        Layer::Hub => Ok(hub_message(bytes)),
        Layer::Packet => {
            let v = hub::from_bytes(bytes).map_err(|e| e.to_string())?;
            let p = hub::MultiplexPacket::from_value(&v).map_err(|e| e.to_string())?;
            Ok(packet(&p))
        }
        Layer::Envelope => envelope_json(bytes),
        Layer::Valueset => valueset::decode(bytes).map(|vs| vs.to_json()).map_err(|e| e.to_string()),
    }
}

fn hub_message(payload: &[u8]) -> Json {
    match hub::decode_message(payload) {
        Err(e) => json!({ "error": e.to_string(), "hex": hex::encode(payload) }),
        Ok(hub::HubMessage::Invocation { invocation_id, target, arguments, .. }) => {
            let mut obj = json!({
                "type": "Invocation",
                "invocationId": invocation_id,
                "target": target,
                "arguments": arguments.iter().map(hub::value_to_json).collect::<Vec<_>>(),
            });

            // Decode the multiplex packet of the packet-carrying methods.
            let packet_arg = match target.as_str() {
                hub::method::ON_RECEIVE_MESSAGE | hub::method::ON_RECEIVE_SESSION_BASED_MESSAGE => arguments.get(2),
                hub::method::SEND_MESSAGE | hub::method::SEND_SESSION_BASED_MESSAGE => arguments.get(2),
                _ => None,
            };
            if let Some(v) = packet_arg {
                obj["packet"] = match hub::MultiplexPacket::from_value(v) {
                    Ok(p) => packet(&p),
                    Err(e) => json!({ "error": e.to_string() }),
                };
            }

            obj
        }
        Ok(other) => json!({ "type": format!("{other:?}") }),
    }
}

fn packet(p: &hub::MultiplexPacket) -> Json {
    let properties: serde_json::Map<String, Json> =
        p.properties.iter().map(|(k, v)| (k.clone(), hub::value_to_json(v))).collect();
    let mut obj =
        json!({ "Type": p.handler_type, "Properties": properties, "RawLength": p.raw.as_ref().map(Vec::len) });

    match dcg_packet::parse_packet(p) {
        Ok(dcg_packet::ParsedPacket::Fragment(f)) => {
            obj["kind"] = json!("fragment");
            if f.fragment_count == 1 && f.fragment_id == 1 {
                obj["envelope"] = envelope_json(&f.raw).unwrap_or_else(|e| json!({ "error": e }));
            } else {
                obj["raw_hex"] = json!(hex::encode(&f.raw));
            }
        }
        Ok(dcg_packet::ParsedPacket::Ack(a)) => {
            obj["kind"] = json!("ack");
            obj["outcome"] = json!(format!("{:?}", a.outcome()));
        }
        Ok(dcg_packet::ParsedPacket::Presence(t)) => obj["kind"] = json!(format!("presence({t})")),
        Err(e) => obj["dropped"] = json!(e.to_string()),
    }
    obj
}

fn envelope_json(bytes: &[u8]) -> Result<Json> {
    let env = envelope::decode(bytes).map_err(|e| e.to_string())?;
    let headers: serde_json::Map<String, Json> = env.headers.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();

    let content_type = env.headers.get(header::MS_CONTENT_TYPE);
    let body = match content_type {
        // Platform messages carry ms-content-type; App messages never do.
        Some(header::CONTENT_TYPE_JSON) => match serde_json::from_slice::<Json>(&env.payload) {
            Ok(j) => json!({ "json": j }),
            Err(_) => json!({ "hex": hex::encode(&env.payload) }),
        },
        Some(_) => json!({ "hex": hex::encode(&env.payload) }),
        None => {
            let payload =
                if env.headers.get(header::IS_GZIPPED_COMPRESSED).is_some_and(|v| v.eq_ignore_ascii_case("true")) {
                    match valueset::gunzip(&env.payload) {
                        Ok(p) => p,
                        Err(e) => bail!("gzip: {e}"),
                    }
                } else {
                    env.payload.clone()
                };

            match valueset::decode(&payload) {
                Ok(vs) => json!({ "valueset": vs.to_json() }),
                Err(e) => json!({ "valueset_error": e.to_string(), "hex": hex::encode(&payload) }),
            }
        }
    };

    Ok(json!({ "headers": headers, "payloadLength": env.payload.len(), "payload": body }))
}

#[cfg(test)]
mod tests;
