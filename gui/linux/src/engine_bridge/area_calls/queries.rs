//! The calls part's answers to the views' queries.
//!
//! The contacts, a contact's card, the call history, call control and their states are each read from
//! the engine when asked.

use std::sync::Arc;

use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::calling::{ActiveCall, ActiveCallRole, CallControlStatus};
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::calls_json::{
    active_call_json, calls_state_json, connection_notice, control_status, dial_failure, history_item_json,
};
use super::contacts_json::{card_json, contact_item_json, contacts_state_json};
use crate::engine_bridge::EngineBridge;
use crate::images;

/// A read for a view.
///
/// A failure other than the engine not running yet is logged, and the view shows nothing for it.
pub(super) fn read<T>(what: &str, result: Result<T, FeatureApiError>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(FeatureApiError::NotStarted) => None,
        Err(e) => {
            info!(what, error = %e, "a value for the calls views was not read");
            None
        }
    }
}

fn contact_photo(engine: &Arc<Engine>, contact_id: i64) -> Option<String> {
    images::cached("contacts", &contact_id.to_string(), || {
        engine.clone().contacts().thumbnail(contact_id).ok().flatten()
    })
}

impl EngineBridge {
    pub(super) fn contacts_json(&self, engine: &Arc<Engine>) -> Value {
        let list = read("contacts", engine.clone().contacts().list()).unwrap_or_default();
        Value::Array(
            list.iter()
                .map(|contact| {
                    let photo = contact.has_thumbnail.then(|| contact_photo(engine, contact.contact_id)).flatten();
                    contact_item_json(contact, photo)
                })
                .collect(),
        )
    }

    pub(super) fn contact_json(&self, engine: &Arc<Engine>, arg: &str) -> Value {
        let id = match arg.parse::<i64>() {
            Ok(id) => id,
            Err(e) => {
                warn!(%arg, error = %e, "a contact card was asked for with an argument that is not a contact id");
                return Value::Null;
            }
        };
        let Some(card) = read("a contact card", engine.clone().contacts().card(id)).flatten() else {
            return Value::Null;
        };

        let photo = card.has_thumbnail.then(|| contact_photo(engine, id)).flatten();
        card_json(&card, photo)
    }

    pub(super) fn contacts_state(&self, engine: &Arc<Engine>) -> Value {
        let Some(state) = read("the contacts state", engine.clone().contacts().state()) else { return Value::Null };
        let calls = &self.areas.calls;
        contacts_state_json(&state, &calls.contacts_wait, &calls.contacts_request, calls.contacts_access_pending)
    }

    pub(super) fn calls_json(&self, engine: &Arc<Engine>) -> Value {
        let history = read("calls", engine.clone().calling().history()).unwrap_or_default();
        Value::Array(
            history
                .iter()
                .map(|call| history_item_json(call, call.photo_contact_id.and_then(|id| contact_photo(engine, id))))
                .collect(),
        )
    }

    pub(super) fn calls_state(&self, engine: &Arc<Engine>) -> Value {
        let Some(state) = read("the call history state", engine.clone().calling().state()) else { return Value::Null };
        let calls = &self.areas.calls;
        calls_state_json(&state, &calls.calls_wait, &calls.history_request, calls.history_access_pending)
    }

    /// Call control's state, the phone's current calls and what shows them.
    pub(super) fn call_control(&self, engine: &Arc<Engine>) -> Value {
        let control = read("the call control state", engine.clone().calling().control_state());
        let calls = &self.areas.calls;
        let status = control.map_or(CallControlStatus::Off, |state| state.status);
        let call =
            |call: &ActiveCall| active_call_json(call, call.photo_contact_id.and_then(|id| contact_photo(engine, id)));
        let with_role = |role| calls.calls.iter().find(|c| c.role == role).map(call);
        // Messages gate a text only once their state is known.
        let can_message = engine
            .clone()
            .messaging()
            .state()
            .map_or(true, |state| state.settings.pc_consent && state.phone_permission);
        json!({
            "offered": control.is_some_and(|state| state.offered),
            "pcConsent": control.is_some_and(|state| state.pc_consent),
            "enabled": control.is_some_and(|state| state.enabled),
            "status": control_status(status),
            "canDial": status == CallControlStatus::Ready && !calls.dialing,
            "connecting": calls.connecting,
            "dialing": calls.dialing,
            "connectionNotice": control.as_ref().and_then(connection_notice),
            "dialNotice": calls.dial_notice.map(dial_failure),
            "active": calls.calls.iter().map(call).collect::<Vec<_>>(),
            "popup": calls.presentation.popup(&calls.calls).map(call).collect::<Vec<_>>(),
            "primary": with_role(ActiveCallRole::Primary),
            "secondary": with_role(ActiveCallRole::Secondary),
            "callWindowOpen": calls.presentation.window_call.is_some(),
            "canTakeAudio": control.is_some_and(|state| state.can_take_audio),
            "hasMicrophone": crate::audio::has_microphone(),
            "hasOutput": crate::audio::has_output(),
            "keypadNumber": calls.keypad_number,
            "canMessage": can_message,
        })
    }
}
