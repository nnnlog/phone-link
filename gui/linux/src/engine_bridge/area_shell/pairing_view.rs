//! The pairing view's actions and the engine's pairing steps, applied to the view's state and carried
//! out: the limits it starts, the sessions it cancels, and the phone it selects once paired.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::pairing::PairingFlow;
use serde_json::json;
use tracing::warn;

use super::pairing::Effects;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::Revision;
use crate::user_text;

impl EngineBridge {
    pub(super) fn shell_perform_pairing(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) {
        let api = engine.clone().pairing();
        match name {
            "pairing.start" => match arg {
                "qrCode" => self.as_mut().shell_start_pairing(&engine, PairingFlow::QrCode),
                "manualPin" => self.as_mut().shell_start_pairing(&engine, PairingFlow::ManualPin),
                _ => warn!(arg, "pairing.start takes qrCode or manualPin"),
            },
            "pairing.retry" => {
                let flow = self.areas.shell.pairing.flow();
                self.as_mut().shell_start_pairing(&engine, flow);
            }
            "pairing.showManual" => {
                let account_name = api.account_name();
                let effects = self.as_mut().rust_mut().areas.shell.pairing.show_manual(account_name);
                self.as_mut().shell_pairing_effects(effects);
            }
            "pairing.backToQr" => match self.as_mut().rust_mut().areas.shell.pairing.back_to_qr() {
                Some(effects) => self.as_mut().shell_pairing_effects(effects),
                None => self.as_mut().shell_start_pairing(&engine, PairingFlow::QrCode),
            },
            "pairing.continueManually" => self.as_mut().shell_start_pairing(&engine, PairingFlow::ManualPin),
            "pairing.refreshPin" => {
                if self.areas.shell.pairing.can_refresh_pin() && api.refresh_challenge_pin() {
                    self.as_mut().rust_mut().areas.shell.pairing.refreshing();
                }
            }
            "pairing.cancel" => {
                api.cancel();
                self.as_mut().rust_mut().areas.shell.pairing.cancel();
            }
            other => warn!(action = other, "an unknown pairing action"),
        }
        self.bump(Revision::Pairing);
    }

    /// Starts a pairing session of `flow`, cancelling the one in progress.
    fn shell_start_pairing(mut self: Pin<&mut Self>, engine: &Arc<Engine>, flow: PairingFlow) {
        let started = engine.clone().pairing().start(flow).map_err(|e| {
            warn!(error = %e, "pairing did not start");
            user_text::feature_error(&e)
        });
        let effects = self.as_mut().rust_mut().areas.shell.pairing.started(flow, started);
        self.shell_pairing_effects(effects);
    }

    /// Runs what a change of the pairing calls for: a limit, cancelling the session, selecting the
    /// paired phone.
    fn shell_pairing_effects(mut self: Pin<&mut Self>, effects: Effects) {
        if let Some(timer) = effects.timer {
            let thread = self.qt_thread();
            self.runtime.spawn(async move {
                tokio::time::sleep(timer.after).await;
                let _ = thread.queue(move |bridge| bridge.shell_pairing_limit(timer.generation));
            });
        }
        if effects.cancel_session
            && let Some(engine) = self.engine.clone()
        {
            engine.pairing().cancel();
        }
        if let Some(phone) = effects.paired {
            self.as_mut().emit_area_event("shell", "paired", &json!({}));
            self.select_phone(&QString::from(phone.as_str()));
        }
    }

    fn shell_pairing_limit(mut self: Pin<&mut Self>, generation: u64) {
        let effects = self.as_mut().rust_mut().areas.shell.pairing.limit_reached(generation);
        self.as_mut().shell_pairing_effects(effects);
        self.bump(Revision::Pairing);
    }

    /// A step of the pairing session: the engine keeps the last one.
    pub(in crate::engine_bridge) fn pairing_changed(mut self: Pin<&mut Self>) {
        let update = self.engine.clone().and_then(|engine| engine.pairing().current());
        if let Some(update) = update {
            let effects = self.as_mut().rust_mut().areas.shell.pairing.apply(update);
            self.as_mut().shell_pairing_effects(effects);
        }
        self.bump(Revision::Pairing);
    }
}
