//! Call history's contribution to the connect message.
//!
//! Request: `configuration` › `CallLogs` = "use calls on this Mac" ∧ an Android phone, always; with
//! the phone's `CallLogs` capability and that flag, `sequenceNumbers` › `call_logs` (the stored
//! `current`, `-1` when none) — left out when the store cannot be read, rather than sent as an
//! invented value. Response: with the same two conditions and `result` `Int32 0`, its `call_logs`
//! (`Int64`, else `-1`) is stored as `expected`; `permissions` › `calls` and `call_logs` (`Bool`) set
//! the two call permissions whatever `result` is.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use phonelink_app::consts::{key as app_key, permission};
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{Value, ValueSet};

use super::wire::key;
use super::{CallingEvent, Shared, gates};
use crate::store;

/// The provider registered for [`phonelink_app::ConnectProvider::Calling`].
pub(crate) struct Contributor {
    pub(crate) shared: Arc<Shared>,
}

impl Contributor {
    /// `configuration.CallLogs`: the stored consent for an Android phone; `false` for another
    /// platform or when the consent cannot be read.
    fn sync_enabled(&self, ctx: &ConnectContext<'_>) -> bool {
        if !gates::is_android(ctx.record) {
            return false;
        }

        match self.shared.db(ctx.phone).and_then(|db| store::state(&db)) {
            Ok(s) => s.pc_consent,
            Err(e) => {
                error!(phone = %ctx.phone, error = %e, "call history store not read; the connect turns call-log sync off");
                false
            }
        }
    }
}

#[async_trait]
impl ConnectContributor for Contributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let enabled = self.sync_enabled(ctx);
        let mut c = ConnectContribution::default();
        c.configuration.insert(key::CONFIG_CALL_LOGS, enabled);

        if enabled && gates::call_logs_capable(ctx.record) {
            match self.shared.db(ctx.phone).and_then(|db| store::sync_state(&db)) {
                Ok(s) => c.sequence_numbers.insert(key::SEQ_CALL_LOGS, Value::Int64(s.current)),
                // An unreadable store is not "nothing stored": no value is stated.
                Err(e) => {
                    error!(phone = %ctx.phone, error = %e, "call history store not read; the connect leaves the sequence number out")
                }
            }
        }
        c
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        let read_expected = gates::call_logs_capable(ctx.record) && self.sync_enabled(ctx);
        if read_expected && response.get(app_key::RESULT) == Some(&Value::Int32(0)) {
            let expected = response.i64(key::SEQ_CALL_LOGS).unwrap_or(-1);
            match self.shared.db(ctx.phone).and_then(|db| store::set_expected(&db, expected)) {
                Ok(()) => self.shared.emit(CallingEvent::StateChanged { phone: ctx.phone.clone() }),
                Err(e) => {
                    error!(phone = %ctx.phone, error = %e, "call history expected sequence number not stored")
                }
            }
        }

        // Whatever `result` is.
        let Some(Value::Object(permissions)) = response.get(key::PERMISSIONS) else { return };
        for (name, bit) in
            [(key::PERMISSIONS_CALLS, permission::CALLS), (key::PERMISSIONS_CALL_LOGS, permission::CALL_LOGS)]
        {
            match permissions.get(name) {
                Some(Value::Bool(granted)) => {
                    let granted = *granted;
                    ctx.app.update_record_for(ctx.phone, |r| r.set_permission(bit, granted));
                }
                Some(other) => warn!(
                    value_type = other.type_name(),
                    wire_key = name,
                    "connect call permission is not a Bool; ignored"
                ),
                None => {}
            }
        }
    }
}
