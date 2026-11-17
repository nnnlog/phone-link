//! The shell part's `query` and `perform` names, those that need no engine and those that do, each
//! routed to the code that answers it.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::logging::{clear_log, log_size};
use phonelink_engine::edge::WebSignInKind;
use serde_json::{Value, json};
use tracing::{info, warn};

use super::account::{pcs, phones, registration_json, sign_in_load_failed};
use super::app::installed_icon;
use super::settings::{sections, settings, wallpaper};
use super::status::prompt_of;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::{images, user_text};

impl EngineBridge {
    /// The answers that need no engine.
    pub(in crate::engine_bridge) fn query_local(&self, name: &str, arg: &str) -> Option<Value> {
        let state = &self.areas.shell;
        let value = match name {
            "autostart" => Value::Bool(crate::autostart::enabled()),
            "qr" => images::qr_code(arg).map_or(Value::Null, Value::from),
            "logs" => json!({ "installed": crate::app_log::installed(), "size": log_size() }),
            "pairing" => state.pairing.json(),
            "shell.connection" => json!({ "failure": state.connect_failure }),
            "shell.signIn" => {
                let kind = self.sign_in.as_ref().map(|page| match page.kind {
                    WebSignInKind::Login => "login",
                    WebSignInKind::Consent => "consent",
                });
                json!({ "kind": kind })
            }
            "shell.account" => {
                let signed_out = self.phase == QString::from("signedOut");
                json!({
                    "name": state.account_name.clone().flatten(),
                    "signedIn": !signed_out,
                    "canSignOut": !self.starting && !signed_out && !state.signing_out,
                    "signingOut": state.signing_out,
                })
            }
            "shell.about" => json!({
                "version": env!("CARGO_PKG_VERSION"),
                "repository": env!("CARGO_PKG_REPOSITORY"),
                "icon": installed_icon(),
            }),
            "account.registration" => registration_json(state.registration.as_ref(), state.reading_registration),
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn query_shell(&self, engine: &Arc<Engine>, name: &str, _arg: &str) -> Option<Value> {
        let value = match name {
            "settings" => settings(engine),
            "wallpaper" => wallpaper(engine),
            "shell.sections" => sections(engine),
            "shell.phones" => phones(engine),
            "account.pcs" => pcs(engine, &self.areas.shell.unlinking),
            _ => return None,
        };
        Some(value)
    }

    /// The actions that need no engine.
    pub(in crate::engine_bridge) fn perform_local(mut self: Pin<&mut Self>, name: &str, arg: &str) -> bool {
        match name {
            "autostart.set" => {
                if let Err(e) = crate::autostart::set(arg == "true") {
                    warn!(error = %e, "the autostart entry was not written");
                    let message = user_text::something_went_wrong();
                    self.as_mut().show_failure(Failure::new("Couldn't Change the Setting", message));
                }
                self.bump(Revision::Settings);
            }
            "app.refresh" => self.shell_refresh(),
            // A window is about to be activated: a notification click's token lets Wayland raise it.
            "app.activationToken" => {
                if let Some(token) = crate::notifier::take_activation_token() {
                    crate::engine_bridge::qobject::set_activation_token(&token);
                }
            }
            "signIn.loadFailed" => sign_in_load_failed(arg),
            "app.quit" => self.shell_quit(),
            "shell.showPairing" => self.emit_area_event("shell", "showPairing", &json!({})),
            "logs.save" => self.shell_save_log(arg),
            "logs.clear" => {
                if let Err(e) = clear_log() {
                    warn!(error = %e, "the log was not cleared");
                    self.as_mut().show_failure(Failure::feature("Phone Link Couldn't Clear the Log", &e));
                }
                self.bump(Revision::Settings);
            }
            _ => return false,
        }
        true
    }

    pub(in crate::engine_bridge) fn perform_shell(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        if name.starts_with("pairing.") {
            self.shell_perform_pairing(engine, name, arg);
            return true;
        }

        let flag = arg == "true";
        let setting = |e: FeatureApiError| Failure::setting(&e);
        let done: Result<(), Failure> = match name {
            "app.foreground" => {
                engine.set_foreground(flag);
                Ok(())
            }
            "app.windowFocused" => {
                self.as_mut().rust_mut().window_focused = flag;
                if let Err(e) = engine.messaging().window_focused(flag) {
                    info!(error = %e, "the window focus was not passed on");
                }
                self.as_mut().update_call_alerts();
                Ok(())
            }
            "status.acceptPrompt" => match prompt_of(arg) {
                Some(prompt) => engine
                    .device_status()
                    .accept_prompt(prompt)
                    .map_err(|e| Failure::feature("Couldn't Send Request to Phone", &e)),
                None => {
                    warn!(kind = arg, "an unknown status prompt was accepted");
                    Ok(())
                }
            },
            "settings.clipboard" => engine.clipboard().set_phone_toggle(flag).map_err(setting),
            "settings.remoteControl" => engine.remote_control().set_phone_toggle(flag).map_err(setting),
            "settings.fileSharing" => engine.file_transfer().set_share_enabled(flag).map_err(setting),
            "hotspot.connect" | "hotspot.disconnect" => {
                self.as_mut().shell_hotspot(engine, name == "hotspot.connect");
                Ok(())
            }
            "signOut" => {
                self.as_mut().shell_sign_out(engine, false);
                Ok(())
            }
            "account.resetRegistration" => {
                self.as_mut().shell_sign_out(engine, true);
                Ok(())
            }
            "account.loadRegistration" => {
                self.as_mut().shell_load_registration(engine);
                Ok(())
            }
            "account.loadName" => {
                self.as_mut().shell_load_account_name(engine);
                Ok(())
            }
            "account.unlinkPc" => {
                self.as_mut().shell_unlink_pc(engine, arg);
                Ok(())
            }
            "shell.reloadPhones" => {
                self.as_mut().reload_phones();
                Ok(())
            }
            _ => return false,
        };

        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        if name.starts_with("settings.") {
            self.bump(Revision::Settings);
        }
        true
    }
}
