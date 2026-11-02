//! The inbound `callLogs` processor: the metadata message, item messages, completion and the
//! sequence number.
//!
//! A metadata message reconciles the call table against the phone's id list in one transaction and
//! the answer names the ids the PC wants. The processor then stays in the App layer's
//! unfinished-processor table while an id is pending; each item message removes the ids it carries.
//! An item message with no open sync is processed on its own (an incremental push) and completes at
//! once. A refused message changes nothing and is answered `{ result: 1 }`.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, error, info};

use phonelink_app::consts::key as app_key;
use phonelink_app::{
    ContentProcessor, ContentProcessorFactory, InboundContext, Malformed, Processed, Rejection, simple_answer,
};
use phonelink_core::codec::valueset::ValueSet;
use phonelink_features::shared::db::FeatureDb;

use super::store;
use super::wire;
use super::{CallingEvent, Shared};

/// The reason a message is refused when the store fails under it.
const STORE_FAILURE: &str = "call history store failure";

/// The factory of `callLogs` processors.
pub(crate) struct Factory {
    pub(crate) shared: Arc<Shared>,
}

impl ContentProcessorFactory for Factory {
    /// The message that opens a processor needs a `correlationVector` key; a value of another type
    /// counts as present and is answered as `Empty`.
    fn create(
        &self,
        _ctx: &InboundContext<'_>,
        payload: &ValueSet,
    ) -> Result<Option<Box<dyn ContentProcessor>>, Rejection> {
        if !payload.contains_key(app_key::CORRELATION_VECTOR) {
            // The App layer logs the rejection.
            return Err(Rejection::new("callLogs without correlationVector"));
        }
        Ok(Some(Box::new(Processor { shared: self.shared.clone(), pending: HashSet::new(), sequence: None })))
    }
}

/// One sync (or one item message processed on its own).
struct Processor {
    shared: Arc<Shared>,
    /// The wanted ids not yet received.
    pending: HashSet<i64>,
    /// The last `sequenceNumber` of the transaction.
    sequence: Option<i64>,
}

/// The App layer logs the rejection, with its key.
fn refuse(e: Malformed) -> Processed {
    Processed::Rejected(e.rejection())
}

/// The App layer logs the rejection, with the store's error.
fn store_failure(e: &rusqlite::Error) -> Processed {
    Processed::Rejected(Rejection::new(STORE_FAILURE).because(e))
}

impl Processor {
    fn db(&self, ctx: &InboundContext<'_>) -> Result<Arc<FeatureDb>, Processed> {
        self.shared.db(ctx.phone).map_err(|e| {
            error!(phone = %ctx.phone, error = %e, "call history store not opened; the message is rejected");
            Processed::Rejected(Rejection::new(STORE_FAILURE))
        })
    }
}

#[async_trait]
impl ContentProcessor for Processor {
    async fn metadata(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let m = match wire::parse_metadata(payload) {
            Ok(m) => m,
            Err(e) => return refuse(e),
        };

        let db = match self.db(ctx) {
            Ok(db) => db,
            Err(p) => return p,
        };

        let every_listed = self.shared.config.request_every_listed_id;
        let now = self.shared.now_ms();
        let synced = db.transaction(|tx| {
            let s = store::synchronize(tx, &m.ids, &m.last_updated, every_listed)?;
            if s.wanted.is_empty() {
                store::complete(tx, now, m.sequence)?;
            }
            Ok::<_, rusqlite::Error>(s)
        });
        let synced = match synced {
            Ok(s) => s,
            Err(e) => return store_failure(&e),
        };

        self.pending = synced.wanted.iter().copied().collect();
        self.sequence = m.sequence;
        if synced.deleted > 0 {
            self.shared.emit(CallingEvent::HistoryChanged { phone: ctx.phone.clone() });
        }

        let answer = wire::metadata_reply(
            m.correlation_vector.as_deref(),
            ctx.app.standard_keys().contract_version,
            &synced.wanted,
        );
        if synced.wanted.is_empty() {
            info!(listed_count = m.ids.len(), "call history sync finished; nothing requested");
            self.shared.emit(CallingEvent::StateChanged { phone: ctx.phone.clone() });
            Processed::completed(answer)
        } else {
            info!(listed_count = m.ids.len(), requested_count = synced.wanted.len(), "call history sync started");
            Processed::open(answer)
        }
    }

    async fn content(&mut self, ctx: &InboundContext<'_>, payload: &ValueSet) -> Processed {
        let items = match wire::parse_items(payload) {
            Ok(i) => i,
            Err(e) => return refuse(e),
        };

        let db = match self.db(ctx) {
            Ok(db) => db,
            Err(p) => return p,
        };

        let mut pending = self.pending.clone();
        for id in items.ids() {
            pending.remove(&id);
        }
        let sequence = items.sequence.or(self.sequence);
        let completed = pending.is_empty();

        let now = self.shared.now_ms();
        let applied = db.transaction(|tx| {
            store::apply_items(tx, &items)?;
            if completed {
                store::complete(tx, now, sequence)?;
            }
            Ok::<_, rusqlite::Error>(())
        });
        if let Err(e) = applied {
            return store_failure(&e);
        }

        self.pending = pending;
        self.sequence = sequence;
        self.shared.emit(CallingEvent::HistoryChanged { phone: ctx.phone.clone() });
        debug!(
            stored_count = items.upserts.len(),
            deleted_count = items.deletes.len(),
            completed,
            "callLogs items applied"
        );
        if completed {
            info!("call history sync finished");
            self.shared.emit(CallingEvent::StateChanged { phone: ctx.phone.clone() });
        }
        Processed::Answer { answer: simple_answer(0), completed }
    }
}
