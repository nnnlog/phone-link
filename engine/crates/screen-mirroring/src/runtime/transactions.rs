//! The orchestrator transactions that wait for their answer.

use super::{Inner, Transaction};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use tokio::sync::oneshot;
use tracing::{debug, warn};

impl Inner {
    pub(super) fn begin_transaction(&self, kind: Transaction) -> (String, oneshot::Receiver<Vec<u8>>) {
        let id = new_guid_lower();
        let (tx, rx) = oneshot::channel();
        self.state.lock_or_panic().transactions.insert(id.clone(), (kind, tx));
        (id, rx)
    }

    pub(super) fn complete_transaction(&self, id: Option<&str>, body: &[u8]) {
        let Some(id) = id else { return warn!("result without a transactionId; dropped") };
        match self.state.lock_or_panic().transactions.remove(id) {
            Some((_, tx)) => {
                let _ = tx.send(body.to_vec());
            }
            None => debug!(transaction = id, "result for no pending request; ignored"),
        }
    }
}
