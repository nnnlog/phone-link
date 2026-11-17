//! The account: its linked phones and PCs, this computer's registration and the account's sign-in
//! name (both read from the keyring), unlinking a PC, signing out or resetting the registration, and
//! the sign-in page's load failures.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::{DeviceRegistration, Engine};
use phonelink_engine::api::phone::{AccountPc, PhoneChoice};
use serde_json::{Value, json};
use tracing::{debug, warn};

use super::pairing::Pairing;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::translate;
use crate::{images, user_text};

/// Logs a sign-in page that did not load (`{code, domain, error}` from the view). A load that another
/// one replaced (a redirect, a new navigation) is no failure.
pub(super) fn sign_in_load_failed(arg: &str) {
    #[derive(serde::Deserialize)]
    struct LoadFailure {
        code: i64,
        domain: i64,
        error: String,
    }
    /// `QtWebEngine`'s `ErrorDomain` of network errors, and Chromium's `ERR_ABORTED`.
    const NETWORK_DOMAIN: i64 = 2;
    const ABORTED: i64 = -3;
    match serde_json::from_str::<LoadFailure>(arg) {
        Ok(f) if f.domain == NETWORK_DOMAIN && f.code == ABORTED => debug!("a sign-in page load was superseded"),
        Ok(f) => warn!(code = f.code, domain_kind = f.domain, error = %f.error, "a sign-in page did not load"),
        Err(e) => warn!(error = %e, "signIn.loadFailed takes {{code, domain, error}}"),
    }
}

/// Unix seconds as milliseconds, `null` when unknown (`0`).
fn millis(seconds: i64) -> Value {
    if seconds > 0 { Value::from(seconds.saturating_mul(1000)) } else { Value::Null }
}

/// A linked phone: its name, its model when that is not already its name, when the account last saw
/// it, and whether this computer uses it.
pub(super) fn phone_json(phone: &PhoneChoice, in_use: Option<&str>) -> Value {
    let in_use = match in_use {
        Some(id) => id.eq_ignore_ascii_case(&phone.id),
        None => phone.selected,
    };
    json!({
        "id": phone.id,
        "name": phone.name,
        "model": phone.model.as_deref().filter(|model| *model != phone.name),
        "lastSeenMs": millis(phone.last_seen),
        "inUse": in_use,
    })
}

/// The account's linked phones, most recently seen first; the one in use is the selected phone, else
/// the one the account's list marks.
pub(super) fn phones(engine: &Arc<Engine>) -> Value {
    let phones = match engine.clone().phone().phones() {
        Ok(phones) => phones,
        Err(FeatureApiError::NotStarted) => Vec::new(),
        Err(e) => {
            warn!(error = %e, "the linked phones were not read");
            Vec::new()
        }
    };
    let in_use = engine.clone().phone().info().ok().map(|info| info.id);

    Value::Array(phones.iter().map(|phone| phone_json(phone, in_use.as_deref())).collect())
}

/// The app a PC was enrolled for, as the account pane names it.
pub(super) fn pc_app(client_type: &str) -> String {
    match client_type {
        "PL" => translate("Phone Link").to_owned(),
        "WEA" => translate("Cross Device").to_owned(),
        "" => translate("Unknown app").to_owned(),
        other => other.to_owned(),
    }
}

pub(super) fn pc_json(pc: &AccountPc, unlinking: &BTreeSet<String>) -> Value {
    json!({
        "id": pc.id,
        "name": pc.name,
        "app": pc_app(&pc.client_type),
        "registeredMs": millis(pc.registered),
        "lastSeenMs": millis(pc.last_seen),
        "thisComputer": pc.this_mac,
        "unlinking": unlinking.contains(&pc.id),
    })
}

/// The account's PCs, this computer's identities marked, most recently seen first.
pub(super) fn pcs(engine: &Arc<Engine>, unlinking: &BTreeSet<String>) -> Value {
    let pcs = match engine.clone().phone().account_pcs() {
        Ok(pcs) => pcs,
        Err(FeatureApiError::NotStarted) => Vec::new(),
        Err(e) => {
            warn!(error = %e, "the account's PCs were not read");
            Vec::new()
        }
    };

    Value::Array(pcs.iter().map(|pc| pc_json(pc, unlinking)).collect())
}

pub(super) fn registration_json(registration: Option<&Option<DeviceRegistration>>, reading: bool) -> Value {
    match registration {
        None => json!({ "loaded": false, "reading": reading }),
        Some(None) => json!({ "loaded": true, "reading": reading, "registered": false }),
        Some(Some(registration)) => json!({
            "loaded": true,
            "reading": reading,
            "registered": true,
            "deviceId": registration.puid,
            "linkedIds": registration.distinguished_device_ids,
        }),
    }
}

impl EngineBridge {
    /// Signs out, or resets this computer's registration (which also signs out): the engine ends the
    /// phone session and deletes this computer's copies of the phone's data. The window then offers to
    /// sign in again.
    pub(super) fn shell_sign_out(mut self: Pin<&mut Self>, engine: Arc<Engine>, reset: bool) {
        let signed_out = self.phase == QString::from("signedOut");
        if self.starting || signed_out || self.areas.shell.signing_out {
            return;
        }

        self.as_mut().rust_mut().areas.shell.signing_out = true;
        self.as_mut().bump(Revision::Settings);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = if reset { engine.reset_device_registration().await } else { engine.sign_out().await };
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.shell.signing_out = false;
                match result {
                    Ok(()) => bridge.as_mut().shell_signed_out(),
                    Err(e) => {
                        warn!(error = %e, is_reset = reset, "signing out did not finish");
                        let title = if reset {
                            "Phone Link Couldn't Reset the Registration"
                        } else {
                            "Phone Link Couldn't Sign Out"
                        };
                        // Settings closed when the user confirmed: the main window shows why, in front.
                        bridge.as_mut().show_section(QString::default());
                        bridge.as_mut().show_failure(Failure::new(title, user_text::engine_error(&e)));
                    }
                }
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// The account was signed out: what the window showed of it goes.
    fn shell_signed_out(mut self: Pin<&mut Self>) {
        images::clear_all();
        {
            let mut rust = self.as_mut().rust_mut();
            let state = &mut rust.areas.shell;
            state.connect_failure = None;
            state.pairing = Pairing::default();
            state.account_name = None;
            state.registration = None;
            state.unlinking.clear();
        }

        self.as_mut().set_phones_json(QString::from("[]"));
        self.as_mut().set_phone_name(QString::default());
        self.as_mut().set_phone_connected(false);
        self.as_mut().set_features_disabled(false);
        self.as_mut().set_status_json(QString::from("{}"));
        self.as_mut().set_phase(QString::from("signedOut"));
        self.as_mut().bump(Revision::Pairing);
        self.show_section(QString::default());
    }

    /// Reads the account's sign-in name off the GUI thread: it reads the keyring.
    pub(super) fn shell_load_account_name(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.shell.reading_account_name {
            return;
        }
        self.as_mut().rust_mut().areas.shell.reading_account_name = true;

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let name = engine.pairing().account_name();
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.shell.account_name = Some(name);
                bridge.as_mut().rust_mut().areas.shell.reading_account_name = false;
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Reads this computer's registration off the GUI thread: it reads the keyring.
    pub(super) fn shell_load_registration(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.shell.reading_registration {
            return;
        }
        self.as_mut().rust_mut().areas.shell.reading_registration = true;

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let registration = engine.device_registration();
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.shell.registration = Some(registration);
                bridge.as_mut().rust_mut().areas.shell.reading_registration = false;
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Unlinks the PC `id` from the account; the PCs are read again after it.
    pub(super) fn shell_unlink_pc(mut self: Pin<&mut Self>, engine: Arc<Engine>, id: &str) {
        if !self.as_mut().rust_mut().areas.shell.unlinking.insert(id.to_owned()) {
            return;
        }
        self.as_mut().bump(Revision::Settings);

        let thread = self.qt_thread();
        let id = id.to_owned();
        self.runtime.spawn(async move {
            let result = engine.phone().unlink_pc(id.clone()).await;
            let _ = thread.queue(move |mut bridge| {
                bridge.as_mut().rust_mut().areas.shell.unlinking.remove(&id);
                if let Err(e) = result {
                    warn!(error = %e, "the PC was not unlinked");
                    bridge.as_mut().show_failure(Failure::feature("Phone Link Couldn't Unlink the PC", &e));
                }
                bridge.bump(Revision::Settings);
            });
        });
    }
}
