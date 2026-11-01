//! The `contactsV2` content processor: the reconciliation of a metadata message and its response
//! (PC → phone), item messages, sessions, and inbound error handling.
//!
//! A sync session is the processor the App layer keeps in its unfinished-processor table under
//! `(phone, syncID or the request's correlation id)`: a metadata message that requests ids is
//! stored, replacing a session with that key; one that requests nothing completes at once and
//! leaves a stored session untouched; an item with no stored session gets a fresh processor and so
//! completes at once. The session holds the per-type pending sets and the last `sequenceNumbers`
//! object it received.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info};

use phonelink_app::{ContentProcessor, ContentProcessorFactory, InboundContext, Processed, Rejection, simple_answer};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::StoreError;

use super::parse::{SequenceNumbers, parse_item, parse_metadata};
use super::store::Completion;
use super::wire::{CONTENT_TYPE, RecordType, key};
use super::{ContactsEvent, Shared};
use phonelink_app::Malformed;

/// The reason a message is refused when the store fails under it (nothing of it is applied).
const STORE_FAILURE: &str = "contacts store failure";

/// Creates a session for every `contactsV2` message that opens one: a metadata message, or an item
/// with no stored session. Either needs a `correlationVector` key (the envelope header copied in when
/// the payload lacks one). A value of another `DataValueType` counts as present and is read as none:
/// the message is processed, and a metadata answer carries `correlationVector` as `Empty`.
pub(super) struct ContactsFactory {
    pub(super) shared: Arc<Shared>,
}

impl ContentProcessorFactory for ContactsFactory {
    fn create(
        &self,
        ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        let correlation_vector = match payload.get(key::CORRELATION_VECTOR) {
            Some(Value::String(cv)) => Some(cv.clone()),
            Some(other) => {
                debug!(value_type = other.type_name(), "contactsV2 correlationVector is not a String; read as none");
                None
            }
            // The App layer logs the rejection.
            None => return Err(Rejection::new("contactsV2 without correlationVector")),
        };

        Ok(Some(Box::new(Session {
            shared: self.shared.clone(),
            phone: ctx.phone.clone(),
            correlation_vector,
            pending: Default::default(),
            sequence_numbers: SequenceNumbers::NONE,
        })))
    }
}

/// One sync session.
struct Session {
    shared: Arc<Shared>,
    phone: DcgClient,
    /// The opening message's `correlationVector`; `None` when it was not a `String`.
    correlation_vector: Option<String>,
    /// The requested ids not yet received, by [`RecordType`].
    pending: [HashSet<i64>; 6],
    /// The last `sequenceNumbers` object of the session (`-1` each before one arrives).
    sequence_numbers: SequenceNumbers,
}

/// The App layer logs the rejection, with its key.
fn refuse(e: Malformed) -> Processed {
    Processed::Rejected(e.rejection().at_key(e.key))
}

/// The App layer logs the rejection, with the store's error.
fn store_failure(e: &StoreError) -> Processed {
    Processed::Rejected(Rejection::new(STORE_FAILURE).because(e))
}

impl Session {
    fn all_received(pending: &[HashSet<i64>; 6]) -> bool {
        pending.iter().all(HashSet::is_empty)
    }
}

#[async_trait]
impl ContentProcessor for Session {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let msg = match parse_metadata(payload) {
            Ok(m) => m,
            Err(e) => return refuse(e),
        };

        let sequence_numbers = msg.sequence_numbers.unwrap_or(self.sequence_numbers);
        let now_ms = self.shared.now_ms();
        let store = match self.shared.store(&self.phone) {
            Ok(s) => s,
            Err(e) => return store_failure(&e),
        };
        let requested = match store.apply_metadata(&msg, |requested| {
            requested.iter().all(Vec::is_empty).then_some(Completion { sequence_numbers, now_ms })
        }) {
            Ok(r) => r,
            Err(e) => return store_failure(&e),
        };

        // The store change is committed: the rest of the message applies.
        if let Some(capabilities) = msg.contact_capabilities {
            ctx.app.update_record_for(&self.phone, |r| r.contacts_capabilities = Some(capabilities));
        }
        self.sequence_numbers = sequence_numbers;
        for t in RecordType::ALL {
            self.pending[t.index()] = requested[t.index()].iter().copied().collect();
        }

        let total: usize = requested.iter().map(Vec::len).sum();
        let completed = total == 0;
        self.shared.records_changed(&self.phone);
        if completed {
            self.shared.emit(ContactsEvent::SyncStateChanged { phone: self.phone.clone() });
            info!("contacts sync finished; nothing requested");
        } else {
            info!(requested_count = total, "contacts sync started");
        }

        // A value read as none is serialized as `Empty`.
        let correlation_vector = self.correlation_vector.clone().map_or(Value::Empty, Value::String);
        let mut answer = ValueSet::new()
            .with(key::CORRELATION_VECTOR, correlation_vector)
            .with(key::CONTENT_TYPE, CONTENT_TYPE)
            .with(key::CONTRACT_VERSION, ctx.app.standard_keys().contract_version)
            .with(key::ITEM_COUNT, i32::try_from(total).unwrap_or(i32::MAX));
        for (t, ids) in RecordType::ALL.iter().zip(requested) {
            answer.insert(t.ids_key(), Value::Int64Array(ids));
        }
        answer.insert(key::RESULT, 0i32);
        Processed::Answer { answer, completed }
    }

    async fn content(&mut self, _ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let item = match parse_item(payload) {
            Ok(i) => i,
            Err(e) => return refuse(e),
        };

        let mut pending = self.pending.clone();
        for t in RecordType::ALL {
            for id in item.processed(t) {
                pending[t.index()].remove(id);
            }
        }

        let sequence_numbers = item.sequence_numbers.unwrap_or(self.sequence_numbers);
        let completed = Self::all_received(&pending);
        let completion = completed.then(|| Completion { sequence_numbers, now_ms: self.shared.now_ms() });

        let store = match self.shared.store(&self.phone) {
            Ok(s) => s,
            Err(e) => return store_failure(&e),
        };
        if let Err(e) = store.apply_item(&item, completion) {
            return store_failure(&e);
        }

        self.pending = pending;
        self.sequence_numbers = sequence_numbers;
        if item.has_entries() {
            self.shared.records_changed(&self.phone);
        }
        if completed {
            self.shared.emit(ContactsEvent::SyncStateChanged { phone: self.phone.clone() });
            info!("contacts sync finished");
        }
        debug!(completed, "contactsV2 item applied");
        Processed::Answer { answer: simple_answer(0), completed }
    }
}
