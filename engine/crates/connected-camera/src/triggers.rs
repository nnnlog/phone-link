//! What starts a sync or a stream, and listening for the phone's publications.

use super::{ConnectedCameraEvent, Inner, Triggers, key};
use phonelink_core::seams::SessionEvent;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_features::PhoneSwitchChanged;
use phonelink_features::shared::db::StoreError;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

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
                            Ok(PeerEvent::Added(peer)) => inner.start_listening(&peer, true),
                            Ok(PeerEvent::Removed(phone)) => inner.phone_removed(&phone),
                            Ok(PeerEvent::Updated(peer)) => {
                                if inner.with_phone(&peer.client, |p| p.listening) {
                                    inner.register_if_needed(&peer);
                                    inner.changed(&peer.client);
                                }
                            }
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "missed device-list events; their changes are not applied"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                    event = t.sessions.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(SessionEvent::Connected(phone)) => {
                                if inner.with_phone(&phone, |p| p.listening) {
                                    if let Some(peer) = inner.env.peers.peer(&phone) {
                                        inner.send_sync(&peer, false);
                                    }
                                }
                            }
                            Ok(SessionEvent::Disconnected { .. }) => {}
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "missed session events; phones that came up meanwhile get no camera metadata sync"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                    event = t.switches.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PhoneSwitchChanged { phone, enabled }) => {
                                match inner.env.peers.peer(&phone) {
                                    Some(peer) if enabled => inner.phone_reenabled(&peer),
                                    _ => inner.stop_listening(&phone),
                                }
                                inner.changed(&phone);
                            }
                            Err(RecvError::Lagged(n)) => warn!(skipped = n, "missed phone-switch events; their changes are not applied"),
                            Err(RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    /// Starts listening to an enabled Android phone (`force_session_start` for a phone that joined or
    /// was enabled again).
    pub(super) fn start_listening(&self, peer: &PeerDevice, force_session_start: bool) {
        let phone = &peer.client;
        if !self.camera.is_available() || !peer.info.is_android() || !self.phone_enabled(phone) {
            debug!(%phone, "no Mac camera, not an Android phone, or phone disabled; camera not listening");
            return;
        }

        self.with_phone(phone, |p| {
            p.listening = true;
            p.registration_failed = false;
        });

        self.register_if_needed(peer);
        self.send_sync(peer, force_session_start);
        self.changed(phone);
    }

    /// The phone's switch was turned back on.
    pub(super) fn phone_reenabled(&self, peer: &PeerDevice) {
        if !self.config.multiple_cameras && !self.other_registered(&peer.client).is_empty() {
            self.update_logged(&peer.client, |d| d.camera_enabled = false);
        }
        self.start_listening(peer, true);
    }

    pub(super) fn stop_listening(&self, phone: &DcgClient) {
        self.unregister(phone);
        self.with_phone(phone, |p| p.listening = false);
        self.changed(phone);
    }

    pub(super) fn phone_removed(&self, phone: &DcgClient) {
        self.stop_listening(phone);
        self.state.lock_or_panic().phones.remove(&key(phone));
    }

    /// The switch, with one camera at a time unless several are allowed.
    pub(super) fn set_camera_enabled(&self, peer: &PeerDevice, enabled: bool) -> Result<(), StoreError> {
        let phone = &peer.client;
        if enabled && !self.config.multiple_cameras {
            for other in self.other_registered(phone) {
                self.update(&other, |d| d.camera_enabled = false)?;
                self.with_phone(&other, |p| p.registration_failed = false);
                if let Some(other_peer) = self.env.peers.peer(&other) {
                    self.register_if_needed(&other_peer);
                    let name = other_peer.info.name.clone().unwrap_or_default();
                    self.emit(ConnectedCameraEvent::Replaced { phone: key(phone), replaced: other.clone(), name });
                } else {
                    self.unregister(&other);
                }
                self.changed(&other);
            }
        }

        if self.update(phone, |d| d.camera_enabled = enabled)? {
            info!(%phone, enabled, "camera switch changed");
            self.with_phone(phone, |p| p.registration_failed = false);
        }

        if self.with_phone(phone, |p| p.listening) {
            self.register_if_needed(peer);
        }
        self.changed(phone);
        Ok(())
    }
}
