//! Ring my phone: starting, counting down and stopping a ring.

use super::{ControlOutcome, Inner, Prompt, RingRequest, RingShown, Ringing, ring};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use std::time::Duration;
use tracing::{info, warn};

impl Inner {
    pub(super) fn ring_my_phone(&self, phone: &DcgClient) -> ControlOutcome {
        let Some(control) = self.current_ring_control(phone) else { return ControlOutcome::NotAvailable };
        if !control.enabled || control.shown != RingShown::Off {
            return ControlOutcome::NotAvailable;
        }

        self.start_ringing(phone, false);
        self.spawn_ring_request(phone, RingRequest::On);
        self.changed(phone);
        ControlOutcome::RingSent(RingRequest::On)
    }

    pub(super) fn stop_ringing(&self, phone: &DcgClient) -> ControlOutcome {
        let Some(control) = self.current_ring_control(phone) else { return ControlOutcome::NotAvailable };
        let RingShown::Ringing { .. } = control.shown else { return ControlOutcome::NotAvailable };
        if !control.enabled || !control.can_stop {
            return ControlOutcome::NotAvailable;
        }

        {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            let Some(ringing) = ps.ring.as_mut() else { return ControlOutcome::NotAvailable };
            if !ringing.confirm_stop {
                ringing.confirm_stop = true;
                drop(state);
                self.changed(phone);
                return ControlOutcome::Prompt(Prompt::StopRinging);
            }
            ps.ring = None;
        }

        self.spawn_ring_request(phone, RingRequest::Off);
        self.changed(phone);
        ControlOutcome::RingSent(RingRequest::Off)
    }

    /// Shows `phone` ringing with a new countdown; the countdown task ends it.
    pub(super) fn start_ringing(&self, phone: &DcgClient, confirm_stop: bool) {
        let countdown = self.config.ring_my_phone_countdown;
        let ends_at = tokio::time::Instant::now() + countdown;
        let id = {
            let mut state = self.state.lock_or_panic();
            let id = state.next_id();
            state.phone(phone).ring = Some(Ringing { id, ends_at, confirm_stop });
            id
        };

        let (weak, shutdown, phone) = (self.weak.clone(), self.shutdown.clone(), phone.clone());
        self.runtime.spawn(async move {
            let second = Duration::from_secs(1);
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + second, second);

            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = ticks.tick() => {}
                }
                let Some(inner) = weak.upgrade() else { break };
                let ended = {
                    let mut state = inner.state.lock_or_panic();
                    let ps = state.phone(&phone);
                    match ps.ring {
                        Some(r) if r.id == id => {
                            let ended = tokio::time::Instant::now() >= r.ends_at;
                            if ended {
                                ps.ring = None;
                            }
                            ended
                        }
                        // Stopped, ended or replaced meanwhile.
                        _ => break,
                    }
                };

                inner.changed(&phone);
                if ended {
                    info!(%phone, "ring my phone countdown ended");
                    break;
                }
            }
        });
    }

    /// Sends `request` without waiting. A stop whose outcome is not a success brings the ringing
    /// state back — confirmed, with a new countdown — when the control still shows off.
    pub(super) fn spawn_ring_request(&self, phone: &DcgClient, request: RingRequest) {
        let (weak, app, timeout) = (self.weak.clone(), self.env.app.clone(), self.config.ring_my_phone_request_timeout);
        let phone = phone.clone();
        self.runtime.spawn(async move {
            let response = match app.request_drm(ring::drm_request(request, timeout)).await {
                Ok(response) => response,
                Err(e) => {
                    warn!(request_kind = ?request, error = %e, "ring my phone request not sent");
                    return;
                }
            };

            if request != RingRequest::Off || response.outcome.is_success() {
                return ring::log_outcome(request, &response);
            }

            let Some(inner) = weak.upgrade() else { return ring::log_outcome(request, &response) };
            let restorable = inner.current_ring_control(&phone).is_some_and(|c| c.enabled && c.shown == RingShown::Off);
            ring::log_stop_not_delivered(&response, restorable);
            if restorable {
                inner.start_ringing(&phone, true);
                inner.changed(&phone);
            }
        });
    }
}
