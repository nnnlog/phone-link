//! The messaging contribution to the connect message: `configuration` keys and the current anchors
//! out; expected anchors, the messages permission, `capabilities.messaging` and
//! `settings.mmsAutoDownloadEnabled` in.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{error, warn};

use phonelink_app::consts::{capability, permission};
use phonelink_app::{ConnectContext, ConnectContribution, ConnectContributor};
use phonelink_core::codec::valueset::{FromValue, Value, ValueSet};
use phonelink_features::shared::db::StoreError;

use super::caps::{self, Predicates};
use super::consts::{anchor, key};
use super::outbound::configuration;
use super::store;
use super::{Inner, MessagingEvent};

pub(super) struct Contributor {
    inner: Arc<Inner>,
}

impl Contributor {
    pub(super) fn new(inner: &Arc<Inner>) -> Self {
        Self { inner: Arc::clone(inner) }
    }

    /// The provider's keys. A store that cannot be read loses only its stored values: the
    /// configuration is computed as for a phone that has reported no capabilities, and the sequence
    /// numbers are left out — never invented.
    fn build(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        let inner = &self.inner;
        let stored = match inner.db(ctx.phone).and_then(|db| db.read(store::phone_state)) {
            Ok(state) => state.capabilities,
            Err(e) => {
                error!(error = %e, "messaging store not read; the connect configuration uses no stored capabilities");
                None
            }
        };

        let p =
            Predicates::new(inner.effective_caps(ctx.phone, stored, ctx.record), inner.contract_version(ctx.record));
        let mut c = ConnectContribution { configuration: configuration(&p), ..ConnectContribution::default() };

        if !ctx.record.has_capability(capability::MESSAGES) {
            return c;
        }

        match self.sequence_numbers(ctx, &p) {
            Ok(s) => c.sequence_numbers = s,
            Err(e) => {
                error!(error = %e, "messaging store not read; the connect leaves the messaging sequence numbers out")
            }
        }

        c
    }

    /// `sequenceNumbers` of a messaging-capable phone: the stored current anchors, `-1` when none.
    fn sequence_numbers(&self, ctx: &ConnectContext<'_>, p: &Predicates) -> Result<ValueSet, StoreError> {
        let db = self.inner.db(ctx.phone)?;
        let (forced, current) = db.read(|conn| {
            let forced = p.participants() && store::participants_empty(conn)?;
            let mut current = std::collections::HashMap::new();
            for t in anchor::ALL {
                current.insert(t, store::current_anchor(conn, t)?);
            }
            Ok((forced, current))
        })?;

        let value = |t: &'static str| current.get(t).copied().unwrap_or(-1);
        // The participants forcing rule: without stored participants a phone that tracks them is
        // asked for every SMS/MMS (and RCS chat and file transfer) again.
        let messages = |t: &'static str| if forced { -1 } else { value(t) };

        let mut s = ValueSet::new();
        s.insert(anchor::MMS, messages(anchor::MMS));
        s.insert(anchor::SMS, messages(anchor::SMS));
        if p.rcs_receive() {
            s.insert(anchor::RCS_CHAT, messages(anchor::RCS_CHAT));
            s.insert(anchor::RCS_FILE_TRANSFER, messages(anchor::RCS_FILE_TRANSFER));
        }
        s.insert(anchor::CONVERSATION, value(anchor::CONVERSATION));
        s.insert(anchor::SUBSCRIPTION, value(anchor::SUBSCRIPTION));
        if p.rcs_receive() {
            s.insert(anchor::RCS_CONVERSATION, value(anchor::RCS_CONVERSATION));
        }

        Ok(s)
    }

    fn apply_response(&self, ctx: &ConnectContext<'_>, response: &ValueSet) -> Result<(), StoreError> {
        let inner = &self.inner;
        let db = inner.db(ctx.phone)?;
        let state = db.read(store::phone_state)?;
        let contract = inner.contract_version(ctx.record);
        let before = Predicates::new(inner.effective_caps(ctx.phone, state.capabilities, ctx.record), contract);

        // Expected anchors — read before `capabilities.messaging` is applied.
        let capable = ctx.record.has_capability(capability::MESSAGES);
        let result_ok = matches!(response.get(key::RESULT), Some(Value::Int32(0)));
        let mut expected = Vec::new();
        if capable && result_ok {
            let mut types = vec![anchor::SMS, anchor::MMS, anchor::CONVERSATION, anchor::SUBSCRIPTION];
            if before.rcs_receive() {
                types.extend(anchor::RCS);
            }
            for t in types {
                let v = match response.get(t) {
                    Some(Value::Int64(v)) => *v,
                    Some(other) => {
                        warn!(wire_key = t, value_type = other.type_name(), "connect anchor not Int64; stored as -1");
                        -1
                    }
                    None => -1,
                };
                expected.push((t, v));
            }
        }

        let permission_granted = object(response, key::PERMISSIONS).and_then(|p| typed::<bool>(p, key::MESSAGING));
        let reported = object(response, key::CAPABILITIES).and_then(|c| typed::<i32>(c, key::MESSAGING));
        let auto_download =
            object(response, key::SETTINGS).and_then(|s| typed::<bool>(s, key::MMS_AUTO_DOWNLOAD_ENABLED));

        db.transaction(|tx| -> Result<(), StoreError> {
            for (t, v) in &expected {
                store::set_expected_anchor(tx, t, *v)?;
            }
            if let Some(v) = reported {
                store::set_capabilities(tx, caps::from_connect(v))?;
            }
            if let Some(on) = auto_download {
                store::set_mms_auto_download_on_phone(tx, on)?;
            }
            Ok(())
        })?;

        if let Some(granted) = permission_granted {
            ctx.app.update_phone_record(|r| r.set_permission(permission::MESSAGES, granted));
        }

        let after_caps = reported.map_or(before.capabilities, caps::from_connect);
        if after_caps != before.capabilities {
            inner.after_capabilities_change(ctx.phone, before, Predicates::new(after_caps, contract));
        } else if auto_download.is_some_and(|on| on != state.mms_auto_download_on_phone) {
            inner.emit(MessagingEvent::SettingsChanged { phone: ctx.phone.clone() });
        }
        if !expected.is_empty() {
            inner.refresh_sync_state(ctx.phone);
        }

        Ok(())
    }
}

fn object<'a>(vs: &'a ValueSet, k: &str) -> Option<&'a ValueSet> {
    match vs.get(k) {
        Some(Value::Object(o)) => Some(o),
        Some(other) => {
            warn!(wire_key = k, value_type = other.type_name(), "connect reply key of another type ignored");
            None
        }
        None => None,
    }
}

/// The value of `k` if it has type `T`; a key of another type is logged and ignored.
fn typed<'a, T: FromValue<'a>>(vs: &'a ValueSet, k: &str) -> Option<T> {
    vs.read_checked(k).unwrap_or_else(|e| {
        warn!(wire_key = k, value_type = e.found, "connect reply key of another type ignored");
        None
    })
}

#[async_trait]
impl ConnectContributor for Contributor {
    async fn contribute(&self, ctx: &ConnectContext<'_>) -> ConnectContribution {
        self.build(ctx)
    }

    async fn apply(&self, ctx: &ConnectContext<'_>, response: &ValueSet) {
        if let Err(e) = self.apply_response(ctx, response) {
            error!(error = %e, "messaging connect reply not applied; the store failed");
        }
    }
}
