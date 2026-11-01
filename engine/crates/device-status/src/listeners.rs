//! What the feature listens to: the phone record, the sessions, the device list.

use super::{DeviceStatusEvent, Inner, PhoneState, Prompt, Seen, cdeh, key, select};
use phonelink_app::{AppEvent, PhoneRecord};
use phonelink_core::seams::SessionEvent;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_transport::auth::PeerEvent;
use tokio::sync::broadcast;
use tracing::{debug, warn};

impl Inner {
    pub(super) fn spawn_listeners(
        &self,
        mut session_events: broadcast::Receiver<SessionEvent>,
        mut peer_events: broadcast::Receiver<PeerEvent>,
        mut app_events: broadcast::Receiver<AppEvent>,
    ) {
        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = session_events.recv() => event,
                };
                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(SessionEvent::Connected(phone)) => inner.on_availability(&phone, true),
                    Ok(SessionEvent::Disconnected { partner, .. }) => inner.on_availability(&partner, false),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "device status missed session events; their availability changes are lost")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = peer_events.recv() => event,
                };
                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(PeerEvent::Added(peer)) => {
                        if inner.env.sessions.is_connected(&peer.client) && cdeh::has_device_status(&peer) {
                            inner.spawn_pull(peer.client.clone());
                        }
                    }
                    Ok(PeerEvent::Removed(client)) => {
                        let had = inner.state.lock_or_panic().phones.remove(&key(&client)).is_some();
                        if had {
                            inner.changed(&client);
                        }
                    }
                    Ok(PeerEvent::Updated(_)) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "device status missed peer events; their pulls and removals are lost")
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = app_events.recv() => event,
                };
                let Some(inner) = weak.upgrade() else { break };
                match event {
                    Ok(AppEvent::PhoneRecordChanged { phone }) => inner.on_record_changed(&phone),
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "device status missed App events; re-reading the record");
                        if let Some(phone) = inner.env.app.selected_phone() {
                            inner.on_record_changed(&phone);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// The phone's platform connection came up or went down: both controls are re-read, and a
    /// phone that is now available with `DEVICE_STATUS` is pulled.
    pub(super) fn on_availability(&self, phone: &DcgClient, available: bool) {
        {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            ps.dnd.reset();
            ps.ringer.reset();
            if !available {
                // The control shows unknown; the ring it counted down ends.
                ps.ring = None;
            }
        }

        self.changed(phone);
        if available {
            match self.env.peers.peer(phone) {
                Some(peer) if cdeh::has_device_status(&peer) => self.spawn_pull(peer.client),
                Some(_) => {}
                None => debug!(%phone, "available phone not in the account's device list; not pulled"),
            }
        }
    }

    /// A write to the phone record (any writer of the Phone Link model, permissions, the
    /// all-features-disabled flag).
    pub(super) fn on_record_changed(&self, phone: &DcgClient) {
        let Some(record) = self.selected_record(phone) else { return };
        let now = Seen::of(&record);
        let withdrawn = {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            if ps.dnd.seen.replace(now).is_some_and(|before| before.dnd_reset(&now)) {
                ps.dnd.reset();
            }
            if ps.ringer.seen.replace(now).is_some_and(|before| before.ringer_reset(&now)) {
                ps.ringer.reset();
            }
            Self::selected_changed(ps, &record);
            if record.all_features_disabled {
                // The control shows unknown; the ring it counted down ends.
                ps.ring = None;
            }
            Self::withdraw_failure_prompt(ps, &record)
        };

        if withdrawn {
            self.emit(DeviceStatusEvent::PromptWithdrawn { phone: phone.clone(), prompt: Prompt::DndTurnOffFailed });
        }
        self.changed(phone);
        self.check_battery();
    }

    /// After a write to either model: a reverted control whose selected value moved on ends.
    pub(super) fn selected_changed(ps: &mut PhoneState, record: &PhoneRecord) {
        let sel = select(&record.status_keys, ps.cdeh.as_ref());
        ps.dnd.selected_now(sel.do_not_disturb_status);
        ps.ringer.selected_now(sel.ringer_mode);
    }

    /// The failure prompt ends once the DND status shows off (`1`).
    pub(super) fn withdraw_failure_prompt(ps: &mut PhoneState, record: &PhoneRecord) -> bool {
        if ps.failure_prompt && select(&record.status_keys, ps.cdeh.as_ref()).do_not_disturb_status == Some(1) {
            ps.failure_prompt = false;
            return true;
        }
        false
    }
}
