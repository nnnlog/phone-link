//! Inbound App requests, from the platform router onwards.
//!
//! It covers the dispatch (the device proxy before it is the router's), the unfinished-processor
//! table, the response construction, and the handling of unknown or unsupported inbound traffic:
//! log, keep the session, answer as the protocol requires.
//!
//! [`AppInbound::on_app_message`] queues each message; one worker task (started by
//! [`AppLayer::start`]) processes them in arrival order, up to and including sending the answer. A
//! panic while one message is processed (a feature bug) is isolated to that message: it is refused
//! as malformed content would be and the worker goes on with the next one, keeping the shared
//! connection.

use std::panic::AssertUnwindSafe;

use futures_util::FutureExt;
use serde_json::json;
use tracing::{debug, error, warn};

use phonelink_core::codec::valueset::{self, ValueSet};
use phonelink_core::seams::AppInbound;
use phonelink_core::tap::{Direction, TapEvent, TapLayer};
use phonelink_core::types::{DcgClient, DcgSendStatus, HeaderMap, MessageType, TraceContext, TransportMessage, header};

use super::AppLayer;
use super::consts::{content_type, key, priority, result_code, route};
use super::message::{AppMessage, read_app_message};
use super::processor::{ContentProcessor, InboundContext, Processed, Rejection, simple_answer};
use super::request::tap_summary;
use phonelink_core::sync::MutexExt;

/// One queued inbound message.
pub(crate) struct InboundItem {
    pub source: DcgClient,
    pub trace: TraceContext,
    pub message: TransportMessage,
}

/// Why an inbound message got no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropReason {
    /// Not from the selected phone (or no phone is selected).
    NotSelectedPhone,
    /// The payload is not a `ValueSet` and the headers do not carry `_requestId` with
    /// `_route = /legacy/phonecontent`.
    Unparseable,
    /// No route in the header or the payload.
    NoRoute,
    /// A route other than `/legacy/phonecontent` (including a payload-only `/internal/response`).
    OtherRoute,
    /// `/legacy/phonecontent` without `_requestId` (header or payload): nothing applied.
    NoRequestId,
}

/// What [`AppLayer::handle_inbound`] did with one message.
#[derive(Debug, Clone, PartialEq)]
pub enum InboundDisposition {
    /// This answer was sent to the phone.
    Answered(ValueSet),
    Dropped(DropReason),
}

/// Key of the unfinished-processor table: `(phone id, syncId)`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct TransactionKey {
    pub phone: DcgClient,
    pub sync_id: String,
}

/// The transaction key's sync id: the payload's `String` `syncID`, else the `correlationVector`
/// header (after the lift) — empty when there is none, so a missing header and an empty one key the
/// same transaction.
fn transaction_sync_id(values: &ValueSet, headers: &HeaderMap) -> String {
    values.str(key::SYNC_ID).or_else(|| headers.get(header::CORRELATION_VECTOR)).unwrap_or_default().to_string()
}

/// Checks that a readable message is a `/legacy/phonecontent` request with a `_requestId`; the
/// reason it is dropped otherwise.
fn check_phone_content_request(msg: &AppMessage) -> Result<(), DropReason> {
    match msg.route() {
        None => {
            warn!(header_count = msg.headers.len(), "inbound App message without a route; dropped");
            return Err(DropReason::NoRoute);
        }
        Some(r) if r != route::PHONE_CONTENT => {
            warn!(raw_route = r, "inbound App message on an unhandled route; dropped");
            return Err(DropReason::OtherRoute);
        }
        Some(_) => {}
    }

    if msg.request_id().is_none() {
        warn!(raw_content_type = ?msg.content_type(), "phone content without _requestId; nothing applied, no answer");
        return Err(DropReason::NoRequestId);
    }
    Ok(())
}

impl AppInbound for AppLayer {
    fn on_app_message(
        &self,
        source: DcgClient,
        _connection_session_id: Option<String>,
        trace: TraceContext,
        message: TransportMessage,
    ) {
        if self.inbound_tx.send(InboundItem { source, trace, message }).is_err() {
            debug!("inbound App message dropped; the App layer is shutting down");
        }
    }
}

impl AppLayer {
    /// [`AppLayer::handle_inbound`] with a panic isolated to the message: logged, the message's
    /// transaction discarded and — when it is an answerable `/legacy/phonecontent` request —
    /// answered `{ result: 1 }`, as a message rejected as malformed content is answered.
    pub(crate) async fn handle_inbound_isolated(&self, item: InboundItem) {
        let InboundItem { source, trace, message } = item;
        let handled =
            AssertUnwindSafe(self.handle_inbound(source.clone(), trace.clone(), message.clone())).catch_unwind().await;
        let Err(panic) = handled else { return };

        error!(
            %source,
            raw_route = ?message.route(),
            panic = %phonelink_transport::platform::router::panic_text(panic.as_ref()),
            "processing an inbound App message panicked; the message is refused, the session continues"
        );

        let Ok(msg) = read_app_message(&message) else { return };
        if msg.route() != Some(route::PHONE_CONTENT) || msg.request_id().is_none() {
            return;
        }

        let Some(phone) = self.selected_phone().filter(|p| p.is_same_client(&source)) else { return };
        let sync_id = transaction_sync_id(&msg.values, &msg.headers);
        self.transactions.lock_or_recover().remove(&TransactionKey { phone: phone.clone(), sync_id });
        self.send_answer(&phone, &msg.headers, &message.payload, &simple_answer(result_code::FAILURE), trace).await;
    }

    /// Processes one inbound App message that is neither a response nor device-proxy traffic and
    /// sends its answer. The worker started by [`AppLayer::start`] calls this for every queued
    /// message, in arrival order.
    pub async fn handle_inbound(
        &self,
        source: DcgClient,
        trace: TraceContext,
        message: TransportMessage,
    ) -> InboundDisposition {
        // Inbound App requests are taken only from the selected phone.
        let Some(phone) = self.selected_phone().filter(|p| p.is_same_client(&source)) else {
            debug!(%source, raw_route = ?message.route(), "inbound App message not from the selected phone; dropped");
            return InboundDisposition::Dropped(DropReason::NotSelectedPhone);
        };

        // Read: gzip, parse, lift.
        let msg = match read_app_message(&message) {
            Ok(m) => m,
            Err(e) => return self.on_unreadable_message(&phone, &message, &e, trace).await,
        };
        self.tap.emit(TapEvent::new(
            TapLayer::App,
            Direction::In,
            Some(phone.to_string()),
            tap_summary("request", &msg.headers, Some(&msg.values)),
            Some(&message.payload),
        ));

        // Route.
        if let Err(reason) = check_phone_content_request(&msg) {
            return InboundDisposition::Dropped(reason);
        }
        // The engine is never blocked, and a missing phone record cannot occur here.

        // Contract version and lock.
        if let Some(refusal) = self.gate_phone_content(&msg) {
            return self.answer(&phone, &msg.headers, &message.payload, refusal, trace).await;
        }

        // Device-status keys, from every phone-content request.
        let mut values = msg.values.clone();
        self.update_record_for(&phone, |r| r.status_keys.record(&values));

        // A non-empty correlationVector header into the payload.
        if !values.contains_key(key::CORRELATION_VECTOR) {
            if let Some(cv) = msg.headers.get(header::CORRELATION_VECTOR).filter(|cv| !cv.is_empty()) {
                values.insert(key::CORRELATION_VECTOR, cv.to_string());
            }
        }

        // Transaction key.
        let sync_id = transaction_sync_id(&values, &msg.headers);

        // Phase: metadata or content.
        let classified = content_type::classify(msg.content_type());
        let ctx = InboundContext {
            app: self,
            phone: &phone,
            content_type: classified,
            raw_content_type: msg.content_type(),
            sync_id: Some(sync_id.as_str()),
            headers: &msg.headers,
            trace: &trace,
        };
        let txn = TransactionKey { phone: phone.clone(), sync_id: sync_id.clone() };

        debug!(
            content_type = ctx.content_type,
            is_metadata = values.contains_key(key::METADATA),
            "phone content dispatched"
        );
        let answer = self.dispatch(&ctx, txn, &values).await;
        self.answer(&phone, &msg.headers, &message.payload, answer, trace.clone()).await
    }

    /// Handles a message whose payload is not a `ValueSet`: tapped as undecodable, then answered
    /// `{ result: 1 }` when it is an answerable `/legacy/phonecontent` request, else dropped.
    async fn on_unreadable_message(
        &self,
        phone: &DcgClient,
        message: &TransportMessage,
        e: &valueset::ValueSetError,
        trace: TraceContext,
    ) -> InboundDisposition {
        self.tap.emit(TapEvent::new(
            TapLayer::Undecodable,
            Direction::In,
            Some(phone.to_string()),
            json!({"layer": "app", "kind": "request", "error": e.to_string(),
                   "headers": message.headers.iter().map(|(k, v)| [k, v]).collect::<Vec<_>>()}),
            Some(&message.payload),
        ));

        let h = &message.headers;
        if h.contains(header::REQUEST_ID) && h.get(header::ROUTE) == Some(route::PHONE_CONTENT) {
            warn!(error = %e, "phone content payload is not a ValueSet; answered 1");
            let answer = simple_answer(result_code::FAILURE);
            self.send_answer(phone, h, &message.payload, &answer, trace).await;
            return InboundDisposition::Answered(answer);
        }

        warn!(error = %e, raw_route = ?h.get(header::ROUTE), "inbound App payload is not a ValueSet; dropped");
        InboundDisposition::Dropped(DropReason::Unparseable)
    }

    /// The refusal a phone-content request gets for an incompatible contract version or a locked
    /// session, checked in that order; `None` when it is processed.
    fn gate_phone_content(&self, msg: &AppMessage) -> Option<ValueSet> {
        // Contract version (missing or non-numeric reads 0).
        let v = msg.contract_version().value_or_zero();
        if v.trunc() != -1.0 && v.floor() != 3.0 {
            warn!(contract_version = v, "phone content with an incompatible contract version; answered 3");
            return Some(simple_answer(result_code::INCOMPATIBLE_VERSIONS));
        }

        // Lock.
        if self.is_locked() && (v.trunc() == -1.0 || v >= 3.8) {
            debug!("phone content while the session is locked; answered 19");
            return Some(simple_answer(result_code::USER_SESSION_INACTIVE));
        }

        None
    }

    async fn dispatch(&self, ctx: &InboundContext<'_>, txn: TransactionKey, values: &ValueSet) -> ValueSet {
        if values.contains_key(key::METADATA) {
            let Some(factory) = self.factory_for(ctx.content_type) else {
                warn!(raw_content_type = ?ctx.raw_content_type, "metadata for a type without a processor; answered 2");
                return simple_answer(result_code::UNKNOWN_TYPE);
            };

            let mut processor = match factory.create(ctx, values) {
                Ok(Some(p)) => p,
                Ok(None) => return simple_answer(result_code::FAILURE),
                Err(rejection) => return self.rejected(ctx, &txn, rejection),
            };

            let processed = processor.metadata(ctx, values).await;
            self.finish(ctx, txn, processor, processed)
        } else {
            let mut processor = match self.take_transaction(&txn) {
                Some(p) => p,
                None => {
                    let Some(factory) = self.factory_for(ctx.content_type) else {
                        warn!(
                            raw_content_type = ?ctx.raw_content_type,
                            "content for a type without a processor; answered 1"
                        );
                        return simple_answer(result_code::FAILURE);
                    };
                    match factory.create(ctx, values) {
                        Ok(Some(p)) => p,
                        Ok(None) => return simple_answer(result_code::FAILURE),
                        Err(rejection) => return self.rejected(ctx, &txn, rejection),
                    }
                }
            };

            let processed = processor.content(ctx, values).await;
            self.finish(ctx, txn, processor, processed)
        }
    }

    fn finish(
        &self,
        ctx: &InboundContext<'_>,
        txn: TransactionKey,
        processor: Box<dyn ContentProcessor>,
        processed: Processed,
    ) -> ValueSet {
        match processed {
            Processed::Answer { answer, completed } => {
                if !completed {
                    self.transactions.lock_or_recover().insert(txn, processor);
                }
                answer
            }
            Processed::Rejected(rejection) => self.rejected(ctx, &txn, rejection),
        }
    }

    /// A message rejected as malformed content: the transaction is discarded, the message logged.
    fn rejected(&self, ctx: &InboundContext<'_>, txn: &TransactionKey, rejection: Rejection) -> ValueSet {
        self.transactions.lock_or_recover().remove(txn);
        log_rejection(ctx, txn, &rejection);
        rejection.answer
    }

    fn take_transaction(&self, txn: &TransactionKey) -> Option<Box<dyn ContentProcessor>> {
        self.transactions.lock_or_recover().remove(txn)
    }

    /// Sends `answer` and reports it.
    async fn answer(
        &self,
        phone: &DcgClient,
        request_headers: &HeaderMap,
        request_payload: &[u8],
        answer: ValueSet,
        trace: TraceContext,
    ) -> InboundDisposition {
        self.send_answer(phone, request_headers, request_payload, &answer, trace).await;
        InboundDisposition::Answered(answer)
    }

    /// Sends `answer` as the response: `_originalRequestId`, `_route = /internal/response`,
    /// `result` when the answer has an `Int32` `result` (the router adds `_rejectionVersion`);
    /// priority 50.
    async fn send_answer(
        &self,
        phone: &DcgClient,
        request_headers: &HeaderMap,
        request_payload: &[u8],
        answer: &ValueSet,
        trace: TraceContext,
    ) {
        let Some(request_id) = request_headers.get(header::REQUEST_ID) else {
            // Callers check `_requestId` first; reaching this is a broken invariant.
            error!("App answer not sent; the request has no _requestId, which the callers check first");
            return;
        };

        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGINAL_REQUEST_ID, request_id);
        headers.insert(header::ROUTE, header::RESPONSE_ROUTE);
        if let Some(r) = answer.i32(key::RESULT) {
            headers.insert(header::RESULT, r.to_string());
        }

        let response = TransportMessage::new(MessageType::App, headers, valueset::encode(answer));
        // The request as the router sees it: the lifted headers carry `_requestId` even when the
        // phone sent it only in the payload.
        let request = TransportMessage::new(MessageType::App, request_headers.clone(), request_payload.to_vec());

        self.tap.emit(TapEvent::new(
            TapLayer::App,
            Direction::Out,
            Some(phone.to_string()),
            tap_summary("response", &response.headers, Some(answer)),
            Some(&response.payload),
        ));

        let status = self.messenger.respond(phone, &request, response, priority::RESPONSE, trace).await;
        if status == DcgSendStatus::Success {
            debug!(request_id, "App answer sent");
        } else {
            warn!(request_id, ?status, error = status.cause(), "App answer not delivered; the phone gets no answer");
        }
    }
}

/// The one line of a rejected message: a local failure is the engine's (`error`, with its cause), a
/// broken rule the phone's (`warn`).
fn log_rejection(ctx: &InboundContext<'_>, txn: &TransactionKey, rejection: &Rejection) {
    let (content_type, sync_id, reason) = (&ctx.raw_content_type, &txn.sync_id, rejection.reason);
    match &rejection.cause {
        Some(error) => {
            error!(raw_content_type = ?content_type, sync_id = ?sync_id, reason, %error, "phone content not applied; answered as rejected")
        }
        None => {
            warn!(raw_content_type = ?content_type, sync_id = ?sync_id, reason, wire_key = ?rejection.wire_key, "phone content rejected")
        }
    }
}
