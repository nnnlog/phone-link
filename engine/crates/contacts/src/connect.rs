//! The contacts contribution to the connect message.
//!
//! Request: `configuration.ContactsV2Sync` and `configuration.ContactThumbnails` (`Bool true`) always;
//! the six *current* sequence numbers (`Int64`, `-1` when none) only for a contacts-capable phone.
//! Response, read at its top level: with a contacts-capable phone and `result` `Int32 0`, the six
//! *expected* sequence numbers (a missing or non-`Int64` key stores `-1`); `permissions.contacts`
//! (`Bool`) sets the contacts device permission whatever `result` is.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use phonelink_app::consts::{capability, permission};
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::wire::{RecordType, configuration_flags, key};
use super::{ContactsEvent, Shared};

pub(super) struct ContactsConnect {
    pub(super) shared: Arc<Shared>,
}

#[async_trait]
impl ConnectContributor for ContactsConnect {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let mut contribution = ConnectContribution { configuration: configuration_flags(), ..Default::default() };

        // The capability bits stored before this connect.
        if ctx.record.has_capability(capability::CONTACTS) {
            match self.shared.store(ctx.phone).and_then(|s| s.sequence_numbers()) {
                Ok(stored) => {
                    for t in RecordType::ALL {
                        let current = stored[t.index()].map_or(-1, |s| s.current);
                        contribution.sequence_numbers.insert(t.sequence_key(), current);
                    }
                }
                // No value to send: the keys are left out rather than invented.
                Err(e) => {
                    error!(error = %e, "contacts store not read; the connect leaves the contacts sequence numbers out")
                }
            }
        }

        contribution
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        // The capability bits after DeviceInfo applied this response.
        if ctx.record.has_capability(capability::CONTACTS) && response.get(key::RESULT) == Some(&Value::Int32(0)) {
            let expected = RecordType::ALL.map(|t| response.i64(t.sequence_key()).unwrap_or(-1));
            match self.shared.store(ctx.phone).and_then(|s| s.set_expected(expected)) {
                Ok(()) => self.shared.emit(ContactsEvent::SyncStateChanged { phone: ctx.phone.clone() }),
                Err(e) => {
                    error!(error = %e, "contacts expected sequence numbers not stored; the sync state is not updated")
                }
            }
        }

        // Whatever `result` is.
        if let Some(Value::Object(permissions)) = response.get(key::PERMISSIONS) {
            match permissions.get(key::PERMISSIONS_CONTACTS) {
                Some(Value::Bool(granted)) => {
                    let granted = *granted;
                    ctx.app.update_record_for(ctx.phone, |r| r.set_permission(permission::CONTACTS, granted));
                }
                Some(other) => {
                    warn!(value_type = other.type_name(), "connect permissions.contacts is not a Bool; ignored")
                }
                None => {}
            }
        }
    }
}
