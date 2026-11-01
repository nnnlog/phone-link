//! Decoding of one hub message: the `MessagePack` array of each message type.

use rmpv::Value;

use super::{
    CompletionResult, HubCodecError, HubMessage, from_bytes, malformed, read_headers, read_i64, read_opt_string,
    read_string, read_string_array,
};

/// Decodes one hub message payload (the bytes after the length prefix).
pub fn decode_message(payload: &[u8]) -> Result<HubMessage, HubCodecError> {
    let value = from_bytes(payload)?;
    let Value::Array(items) = value else {
        return Err(malformed("hub message is not an array"));
    };
    let Some(first) = items.first() else {
        return Err(malformed("empty hub message array"));
    };

    let message_type = read_i64(first, "message type")?;
    let need = |n: usize| need_elements(&items, message_type, n);

    let msg = match message_type {
        1 => decode_invocation(&items, message_type)?,
        2 => {
            need(4)?;
            HubMessage::StreamItem {
                headers: read_headers(&items[1])?,
                invocation_id: read_string(&items[2], "invocationId")?,
                item: items[3].clone(),
            }
        }
        3 => decode_completion(&items, message_type)?,
        4 => decode_stream_invocation(&items, message_type)?,
        5 => {
            need(3)?;
            HubMessage::CancelInvocation {
                headers: read_headers(&items[1])?,
                invocation_id: read_string(&items[2], "invocationId")?,
            }
        }
        6 => HubMessage::Ping,
        7 => decode_close(&items, message_type)?,
        8 => {
            need(2)?;
            HubMessage::Ack { sequence_id: read_i64(&items[1], "sequenceId")? }
        }
        9 => {
            need(2)?;
            HubMessage::Sequence { sequence_id: read_i64(&items[1], "sequenceId")? }
        }
        other => HubMessage::Unknown { message_type: other },
    };
    Ok(msg)
}

/// Refuses a message of `message_type` with fewer than `n` elements.
fn need_elements(items: &[Value], message_type: i64, n: usize) -> Result<(), HubCodecError> {
    if items.len() < n {
        Err(malformed(format!("message type {message_type} needs {n} elements, got {}", items.len())))
    } else {
        Ok(())
    }
}

/// An invocation's `arguments` array.
fn read_arguments(v: &Value) -> Result<Vec<Value>, HubCodecError> {
    match v {
        Value::Array(args) => Ok(args.clone()),
        _ => Err(malformed("arguments is not an array")),
    }
}

/// An invocation's optional sixth element, `streamIds` (absent = none).
fn read_stream_ids(items: &[Value]) -> Result<Vec<String>, HubCodecError> {
    match items.get(5) {
        Some(v) => read_string_array(v, "streamIds"),
        None => Ok(Vec::new()),
    }
}

/// Message type 1, `Invocation`.
fn decode_invocation(items: &[Value], message_type: i64) -> Result<HubMessage, HubCodecError> {
    need_elements(items, message_type, 5)?;
    Ok(HubMessage::Invocation {
        headers: read_headers(&items[1])?,
        invocation_id: read_opt_string(&items[2], "invocationId")?,
        target: read_string(&items[3], "target")?,
        arguments: read_arguments(&items[4])?,
        stream_ids: read_stream_ids(items)?,
    })
}

/// Message type 3, `Completion`: its result kind is read before the headers.
fn decode_completion(items: &[Value], message_type: i64) -> Result<HubMessage, HubCodecError> {
    need_elements(items, message_type, 4)?;
    let kind = read_i64(&items[3], "resultKind")?;
    let result = match kind {
        1 => {
            need_elements(items, message_type, 5)?;
            CompletionResult::Error(read_opt_string(&items[4], "error")?.unwrap_or_default())
        }
        2 => CompletionResult::Void,
        3 => {
            need_elements(items, message_type, 5)?;
            CompletionResult::Value(items[4].clone())
        }
        other => return Err(HubCodecError::UnknownCompletionKind(other)),
    };

    Ok(HubMessage::Completion {
        headers: read_headers(&items[1])?,
        invocation_id: read_string(&items[2], "invocationId")?,
        result,
    })
}

/// Message type 4, `StreamInvocation`.
fn decode_stream_invocation(items: &[Value], message_type: i64) -> Result<HubMessage, HubCodecError> {
    need_elements(items, message_type, 5)?;
    Ok(HubMessage::StreamInvocation {
        headers: read_headers(&items[1])?,
        invocation_id: read_string(&items[2], "invocationId")?,
        target: read_string(&items[3], "target")?,
        arguments: read_arguments(&items[4])?,
        stream_ids: read_stream_ids(items)?,
    })
}

/// Message type 7, `Close`.
fn decode_close(items: &[Value], message_type: i64) -> Result<HubMessage, HubCodecError> {
    need_elements(items, message_type, 2)?;
    Ok(HubMessage::Close {
        error: read_opt_string(&items[1], "error")?,
        allow_reconnect: match items.get(2) {
            Some(Value::Boolean(b)) => Some(*b),
            Some(Value::Nil) | None => None,
            Some(_) => return Err(malformed("allowReconnect is not a bool")),
        },
    })
}
