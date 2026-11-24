//! The notice of a call the user placed or answered on the phone: a desktop notification that offers
//! to transfer the call to this computer, or to stop these notices.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::calling::ActiveCall;
use phonelink_engine::engine::SystemNotifier;
use serde_json::json;
use tracing::{info, warn};

use crate::engine_bridge::EngineBridge;
use crate::l10n::translate;
use crate::notifier::LocalNotification;

/// The notice's action that transfers the call to this computer.
const TRANSFER: &str = "transfer";
/// The notice's action that turns these notices off.
const TURN_OFF: &str = "turnOff";

/// The notification id of the notice of call `id`.
pub(super) fn notice_id(id: u64) -> String {
    format!("calls/onPhone/{id}")
}

/// Who the notice names: the contact, else the number, else "Unknown".
pub(super) fn notice_title(call: &ActiveCall) -> String {
    match (&call.name, call.number.as_str()) {
        (Some(name), _) if !name.is_empty() => name.clone(),
        (_, number) if !number.is_empty() => number.to_owned(),
        _ => translate("Unknown").to_owned(),
    }
}

impl EngineBridge {
    /// Posts the notice of call `id`, taken on the phone.
    pub(super) fn post_phone_call_notice(self: Pin<&mut Self>, id: u64) {
        let Some(notifier) = self.notifier.clone() else { return };
        let Some(title) = self.areas.calls.calls.iter().find(|call| call.id == id).map(notice_title) else { return };

        let thread = self.qt_thread();
        let handler = Arc::new(move |action: &str| {
            let action = action.to_owned();
            let _ = thread.queue(move |bridge| bridge.phone_call_notice_answered(id, &action));
        });
        let notice = LocalNotification {
            id: &notice_id(id),
            title: &title,
            body: translate("You're in a call on your phone."),
            actions: &[(TRANSFER, translate("Transfer to Computer")), (TURN_OFF, translate("Don't Show Again"))],
            critical: false,
        };
        if let Err(e) = notifier.post_local(&notice, handler) {
            warn!(error = %e, "the notice of a call on the phone was not posted");
        }
    }

    /// The notice of call `id` showed for its lifetime: it is taken back.
    pub(super) fn phone_call_notice_over(mut self: Pin<&mut Self>, id: u64) {
        if self.as_mut().rust_mut().areas.calls.presentation.notice_over(id) {
            self.withdraw_phone_call_notices(&[id]);
        }
    }

    /// Takes the notices of `calls` back.
    pub(super) fn withdraw_phone_call_notices(self: Pin<&mut Self>, calls: &[u64]) {
        let Some(notifier) = self.notifier.clone().filter(|_| !calls.is_empty()) else { return };
        if let Err(e) = notifier.withdraw(calls.iter().copied().map(notice_id).collect()) {
            warn!(error = %e, "the notice of a call on the phone was not taken back");
        }
    }

    /// The user answered the notice of call `id`: the call is transferred to this computer, or these
    /// notices are turned off. A click on the notice itself does nothing.
    fn phone_call_notice_answered(mut self: Pin<&mut Self>, id: u64, action: &str) {
        if !matches!(action, TRANSFER | TURN_OFF) {
            return;
        }
        if self.as_mut().rust_mut().areas.calls.presentation.notice_over(id) {
            self.as_mut().withdraw_phone_call_notices(&[id]);
        }

        if action == TURN_OFF {
            info!("the notices of calls on the phone were turned off");
            self.as_mut().rust_mut().areas.calls.phone_call_notices_off = true;
            // The window's preferences keep the switch.
            self.emit_area_event("calls", "phoneCallNoticesOff", &json!({}));
        } else if let Some(engine) = self.engine.clone() {
            self.transfer_to_computer(engine, id);
        }
    }
}
