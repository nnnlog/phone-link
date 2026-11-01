//! The identifiers the fragment layer allocates: one `MessageId` counter shared by every partner and
//! message kind, and per target `DcgClient` one `SessionId` and one `SequenceNumber` counter.
//!
//! They belong to the process and the PC identity, not to one [`super::DcgTransport`]: the engine
//! builds each identity's transport over [`DcgIdentifiers::process`] for that identity's client
//! type, so a core rebuilt in the same process (after an account removal or a failed feature
//! install) continues the counters and keeps each target's `SessionId`, and two identities talking
//! to the same phone never share a `SessionId`. A transport built over [`DcgIdentifiers::new`] has
//! counters of its own (tests that check first values).

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, new_guid_lower};

/// What the fragment layer keeps per target partner for the process lifetime.
pub(crate) struct TargetIds {
    /// One random lower-case GUID per target, created on first use.
    pub session_id: String,
    /// The next `SequenceNumber`; first value `1`, never reset.
    next_sequence: AtomicI32,
}

impl TargetIds {
    fn new() -> Self {
        Self { session_id: new_guid_lower(), next_sequence: AtomicI32::new(1) }
    }

    /// Allocated once per fragment submission, before its first attempt.
    pub(crate) fn next_sequence_number(&self) -> i32 {
        self.next_sequence.fetch_add(1, Ordering::SeqCst)
    }
}

/// The `MessageId` counter and the per-target `SessionId` / `SequenceNumber` state.
pub struct DcgIdentifiers {
    /// First value `1`.
    next_message_id: AtomicI32,
    targets: Mutex<HashMap<DcgClient, Arc<TargetIds>>>,
}

impl DcgIdentifiers {
    /// Identifiers of their own: the first `MessageId` and each target's first `SequenceNumber` are `1`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self { next_message_id: AtomicI32::new(1), targets: Mutex::new(HashMap::new()) })
    }

    /// The identifiers of this process for the PC identity `client_type`, created on first use.
    pub fn process(client_type: &str) -> Arc<Self> {
        static PROCESS: OnceLock<Mutex<HashMap<String, Arc<DcgIdentifiers>>>> = OnceLock::new();
        let mut map = PROCESS.get_or_init(Mutex::default).lock_or_recover();
        map.entry(client_type.to_string()).or_insert_with(DcgIdentifiers::new).clone()
    }

    pub(crate) fn next_message_id(&self) -> i32 {
        self.next_message_id.fetch_add(1, Ordering::SeqCst)
    }

    /// `target`'s state, created on first use.
    pub(crate) fn target(&self, target: &DcgClient) -> Arc<TargetIds> {
        let mut targets = self.targets.lock_or_panic();
        targets.entry(target.clone()).or_insert_with(|| Arc::new(TargetIds::new())).clone()
    }

    /// The `SessionId` the PC writes on its fragments to `target`.
    pub fn session_id(&self, target: &DcgClient) -> String {
        self.target(target).session_id.clone()
    }
}

#[cfg(test)]
mod tests;
