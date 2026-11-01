//! Each partner's state: its delays, its hub, being marked connected.

use super::{PartnerSessions, PartnerState, SessionPhase, Step};
use crate::platform::session_control::send_activate_session_request;
use crate::platform::tap_decode;
use phonelink_core::seams::{ConnectionFailure, PlatformRequestStatus, SessionEvent};
use phonelink_core::sync::MutexExt;
use phonelink_core::tap::Direction;
use phonelink_core::types::DcgClient;
use serde_json::json;
use std::collections::HashMap;
use std::time::Duration;
use tracing::{debug, info, warn};

impl PartnerSessions {
    pub(super) fn entry<'m>(
        &self,
        map: &'m mut HashMap<DcgClient, PartnerState>,
        partner: &DcgClient,
    ) -> &'m mut PartnerState {
        let base = self.config.relay.partner_retry_base;
        let ttl = self.config.auth.wake.ttl_default;
        map.entry(partner.clone()).or_insert_with(|| PartnerState::new(base, ttl))
    }

    /// Broadcasts `Connected`, with its line.
    pub(super) fn emit_connected(&self, partner: &DcgClient) {
        info!(partner = %partner, "session connected");
        self.publish(SessionEvent::Connected(partner.clone()));
    }

    /// Broadcasts `Disconnected` with `failure`, with its line; `action` says what the connection
    /// does next (`stop`, `pause`, `attempt`, `none`, `local_stop`, `session_ended`).
    pub(super) fn emit_disconnected(&self, partner: &DcgClient, failure: ConnectionFailure, action: &'static str) {
        info!(partner = %partner, ?failure, action, "session disconnected");
        self.publish(SessionEvent::Disconnected { partner: partner.clone(), failure });
    }

    fn publish(&self, event: SessionEvent) {
        let (partner, summary) = match &event {
            SessionEvent::Connected(p) => (p, json!({ "session_event": "connected" })),
            SessionEvent::Disconnected { partner, failure } => {
                (partner, json!({ "session_event": "disconnected", "failure": format!("{failure:?}") }))
            }
        };

        self.tap.emit(tap_decode::note_event(Direction::In, partner, summary));
        // No subscriber is fine.
        let _ = self.events.send(event);
    }

    pub(super) fn env_in_use(&self, partner: &DcgClient) -> bool {
        let map = self.partners.lock_or_panic();
        map.iter().any(|(p, s)| {
            p != partner
                && p.env == partner.env
                && matches!(s.phase, SessionPhase::Connected | SessionPhase::Attempting | SessionPhase::Paused)
        })
    }

    pub(super) fn spawn_activate(&self, partner: &DcgClient) {
        let router = self.router.clone();
        let partner = partner.clone();
        let timeout = self.config.platform.request_timeout;

        tokio::spawn(async move {
            let response = send_activate_session_request(&router, &partner, timeout).await;
            if response.status != PlatformRequestStatus::Success {
                warn!(partner = %partner, status = ?response.status, error = response.status.cause(), "/PlatformActivateSessionRequest not delivered; the session stays connected");
            }
        });
    }

    /// Resets the delays: the retry delay to its start value and the wake TTL to the default.
    pub(super) fn reset_delays(&self, s: &mut PartnerState) {
        s.retry_delay = self.config.relay.partner_retry_base;
        s.wake_ttl = self.config.auth.wake.ttl_default;
    }

    /// After an attempt whose wake ended `DeviceUnreachable`: the next wake uses the other TTL.
    pub(super) fn alternate_wake_ttl(&self, s: &mut PartnerState) {
        let wake = &self.config.auth.wake;
        s.wake_ttl = if s.wake_ttl == wake.ttl_default { wake.ttl_long } else { wake.ttl_default };
    }

    /// The attempts stop by themselves with `failure`: no driver, `Idle`, the delays reset, and every
    /// waiter that began earlier ends with `failure`. A connection the partner joined by itself while
    /// the attempt failed stays connected, with `failure` as its last failure.
    pub(super) fn stop_attempts(&self, s: &mut PartnerState, failure: ConnectionFailure) {
        s.driver = None;
        if s.phase == SessionPhase::Connected {
            self.reset_delays(s);
            s.last_failure = Some(failure);
            s.notify();
            return;
        }

        s.phase = SessionPhase::Idle;
        s.stops += 1;
        s.stop_failure = Some(failure);
        self.reset_delays(s);
        s.notify();
    }

    /// Doubles the retry delay; `None` (and the delay unchanged) once the doubled delay would reach
    /// the bound.
    pub(super) fn double_retry_delay(&self, s: &mut PartnerState) -> Option<Duration> {
        let doubled = s.retry_delay.saturating_mul(2);
        if doubled >= self.config.relay.partner_retry_fatal {
            return None;
        }
        s.retry_delay = doubled;
        Some(doubled)
    }

    /// The hub connection closes unless another partner of the environment uses it (the phone
    /// re-appeared on the hub after it ended the session).
    pub(super) fn release_hub(&self, partner: &DcgClient) {
        if self.env_in_use(partner) {
            debug!(partner = %partner, "hub connection kept: another partner of the environment uses it");
            return;
        }

        let hub = self.hub.clone();
        let env = partner.env;
        tokio::spawn(async move { hub.close(env).await });
    }

    /// Records the transition to connected: ends a driver that has not begun its attempt, and
    /// broadcasts `Connected` once. The delays are kept: a driver in its retry wait runs on and resets
    /// them if the partner is still on the hub when the wait runs out; one in an open, an announce or a
    /// wake finishes by itself. A phone-ended session stays ended: its last failure is cleared and the
    /// partner's hub connection is released (closed unless another partner uses it).
    pub(super) fn mark_connected(&self, partner: &DcgClient) {
        let (ended, announce) = {
            let mut map = self.partners.lock_or_panic();
            let s = self.entry(&mut map, partner);
            if let SessionPhase::SessionEnded { reason } = s.phase {
                s.last_failure = None;
                s.notify();
                (Some(reason), false)
            } else {
                if s.phase != SessionPhase::Connected {
                    s.phase = SessionPhase::Connected;
                    s.last_failure = None;
                    if s.driver.as_ref().is_some_and(|d| d.step == Step::Starting) {
                        s.stop_driver();
                    }
                    s.notify();
                }
                (None, !std::mem::replace(&mut s.announced, true))
            }
        };

        if let Some(reason) = ended {
            info!(partner = %partner, reason, "partner back on the hub after the phone ended the session; it stays ended and the hub connection is released");
            self.release_hub(partner);
        }
        if announce {
            self.emit_connected(partner);
        }
    }
}
