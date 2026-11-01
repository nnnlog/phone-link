//! Content processors: the extension point inbound `contentType`s are dispatched to, and where
//! malformed content is rejected.
//!
//! A [`ContentProcessorFactory`] is registered for one or more catalog `contentType`s
//! ([`super::AppLayer::register_processor_factory`]). For each message that opens a transaction the
//! dispatcher asks the factory for a [`ContentProcessor`], runs its metadata or content phase, and
//! keeps it in the unfinished-processor table under `(phone, syncId)` while it reports *not
//! completed*. A content message whose key has a stored processor goes to that processor whatever
//! its own `contentType`.

use async_trait::async_trait;

use phonelink_core::codec::valueset::ValueSet;
use phonelink_core::types::{DcgClient, HeaderMap, TraceContext};

use super::AppLayer;
use super::consts::{E_FAIL_TEXT, key, result_code};

/// What the dispatcher hands a processor besides the payload.
pub struct InboundContext<'a> {
    /// The App layer, for state the processor updates (phone record, events) and for requests
    /// of its own ([`AppLayer::self_arc`] to spawn them).
    pub app: &'a AppLayer,
    /// The phone the message came from (the selected phone).
    pub phone: &'a DcgClient,
    /// The catalog name the message's `contentType` was classified as (`test` for unknown strings).
    pub content_type: &'static str,
    /// The `contentType` string as received (header or payload), when any.
    pub raw_content_type: Option<&'a str>,
    /// The transaction key's `syncId`: payload `syncID`, else the `correlationVector` header, `""`
    /// when there is none (always `Some`).
    pub sync_id: Option<&'a str>,
    /// The envelope headers after the lift.
    pub headers: &'a HeaderMap,
    /// The inbound message's trace context (the answer is sent with it).
    pub trace: &'a TraceContext,
}

impl InboundContext<'_> {
    /// The message's `correlationVector`: the payload `String` (after inbound dispatch copied a
    /// non-empty header in).
    pub fn correlation_vector<'v>(&self, payload: &'v ValueSet) -> Option<&'v str> {
        payload.str(key::CORRELATION_VECTOR)
    }
}

/// Inbound App content that breaks a rule of its contract.
///
/// `reason` is a fixed English phrase (never user content) and becomes the refusal's reason; `key`
/// names the key it concerns, for the log (empty when the reason names it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{reason}")]
pub struct Malformed {
    pub reason: &'static str,
    pub key: &'static str,
}

impl Malformed {
    pub const fn new(reason: &'static str, key: &'static str) -> Self {
        Self { reason, key }
    }

    /// Content whose `reason` names the key itself.
    pub const fn reason(reason: &'static str) -> Self {
        Self { reason, key: "" }
    }

    /// The `{ result: 1 }` refusal.
    pub fn rejection(&self) -> Rejection {
        let rejection = Rejection::new(self.reason);
        if self.key.is_empty() { rejection } else { rejection.at_key(self.key) }
    }
}

/// A message a processor refuses as malformed content.
///
/// Nothing of it is applied, its transaction is discarded, and it is answered `answer` —
/// `{ result: Int32 1 }` unless the feature defines another answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// A fixed English phrase naming the rule the message broke; never user content.
    pub reason: &'static str,
    pub answer: ValueSet,
    /// The wire key the rule is about, when there is one.
    pub wire_key: Option<&'static str>,
    /// The local failure (a store's error) that refused a well-formed message; the App layer logs
    /// it, so the feature does not.
    pub cause: Option<String>,
}

impl Rejection {
    /// Answered `{ result: Int32 1 }`.
    pub fn new(reason: &'static str) -> Self {
        Self::with_answer(reason, simple_answer(result_code::FAILURE))
    }

    /// Answered with the feature's own `answer`.
    pub fn with_answer(reason: &'static str, answer: ValueSet) -> Self {
        Self { reason, answer, wire_key: None, cause: None }
    }

    /// This rejection, about the wire key `key`.
    #[must_use]
    pub fn at_key(mut self, key: &'static str) -> Self {
        self.wire_key = Some(key);
        self
    }

    /// This rejection, caused by the local failure `cause` rather than by the message.
    #[must_use]
    pub fn because(mut self, cause: impl std::fmt::Display) -> Self {
        self.cause = Some(cause.to_string());
        self
    }

    /// The engine's single `message` text form for a rejection answer that carries `message`:
    /// `<short English reason> 80004005`.
    pub fn message_text(&self) -> String {
        rejection_message(self.reason)
    }
}

/// `<reason> 80004005` (`E_FAIL` as 8 upper-case hex digits).
pub fn rejection_message(reason: &str) -> String {
    format!("{reason} {E_FAIL_TEXT}")
}

/// A "simple" answer: exactly `{ result: Int32 <code> }`.
pub fn simple_answer(code: i32) -> ValueSet {
    ValueSet::new().with(key::RESULT, code)
}

/// The result of one phase.
#[derive(Debug, Clone, PartialEq)]
pub enum Processed {
    /// The message was applied; `answer` is sent; `completed` ends the transaction (else the
    /// processor stays stored under its key).
    Answer {
        answer: ValueSet,
        completed: bool,
    },
    Rejected(Rejection),
}

impl Processed {
    pub fn completed(answer: ValueSet) -> Self {
        Processed::Answer { answer, completed: true }
    }
    pub fn open(answer: ValueSet) -> Self {
        Processed::Answer { answer, completed: false }
    }
}

/// One transaction's processor. Implementations validate the whole message before they change
/// any state; a message they refuse is returned as [`Processed::Rejected`].
#[async_trait]
pub trait ContentProcessor: Send {
    /// The metadata phase (the payload has a `metadata` key, any type). The default refuses: a
    /// processor without a metadata phase rejects such a message (`heartbeat`).
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let _ = (ctx, payload);
        Processed::Rejected(Rejection::new("message type has no metadata phase"))
    }

    /// The content phase.
    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed;
}

/// Creates processors for the `contentType`s it was registered for.
pub trait ContentProcessorFactory: Send + Sync {
    /// Creates a processor from the message that opens a transaction. `Ok(None)` is "a factory that
    /// creates none" (answered `1`); `Err` refuses the message (nothing applied, transaction
    /// discarded).
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection>;
}

#[cfg(test)]
mod tests;
