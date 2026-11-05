//! What starts the feature for a phone, and its registrations.

use super::requests::ConfigFollowUp;
use super::{Cause, Inner, PhoneData, PhoneFilesEvent, SyncState, Trigger, Triggers, key};
use phonelink_core::seams::SessionEvent;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_features::PhoneSwitchChanged;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

impl Inner {
    pub(super) fn spawn_loop(&self, mut t: Triggers) {
        use broadcast::error::RecvError;
        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = t.peers.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PeerEvent::Added(peer)) => inner.register(&peer),
                            Ok(PeerEvent::Removed(phone)) => inner.phone_removed(&phone),
                            Ok(PeerEvent::Updated(peer)) => inner.availability_update(&peer),
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "device-list events missed"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                    event = t.sessions.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(SessionEvent::Connected(phone)) => inner.became_available(&phone),
                            Ok(SessionEvent::Disconnected { partner, .. }) => inner.became_unavailable(&partner),
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "session events missed"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                    event = t.switches.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PhoneSwitchChanged { phone, .. }) => {
                                if let Some(peer) = inner.env.peers.peer(&phone) {
                                    inner.reconcile(&peer, Cause::PhoneSwitch);
                                }
                                inner.gate.notify_waiters();
                            }
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "phone-switch events missed"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    /// A phone of the account, at the start or when it joins.
    pub(super) fn register(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        if !peer.info.is_android() {
            return;
        }

        let data = match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => d,
            Err(e) => {
                error!(%phone, error = %e, "phone-files state unreadable; phone not registered");
                return;
            }
        };

        if self.phone_enabled(phone) && !data.permission_granted && !data.capable {
            self.spawn_config_sync(peer.clone(), false, Trigger::DeviceInitialization, ConfigFollowUp::None);
            return;
        }

        let running = self.reconcile(peer, Cause::Registered);
        if running {
            self.spawn_config_sync(peer.clone(), false, Trigger::FilesServiceStarted, ConfigFollowUp::RecordLtw);
        }
    }

    /// Starts or stops the feature for `peer` as its state says; starts its services when they may run.
    /// Returns whether it runs.
    pub(super) fn reconcile(&self, peer: &PeerDevice, cause: Cause) -> bool {
        let phone = &peer.client;
        let data = match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => d,
            Err(e) => {
                error!(%phone, error = %e, "phone-files state unreadable; the feature is not started or stopped");
                return false;
            }
        };

        let wanted = peer.info.is_android() && self.phone_enabled(phone) && data.enabled && data.capable;
        let (started, stopped) = self.set_running(phone, wanted);

        if stopped {
            self.on_stopped(phone, cause);
            return false;
        }
        if started {
            self.on_started(peer, cause);
        }

        let services =
            wanted && data.permission_granted && (started || matches!(cause, Cause::Config | Cause::Permission));
        if services {
            self.start_services(peer.clone());
        }

        self.gate.notify_waiters();
        wanted
    }

    /// Sets whether the feature runs for `phone`, cancelling its work when it stops; returns
    /// `(started, stopped)`.
    fn set_running(&self, phone: &DcgClient, wanted: bool) -> (bool, bool) {
        self.with_phone(phone, |p| {
            let was = p.running;
            p.running = wanted;
            if was && !wanted {
                p.run_cancel.cancel();
                p.run_cancel = CancellationToken::new();
                p.sync_cancel = p.run_cancel.child_token();
                p.sync = SyncState::Idle;
            }
            (wanted && !was, was && !wanted)
        })
    }

    /// The feature stopped for `phone`: its golden state is cleared.
    fn on_stopped(&self, phone: &DcgClient, cause: Cause) {
        info!(%phone, cause_kind = ?cause, "phone files stopped");
        if let Err(e) = self.store(phone).and_then(|s| s.clear()) {
            error!(%phone, error = %e, "golden state not cleared");
        }
        self.emit(PhoneFilesEvent::Running { phone: phone.clone(), running: false });
        self.status_changed(phone);
        self.gate.notify_waiters();
    }

    /// The feature started for `peer`; a switch also syncs its configuration.
    fn on_started(&self, peer: &PeerDevice, cause: Cause) {
        let phone = &peer.client;
        info!(%phone, cause_kind = ?cause, "phone files started");
        self.emit(PhoneFilesEvent::Running { phone: phone.clone(), running: true });
        self.status_changed(phone);
        if cause == Cause::Switch {
            self.spawn_config_sync(peer.clone(), true, Trigger::FilesServiceStarted, ConfigFollowUp::RecordLtw);
        }
    }

    /// The first metadata request, then the thumbnail prefetch.
    pub(super) fn start_services(&self, peer: PeerDevice) {
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let phone = peer.client.clone();
            let data = match inner.store(&phone).and_then(|s| s.data()) {
                Ok(d) => d,
                Err(e) => return error!(%phone, error = %e, "phone-files state unreadable; no first sync"),
            };

            let recent = data
                .last_full_refresh
                .is_some_and(|t| inner.now_secs() - t < inner.config.full_refresh_interval.as_secs() as i64);

            if data.initial_sync_done && recent {
                inner.clone().incremental_sync(&peer, None, false, Trigger::FilesServiceStarted).await;
            } else {
                inner.clone().full_refresh(&peer, Trigger::FilesServiceStarted).await;
            }

            inner.clone().prefetch(&peer).await;
        });
    }

    /// A phone left the account: its tree and state go.
    pub(super) fn phone_removed(&self, phone: &DcgClient) {
        let was_running = {
            let mut phones = self.phones.lock_or_panic();
            phones.remove(&key(phone)).map(|p| {
                p.run_cancel.cancel();
                p.sync_cancel.cancel();
                p.running
            })
        };

        if let Err(e) = self.store(phone).and_then(|s| {
            s.clear()?;
            s.update(|d| *d = PhoneData::default()).map(|_| ())
        }) {
            error!(%phone, error = %e, "phone-files state not removed");
        }

        if was_running == Some(true) {
            self.emit(PhoneFilesEvent::Running { phone: phone.clone(), running: false });
        }
        self.status_changed(phone);
    }

    pub(super) fn became_available(&self, phone: &DcgClient) {
        let up_to_date = self.with_phone(phone, |p| {
            p.breaker.reset();
            p.config_up_to_date
        });

        self.status_changed(phone);
        self.gate.notify_waiters();

        if let Some(peer) = self.env.peers.peer(phone) {
            if peer.info.is_android() && !up_to_date && self.phone_enabled(phone) {
                self.spawn_config_sync(peer, false, Trigger::DeviceBecameAvailable, ConfigFollowUp::None);
            }
        }
    }

    pub(super) fn became_unavailable(&self, phone: &DcgClient) {
        self.with_phone(phone, |p| {
            p.config_up_to_date = false;
            p.sync_cancel.cancel();
            p.sync_cancel = p.run_cancel.child_token();
        });
        self.status_changed(phone);
    }

    /// The phone's device-list entry changed while it is connected: the changes since the last sync, or
    /// first the configuration when it is stale.
    pub(super) fn availability_update(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        if !peer.info.is_android() || !self.available(phone) {
            return;
        }

        let data = match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => d,
            Err(e) => return error!(%phone, error = %e, "phone-files state unreadable; no refresh"),
        };

        let (up_to_date, running) = self.with_phone(phone, |p| (p.config_up_to_date, p.running));
        if up_to_date && running && data.initial_sync_done && data.permission_granted {
            self.spawn_refresh(peer.clone(), false, Trigger::DeviceUpdated);
        } else if !up_to_date {
            self.spawn_config_sync(peer.clone(), false, Trigger::DeviceUpdated, ConfigFollowUp::Reconnect);
        }
    }

    /// A refresh: a full one when nothing was synced, else an incremental one that is always sent.
    pub(super) fn spawn_refresh(&self, peer: PeerDevice, user: bool, trigger: Trigger) {
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let sequence = match inner.store(&peer.client).and_then(|s| s.data()) {
                Ok(d) => d.sequence,
                Err(e) => {
                    warn!(phone = %peer.client, error = %e, "phone-files state unreadable; a full refresh is sent");
                    -1
                }
            };

            if sequence == -1 {
                inner.full_refresh(&peer, trigger).await;
            } else {
                inner.incremental_sync(&peer, Some(i64::MAX), user, trigger).await;
            }
        });
    }
}
