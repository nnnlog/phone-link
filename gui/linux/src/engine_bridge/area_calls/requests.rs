//! What the calls part asks the phone for.
//!
//! The phone's calls while none are stored, and the call history and contacts permissions that the
//! sections' prompts and Settings request.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::calling::CallsAccess;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::{FeatureApiError, RequestOutcome};
use tracing::info;

use super::state::PhoneRequest;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::user_text;

impl EngineBridge {
    /// The phone is asked for its calls once while none are stored; the engine keeps the "once".
    pub(super) fn refresh_calls_if_empty(self: Pin<&mut Self>, engine: Arc<Engine>) {
        self.runtime.spawn(async move {
            match engine.calling().refresh_if_empty().await {
                Ok(Some(outcome)) => {
                    if user_text::outcome_failure(&outcome).is_some() {
                        info!(reason = ?outcome, "the call history was not refreshed");
                    }
                }
                Ok(None) | Err(FeatureApiError::NotStarted) => {}
                Err(e) => info!(error = %e, "the call history was not refreshed"),
            }
        });
    }

    /// Asks the phone for its calls when the history is allowed and none are stored.
    pub(super) fn refresh_calls_if_empty_now(self: Pin<&mut Self>, engine: Arc<Engine>) {
        let calling = engine.clone().calling();
        let granted = calling.state().is_ok_and(|state| state.access == CallsAccess::Granted);
        if granted && calling.history().is_ok_and(|history| history.is_empty()) {
            self.refresh_calls_if_empty(engine);
        }
    }

    /// The Calls section's *Send Request to Phone*.
    ///
    /// A phone that does not answer is said in the section, any other failure in the window.
    pub(super) fn request_history_permission(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.calls.history_request.pending {
            return;
        }
        self.as_mut().rust_mut().areas.calls.history_request = PhoneRequest { pending: true, timed_out: false };
        self.as_mut().bump(Revision::Calls);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.calling().request_permission().await;
            let _ = thread.queue(move |mut bridge| {
                let mut timed_out = false;
                match result {
                    Ok(result) if result.timed_out => {
                        info!("the call history permission request timed out");
                        timed_out = true;
                    }
                    Ok(result) => {
                        if let Some(text) = user_text::outcome_failure(&result.outcome) {
                            bridge.as_mut().show_failure(Failure::new("Call History Not Allowed", text));
                        } else if let Some(refresh) = result.refresh.filter(|r| user_text::outcome_failure(r).is_some())
                        {
                            info!(reason = ?refresh, "the call history was not refreshed after the grant");
                        }
                    }
                    Err(e) => bridge.as_mut().show_failure(Failure::feature("Call History Not Allowed", &e)),
                }

                bridge.as_mut().rust_mut().areas.calls.history_request = PhoneRequest { pending: false, timed_out };
                bridge.as_mut().bump(Revision::Calls);
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Settings' *Send Request to Phone* for call history.
    pub(super) fn request_history_access(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.calls.history_access_pending {
            return;
        }
        self.as_mut().rust_mut().areas.calls.history_access_pending = true;
        self.as_mut().bump(Revision::Calls);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.calling().request_permission().await;
            let _ = thread.queue(move |mut bridge| {
                let failure = match result {
                    Ok(result) => {
                        if let Some(refresh) = result.refresh.filter(|r| user_text::outcome_failure(r).is_some()) {
                            info!(reason = ?refresh, "the call history was not refreshed after the grant");
                        }
                        let outcome = if result.timed_out { RequestOutcome::TimedOut } else { result.outcome };
                        user_text::outcome_failure(&outcome).map(|text| Failure::new("Couldn't Request Access", text))
                    }
                    Err(e) => Some(Failure::feature("Couldn't Request Access", &e)),
                };
                if let Some(failure) = failure {
                    bridge.as_mut().show_failure(failure);
                }

                bridge.as_mut().rust_mut().areas.calls.history_access_pending = false;
                bridge.as_mut().bump(Revision::Calls);
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// The Contacts section's *Send Request to Phone*.
    ///
    /// A phone that does not answer is said in the section, any other failure in the window.
    pub(super) fn calls_request_contacts_permission(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.calls.contacts_request.pending {
            return;
        }
        self.as_mut().rust_mut().areas.calls.contacts_request = PhoneRequest { pending: true, timed_out: false };
        self.as_mut().bump(Revision::Contacts);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.contacts().request_permission().await;
            let _ = thread.queue(move |mut bridge| {
                let mut timed_out = false;
                match result {
                    Ok(result) if result.timed_out => {
                        info!("the contacts permission request timed out");
                        timed_out = true;
                    }
                    Ok(result) => {
                        if let Some(text) = user_text::outcome_failure(&result.outcome) {
                            bridge.as_mut().show_failure(Failure::new("Contacts Not Allowed", text));
                        } else if let Some(sync) = result.sync.filter(|s| user_text::outcome_failure(s).is_some()) {
                            info!(reason = ?sync, "the contacts were not synced after the grant");
                        }
                    }
                    Err(e) => bridge.as_mut().show_failure(Failure::feature("Contacts Not Allowed", &e)),
                }

                bridge.as_mut().rust_mut().areas.calls.contacts_request = PhoneRequest { pending: false, timed_out };
                bridge.as_mut().bump(Revision::Contacts);
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Settings' *Send Request to Phone* for contacts.
    pub(super) fn calls_request_contacts_access(mut self: Pin<&mut Self>, engine: Arc<Engine>) {
        if self.areas.calls.contacts_access_pending {
            return;
        }
        self.as_mut().rust_mut().areas.calls.contacts_access_pending = true;
        self.as_mut().bump(Revision::Contacts);

        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.contacts().request_permission().await;
            let _ = thread.queue(move |mut bridge| {
                let failure = match result {
                    Ok(result) => {
                        if let Some(sync) = result.sync.filter(|s| user_text::outcome_failure(s).is_some()) {
                            info!(reason = ?sync, "the contacts were not synced after the grant");
                        }
                        let outcome = if result.timed_out { RequestOutcome::TimedOut } else { result.outcome };
                        user_text::outcome_failure(&outcome).map(|text| Failure::new("Couldn't Request Access", text))
                    }
                    Err(e) => Some(Failure::feature("Couldn't Request Access", &e)),
                };
                if let Some(failure) = failure {
                    bridge.as_mut().show_failure(failure);
                }

                bridge.as_mut().rust_mut().areas.calls.contacts_access_pending = false;
                bridge.as_mut().bump(Revision::Contacts);
                bridge.bump(Revision::Settings);
            });
        });
    }
}
