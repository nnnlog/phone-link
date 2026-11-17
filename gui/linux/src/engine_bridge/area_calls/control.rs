//! Call control over Bluetooth.
//!
//! The connection it needs and its pairing question; dialing, answering, declining, ending, swapping and
//! declining with a text; and where its failures show.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::calling::{CallControlError, CallControlStatus, CallingApi};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::messaging::MessageTarget;
use serde_json::json;
use tracing::{info, warn};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::translate;
use crate::system::Desktop;
use crate::user_text;

/// The program and its arguments that open this desktop's Bluetooth settings.
pub(super) fn bluetooth_settings(desktop: Desktop) -> (&'static str, &'static [&'static str]) {
    match desktop {
        Desktop::Kde => ("systemsettings", &["kcm_bluetooth"]),
        Desktop::Gnome => ("gnome-control-center", &["bluetooth"]),
        Desktop::Xfce | Desktop::Mate | Desktop::Cinnamon | Desktop::Other => ("blueman-manager", &[]),
    }
}

impl EngineBridge {
    /// A Bluetooth pairing's question: `Some(code)` asks the user to confirm the code the phone shows.
    ///
    /// The user checks that the phone shows `code` and answers with `calls.bluetoothPairingAnswer`;
    /// `None` takes the question back, the pairing having ended without the answer.
    pub(in crate::engine_bridge) fn bluetooth_pairing_question(self: Pin<&mut Self>, code: Option<String>) {
        info!(is_open = code.is_some(), "the Bluetooth pairing question changed");
        self.emit_area_event("calls", "bluetoothPairing", &json!({ "code": code }));
    }

    /// Shows a call-control failure: in the call window while it is open, else in the main window.
    pub(super) fn show_call_failure(self: Pin<&mut Self>, failure: Failure) {
        if self.areas.calls.presentation.window_shows_failures() {
            warn!(failure_kind = failure.title, text = %failure.message, "a failure is shown in the call window");
            let payload = json!({ "title": translate(failure.title), "message": failure.message });
            self.emit_area_event("calls", "failure", &payload);
        } else {
            self.show_failure(failure);
        }
    }

    /// *Connect*, *Try Again* or *Begin Pairing*: the connection over Bluetooth that call control needs.
    pub(super) fn connect_calls(mut self: Pin<&mut Self>, engine: Arc<Engine>, pairing: bool) {
        if self.areas.calls.connecting {
            return;
        }
        self.as_mut().rust_mut().areas.calls.connecting = true;
        self.as_mut().bump(Revision::Calls);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let calling = engine.calling();
            let result = if pairing { calling.pair_for_control().await } else { calling.connect_control().await };
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.calls.connecting = false;
                match result {
                    Ok(status) => info!(?status, "call control connect finished"),
                    Err(e) => bridge
                        .as_mut()
                        .show_call_failure(Failure::new("Couldn't Connect to Your Phone", user_text::call_error(&e))),
                }
                bridge.as_mut().bump(Revision::Calls);
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Places a call to `number` on the phone.
    ///
    /// Call control must be ready (the views offer *Call* only then); a dial the phone refused shows as
    /// a notice in the Calls section, any other failure in the window. QML hears `dialed` with the
    /// number and whether the call was placed.
    pub(super) fn dial(mut self: Pin<&mut Self>, engine: Arc<Engine>, number: String) {
        if self.areas.calls.dialing {
            info!("a call was not placed: another dial runs");
            return;
        }
        let ready =
            engine.clone().calling().control_state().is_ok_and(|state| state.status == CallControlStatus::Ready);
        if !ready {
            let text = user_text::call_error(&CallControlError::NotReady);
            self.show_call_failure(Failure::new("Call Not Placed", text));
            return;
        }

        {
            let mut rust = self.as_mut().rust_mut();
            rust.areas.calls.dialing = true;
            rust.areas.calls.dial_notice = None;
        }
        self.as_mut().bump(Revision::Calls);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.calling().dial(number.clone()).await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.calls.dialing = false;
                let placed = result.is_ok();
                match result {
                    Ok(()) => {}
                    Err(CallControlError::DialFailed { kind }) => {
                        info!(?kind, "the phone did not place the call");
                        bridge.as_mut().rust_mut().areas.calls.dial_notice = Some(kind);
                    }
                    Err(e) => {
                        bridge.as_mut().show_call_failure(Failure::new("Call Not Placed", user_text::call_error(&e)))
                    }
                }
                bridge.as_mut().emit_area_event("calls", "dialed", &json!({ "number": number, "placed": placed }));
                bridge.as_mut().reload_calls();
                bridge.bump(Revision::Calls);
            });
        });
    }

    /// Runs a call-control operation; its failure shows under `title`.
    ///
    /// The current calls are read again after it.
    pub(super) fn control_operation<F, Fut>(
        self: Pin<&mut Self>,
        engine: Arc<Engine>,
        title: &'static str,
        operation: F,
    ) where
        F: FnOnce(Arc<CallingApi>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), CallControlError>> + Send + 'static,
    {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = operation(engine.calling()).await;
            let _ = thread.queue(move |mut bridge| {
                if let Err(e) = result {
                    bridge.as_mut().show_call_failure(Failure::new(title, user_text::call_error(&e)));
                }
                bridge.as_mut().reload_calls();
                bridge.bump(Revision::Calls);
            });
        });
    }

    /// Answers, declines or ends call `id`.
    pub(super) fn answer_call(self: Pin<&mut Self>, engine: Arc<Engine>, action: &str, id: u64) {
        match action {
            "calls.answer" => self.control_operation(engine, "Couldn't Answer Call", move |calling| async move {
                calling.answer(id).await
            }),
            "calls.decline" => self.control_operation(engine, "Couldn't Decline Call", move |calling| async move {
                calling.decline(id).await
            }),
            _ => {
                self.control_operation(
                    engine,
                    "Couldn't End Call",
                    move |calling| async move { calling.end_call(id).await },
                )
            }
        }
    }

    /// Declines ringing call `id` and texts `text` to its `number`.
    pub(super) fn reply_to_call(self: Pin<&mut Self>, engine: Arc<Engine>, id: u64, number: String, text: String) {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let declined = engine.clone().calling().decline(id).await;
            let target = MessageTarget::NewConversation { recipients: vec![number] };
            let sent = engine.messaging().send(target, text, Vec::new()).await;
            let _ = thread.queue(move |mut bridge| {
                if let Err(e) = declined {
                    bridge.as_mut().show_call_failure(Failure::new("Couldn't Decline Call", user_text::call_error(&e)));
                }
                if let Err(e) = sent {
                    bridge.as_mut().show_call_failure(Failure::feature("Message Not Sent", &e));
                }
                bridge.as_mut().reload_calls();
                bridge.bump(Revision::Calls);
            });
        });
    }

    /// Opens this desktop's Bluetooth settings, off the GUI thread.
    pub(super) fn open_bluetooth_settings(self: Pin<&mut Self>) {
        let (program, args) = bluetooth_settings(Desktop::detect());
        let thread = self.qt_thread();
        let spawned = std::thread::Builder::new().name("bluetooth-settings".to_owned()).spawn(move || {
            match std::process::Command::new(program).args(args).status() {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    info!(command = program, %status, "the Bluetooth settings program ended with a failure status")
                }
                Err(e) => {
                    warn!(command = program, error = %e, "the Bluetooth settings program did not start");
                    let text = translate("Turn on Bluetooth on this computer and try again.").to_owned();
                    let failure = Failure::new("Couldn't Open Bluetooth Settings", text);
                    let _ = thread.queue(move |bridge| bridge.show_failure(failure));
                }
            }
        });
        if let Err(e) = spawned {
            warn!(error = %e, "the Bluetooth settings were not opened");
        }
    }
}
