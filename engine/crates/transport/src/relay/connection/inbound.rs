//! Inbound frames: decoding and dispatching them.

use super::{HubConnection, InvokeError, Session, SessionEnd, completion_invocation_id};
use crate::relay::presence;
use phonelink_core::codec::hub;
use phonelink_core::codec::hub::{CompletionResult, HubCodecError, HubMessage};
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use phonelink_core::types::Environment;
use rmpv::Value;
use std::sync::Arc;

impl HubConnection {
    /// Splits every complete frame off `buf` and handles it. A length prefix that fails the
    /// `VarInt` rules desynchronizes the stream: the rest of the buffered bytes are dropped (logged)
    /// and reading resumes with the next WebSocket message, so the connection stays up (Phone Link on
    /// Windows ends it).
    pub(super) fn process_buffer(self: &Arc<Self>, session: &Arc<Session>, buf: &mut Vec<u8>) -> Option<SessionEnd> {
        let mut consumed = 0;
        let mut end = None;
        while end.is_none() {
            match hub::read_frame(&buf[consumed..]) {
                Ok(None) => break,
                Ok(Some((payload, used))) => {
                    end = self.handle_frame(session, payload);
                    consumed += used;
                }
                Err(error) => {
                    self.shared.undecodable(self.params.env, "hub frame", &error.to_string(), Some(&buf[consumed..]));
                    consumed = buf.len();
                }
            }
        }

        buf.drain(..consumed);
        end
    }

    pub(super) fn handle_frame(self: &Arc<Self>, session: &Arc<Session>, payload: &[u8]) -> Option<SessionEnd> {
        let env = self.params.env;
        let value = self.tap_frame(payload);

        match hub::decode_message(payload) {
            Ok(HubMessage::Invocation { target, arguments, invocation_id, .. }) => {
                presence::dispatch(self, session.id, &target, arguments, invocation_id);
                None
            }
            Ok(HubMessage::Completion { invocation_id, result, .. }) => {
                complete_invocation(session, env, invocation_id, result);
                None
            }
            Ok(HubMessage::Ping) => None,
            Ok(HubMessage::Close { error, allow_reconnect }) => {
                Some(SessionEnd::ServerClose { error, allow_reconnect })
            }
            Ok(HubMessage::Unknown { message_type }) => {
                tracing::warn!(env = %env, message_type, "hub message of unknown type skipped");
                None
            }
            Ok(other) => {
                tracing::debug!(env = %env, message_type = other.type_code(), "hub message of a type the client never uses ignored");
                None
            }
            Err(HubCodecError::UnknownCompletionKind(kind)) => {
                self.on_unknown_completion_kind(session, payload, value.ok().as_ref(), kind);
                None
            }
            Err(error) => {
                self.shared.undecodable(env, "hub message", &error.to_string(), Some(payload));
                None
            }
        }
    }

    /// Decodes a frame's `MessagePack` value and taps it (or its decoding error).
    fn tap_frame(&self, payload: &[u8]) -> Result<Value, HubCodecError> {
        let value = hub::from_bytes(payload);
        let summary = match &value {
            Ok(v) => hub::value_to_json(v),
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };

        self.shared.tap.emit(TapEvent::new(
            TapLayer::HubMessage,
            Direction::In,
            Some(self.params.env.name().to_string()),
            summary,
            Some(payload),
        ));
        value
    }

    /// A completion of an unknown result kind: undecodable, and the invocation it answers (when its
    /// id can be read from `value`) fails instead of waiting forever.
    fn on_unknown_completion_kind(&self, session: &Session, payload: &[u8], value: Option<&Value>, kind: i64) {
        let error = format!("unknown completion result kind {kind}");
        self.shared.undecodable(self.params.env, "hub completion", &error, Some(payload));

        // The invocation it answers fails instead of waiting forever.
        if let Some(id) = value.and_then(completion_invocation_id) {
            session.complete(&id, Err(InvokeError::Failed(error)));
        }
    }
}

/// Completes the pending invocation a hub completion answers.
fn complete_invocation(session: &Session, env: Environment, invocation_id: String, result: CompletionResult) {
    let outcome = match result {
        CompletionResult::Error(text) => Err(InvokeError::Failed(text)),
        CompletionResult::Void | CompletionResult::Value(_) => Ok(()),
    };

    if !session.complete(&invocation_id, outcome) {
        tracing::debug!(env = %env, invocation_id, "completion for no pending invocation");
    }
}
