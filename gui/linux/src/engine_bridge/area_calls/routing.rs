//! The calls part's `query` and `perform` names, routed to what answers or runs them.
//!
//! The actions' arguments are read here too.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use phonelink_engine::api::calling::CallsAccess;
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};

/// The user's answer to a Bluetooth pairing's question: `"true"` pairs, `"false"` refuses.
pub(super) fn pairing_answer(arg: &str) -> Option<bool> {
    match arg {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// The call window's keypad key: `{"id": <call>, "key": "<0-9, * or #>"}`.
pub(super) fn key_of(arg: &str) -> Option<(u64, String)> {
    let value: Value = serde_json::from_str(arg).ok()?;
    Some((value.get("id")?.as_u64()?, value.get("key")?.as_str()?.to_owned()))
}

/// The incoming-call window's *Message* reply: `{"id": <call>, "number": "…", "text": "…"}`.
pub(super) fn reply_of(arg: &str) -> Option<(u64, String, String)> {
    let value: Value = serde_json::from_str(arg).ok()?;
    let number = value.get("number")?.as_str()?;
    let text = value.get("text")?.as_str()?;
    Some((value.get("id")?.as_u64()?, number.to_owned(), text.to_owned()))
}

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_calls(&self, engine: &Arc<Engine>, name: &str, arg: &str) -> Option<Value> {
        let value = match name {
            "contacts" => self.contacts_json(engine),
            "contact" => self.contact_json(engine, arg),
            "contactsState" => self.contacts_state(engine),
            "calls" => self.calls_json(engine),
            "callsState" => self.calls_state(engine),
            "callControl" => self.call_control(engine),
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn perform_calls(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        if !name.starts_with("calls.") && !name.starts_with("contacts.") {
            return false;
        }
        let flag = arg == "true";

        let handled = self.as_mut().perform_calls_history(engine.clone(), name, flag)
            || self.as_mut().perform_calls_control(engine.clone(), name, arg, flag)
            || self.as_mut().perform_calls_contacts(engine, name, arg, flag);
        if !handled {
            warn!(action = name, "unknown action");
        }
        true
    }

    /// The call history's actions; whether `name` is one.
    fn perform_calls_history(self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, flag: bool) -> bool {
        match name {
            "calls.markSeen" => {
                let calling = engine.calling();
                if calling.state().is_ok_and(|state| state.access == CallsAccess::Granted)
                    && let Err(e) = calling.mark_seen()
                {
                    info!(error = %e, "the missed calls were not marked seen");
                }
            }
            "calls.refreshIfEmpty" => self.refresh_calls_if_empty(engine),
            "calls.turnOn" => {
                let done = engine.calling().set_pc_consent(true);
                self.calls_setting_changed(
                    done.map_err(|e| Failure::feature("Call History Not Turned On", &e)),
                    Revision::Calls,
                );
            }
            "calls.setHistory" => {
                let done = engine.calling().set_pc_consent(flag);
                self.calls_setting_changed(done.map_err(|e| Failure::setting(&e)), Revision::Calls);
            }
            "calls.requestPermission" => self.request_history_permission(engine),
            "calls.requestAccess" => self.request_history_access(engine),
            _ => return false,
        }
        true
    }

    /// Call control's actions and those of the windows that show the calls; whether `name` is one.
    fn perform_calls_control(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str, flag: bool) -> bool {
        match name {
            "calls.enable" => {
                if let Err(e) = engine.calling().set_control_enabled(flag) {
                    self.as_mut().show_call_failure(Failure::feature("Couldn't Turn On Calls", &e));
                }
                self.as_mut().bump(Revision::Calls);
                self.bump(Revision::Settings);
            }
            "calls.connect" | "calls.pair" => self.connect_calls(engine, name == "calls.pair"),
            "calls.dial" => self.dial(engine, arg.to_owned()),
            "calls.dismissNotice" => {
                self.as_mut().rust_mut().areas.calls.dial_notice = None;
                self.bump(Revision::Calls);
            }
            "calls.openBluetoothSettings" => self.open_bluetooth_settings(),
            "calls.answer" | "calls.decline" | "calls.end" => match arg.parse::<u64>() {
                Ok(id) => self.answer_call(engine, name, id),
                Err(e) => warn!(action = name, %arg, error = %e, "the action takes a call id"),
            },
            "calls.swap" => {
                self.control_operation(engine, "Couldn't Swap Calls", |calling| async move { calling.swap().await })
            }
            "calls.toggleMute" | "calls.toggleHold" | "calls.transfer" => match arg.parse::<u64>() {
                Ok(id) => self.in_call_operation(engine, name, id),
                Err(e) => warn!(action = name, %arg, error = %e, "the action takes a call id"),
            },
            "calls.key" => match key_of(arg) {
                Some((id, key)) => self.control_operation(engine, "Couldn't Send Key", move |calling| async move {
                    calling.send_call_key(id, key).await
                }),
                None => warn!(%arg, "calls.key takes {{id, key}}"),
            },
            "calls.endAll" => self
                .control_operation(engine, "Couldn't End Call", |calling| async move { calling.end_all_calls().await }),
            "calls.keypadNumberTaken" => {
                self.as_mut().rust_mut().areas.calls.keypad_number = None;
                self.bump(Revision::Calls);
            }
            "calls.phoneCallNotices" => {
                self.as_mut().rust_mut().areas.calls.phone_call_notices_off = !flag;
            }
            "calls.hidePopup" => match arg.parse::<u64>() {
                Ok(id) => {
                    self.as_mut().rust_mut().areas.calls.presentation.hidden.insert(id);
                    self.as_mut().present_calls();
                    self.bump(Revision::Calls);
                }
                Err(e) => warn!(%arg, error = %e, "calls.hidePopup takes a call id"),
            },
            "calls.reply" => match reply_of(arg) {
                Some((id, number, text)) => self.reply_to_call(engine, id, number, text),
                None => warn!(%arg, "calls.reply takes {{id, number, text}}"),
            },
            "calls.bluetoothPairingAnswer" => match pairing_answer(arg) {
                Some(confirmed) => crate::bluetooth::answer_pairing_question(confirmed),
                None => warn!(%arg, "calls.bluetoothPairingAnswer takes true or false"),
            },
            _ => return false,
        }
        true
    }

    /// The contacts' actions; whether `name` is one.
    fn perform_calls_contacts(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
        flag: bool,
    ) -> bool {
        match name {
            "contacts.turnOn" => {
                let done = engine.contacts().set_pc_consent(true);
                self.calls_setting_changed(
                    done.map_err(|e| Failure::feature("Contacts Not Turned On", &e)),
                    Revision::Contacts,
                );
            }
            "contacts.setConsent" => {
                let done = engine.contacts().set_pc_consent(flag);
                self.calls_setting_changed(done.map_err(|e| Failure::setting(&e)), Revision::Contacts);
            }
            "contacts.requestPermission" => self.calls_request_contacts_permission(engine),
            "contacts.requestAccess" => self.calls_request_contacts_access(engine),
            "contacts.show" => match arg.parse::<i64>() {
                Ok(id) => {
                    self.as_mut().emit_area_event("contacts", "show", &json!({ "id": id }));
                    self.show_section(QString::from("contacts"));
                }
                Err(e) => warn!(%arg, error = %e, "contacts.show takes a contact id"),
            },
            _ => return false,
        }
        true
    }

    /// A setting switched from a section or Settings: its failure, then the views read it again.
    fn calls_setting_changed(mut self: Pin<&mut Self>, done: Result<(), Failure>, revision: Revision) {
        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        self.as_mut().bump(revision);
        self.bump(Revision::Settings);
    }
}
