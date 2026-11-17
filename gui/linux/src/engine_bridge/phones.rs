//! The account's linked phones: the list the window picks from, the selected phone's name and
//! connection, and selecting another phone.

use std::pin::Pin;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::EngineBridge;
use crate::{images, user_text};

impl EngineBridge {
    /// Reads the linked phones again; none before the engine starts, or when the read fails.
    pub(super) fn reload_phones(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let phones = match engine.phone().phones() {
            Ok(phones) => phones,
            Err(FeatureApiError::NotStarted) => Vec::new(),
            Err(e) => {
                warn!(error = %e, "the linked phones were not read");
                Vec::new()
            }
        };

        let json: Vec<Value> = phones
            .into_iter()
            .map(|p| json!({ "id": p.id, "name": p.name, "model": p.model, "selected": p.selected }))
            .collect();
        self.as_mut().set_phones_json(QString::from(Value::Array(json).to_string().as_str()));
    }

    /// Reads the selected phone again: its name, whether it is connected, and whether Link to Windows
    /// turned off every feature for this computer.
    ///
    /// The last is kept in the phone's record, so it holds from the start, not only from the event that
    /// reports a change. Without a phone, or when the read fails, the last phone read stays shown.
    pub(super) fn reload_phone(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        match engine.phone().info() {
            Ok(info) => {
                self.as_mut().set_phone_name(QString::from(info.name.as_str()));
                self.as_mut().set_phone_connected(info.connected);
                self.as_mut().set_features_disabled(info.all_features_disabled);
            }
            Err(FeatureApiError::NotStarted | FeatureApiError::NoPhone) => {}
            Err(e) => info!(error = %e, "the phone was not read; the last one read stays shown"),
        }
        self.reload_status();
    }

    pub fn select_phone(mut self: Pin<&mut Self>, id: &QString) {
        let Some(engine) = self.engine.clone() else { return };
        if self.starting {
            return;
        }

        // Another phone's ids would name other pictures.
        images::clear_all();
        self.as_mut().rust_mut().starting = true;
        // The first selection starts the phone session; a switch while running keeps the window as it is.
        if self.phase == QString::from("selectingPhone") {
            self.as_mut().set_phase(QString::from("starting"));
        }

        let id = id.to_string();
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.phone().select(id).await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().starting = false;
                match result {
                    Ok(()) => bridge.running(),
                    // A selection that finds the account removed announces it first; that state says
                    // what to do (sign in again).
                    Err(e) if bridge.phase == QString::from("accountRemoved") => {
                        warn!(error = %e, "the phone was not selected");
                    }
                    Err(e) => {
                        warn!(error = %e, "the phone was not selected");
                        bridge.failed_with(&user_text::feature_error(&e), &e.to_string());
                    }
                }
            });
        });
    }
}
