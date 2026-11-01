//! The account's linked phones as the features read them.
//!
//! It covers each phone's DCG device-list entry (of `/DeviceAuthProxy/GetDeviceInfoList`: its
//! `capabilities` strings such as `CLIPBOARD` or `DEVICE_STATUS`, its `clientVersion`) and the
//! changes of each device-list refresh, which the trust synchronization of peers into `AsyncD2D`
//! follows.

use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tracing::warn;

use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, DeviceInfoItem};
use phonelink_transport::auth::store::AccountState;
use phonelink_transport::auth::{AuthService, PeerDevice, PeerEvent};

/// A linked peer is a phone when its `clientType` is `LTW` (Link to Windows) or its `osName` is
/// Android.
pub fn is_phone(info: &DeviceInfoItem) -> bool {
    info.client_type.as_deref() == Some("LTW")
        || info.os_name.as_deref().is_some_and(|o| o.eq_ignore_ascii_case("android"))
}

/// The account's peers as the features that serve only the displayed (selected) phone see them: every
/// linked peer except the phones other than the displayed one.
///
/// A change of the displayed phone is reported as the previous phone's `Removed` and the new
/// phone's `Added`.
pub struct DisplayedPeers {
    inner: Arc<dyn PeerDirectory>,
    displayed: Arc<Mutex<Option<DcgClient>>>,
    events: broadcast::Sender<PeerEvent>,
}

impl DisplayedPeers {
    /// Forwards `inner`'s events for the peers this view shows; call it inside the tokio runtime.
    pub fn new(inner: Arc<dyn PeerDirectory>) -> Arc<Self> {
        let displayed: Arc<Mutex<Option<DcgClient>>> = Arc::default();
        let events = broadcast::channel(64).0;
        let mut source = inner.subscribe();
        let (shown, sink) = (displayed.clone(), events.clone());
        let directory = inner.clone();

        tokio::spawn(async move {
            loop {
                let event = match source.recv().await {
                    Ok(e) => e,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "peer events lagged; the displayed-phone view forwards only what follows");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };

                let current = shown.lock_or_recover().clone();
                let keep = |client: &DcgClient, phone: bool| {
                    !phone || current.as_ref().is_some_and(|d| d.is_same_client(client))
                };

                let forward = match &event {
                    PeerEvent::Added(p) | PeerEvent::Updated(p) => keep(&p.client, is_phone(&p.info)),
                    // A removed peer's entry is gone: it was shown when it is the displayed phone or no phone.
                    PeerEvent::Removed(c) => {
                        current.as_ref().is_some_and(|d| d.is_same_client(c))
                            || !directory.peers().iter().any(|p| p.client.is_same_client(c) && is_phone(&p.info))
                    }
                };

                if forward {
                    let _ = sink.send(event);
                }
            }
        });
        Arc::new(Self { inner, displayed, events })
    }

    /// The displayed phone is now `phone` (`None`: no phone).
    pub fn set_displayed(&self, phone: Option<DcgClient>) {
        let previous = {
            let mut shown = self.displayed.lock_or_recover();
            if shown.as_ref() == phone.as_ref() {
                return;
            }
            std::mem::replace(&mut *shown, phone.clone())
        };

        if let Some(previous) = previous {
            let _ = self.events.send(PeerEvent::Removed(previous));
        }
        if let Some(peer) = phone.and_then(|p| self.inner.peer(&p)) {
            let _ = self.events.send(PeerEvent::Added(peer));
        }
    }
}

impl PeerDirectory for DisplayedPeers {
    fn peers(&self) -> Vec<PeerDevice> {
        let current = self.displayed.lock_or_recover().clone();
        self.inner
            .peers()
            .into_iter()
            .filter(|p| !is_phone(&p.info) || current.as_ref().is_some_and(|d| d.is_same_client(&p.client)))
            .collect()
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.events.subscribe()
    }

    fn is_enrolled(&self) -> bool {
        self.inner.is_enrolled()
    }
}

/// The linked peers of the enrolled account.
pub trait PeerDirectory: Send + Sync {
    /// The linked peers with a trust relationship, from the last device-list refresh (no network).
    fn peers(&self) -> Vec<PeerDevice>;
    /// `Added` / `Removed` / `Updated` peers of every later refresh.
    fn subscribe(&self) -> broadcast::Receiver<PeerEvent>;
    /// The account is enrolled (the PC's identity is provisioned).
    fn is_enrolled(&self) -> bool;

    /// `phone`'s entry (same id, case-insensitive, and environment).
    fn peer(&self, phone: &DcgClient) -> Option<PeerDevice> {
        self.peers().into_iter().find(|p| p.client.env == phone.env && p.client.id.eq_ignore_ascii_case(&phone.id))
    }
}

impl PeerDirectory for AuthService {
    fn peers(&self) -> Vec<PeerDevice> {
        AuthService::peers(self)
    }

    fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
        self.subscribe_peer_events()
    }

    fn is_enrolled(&self) -> bool {
        self.account_info().is_some_and(|a| a.state == AccountState::Enrolled)
    }
}

/// In-memory implementations for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use super::PeerDirectory;
    use phonelink_core::sync::MutexExt;
    use phonelink_core::types::DcgClient;
    use phonelink_transport::auth::PeerDevice;
    use phonelink_transport::auth::PeerEvent;
    use std::sync::Mutex;
    use tokio::sync::broadcast;

    /// A peer list the test edits; each edit is reported as the refresh event it stands for.
    pub struct StaticPeers {
        peers: Mutex<Vec<PeerDevice>>,
        events: broadcast::Sender<PeerEvent>,
        enrolled: std::sync::atomic::AtomicBool,
    }

    impl StaticPeers {
        pub fn new(peers: Vec<PeerDevice>) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                peers: Mutex::new(peers),
                events: broadcast::channel(64).0,
                enrolled: std::sync::atomic::AtomicBool::new(true),
            })
        }

        pub fn set_enrolled(&self, enrolled: bool) {
            self.enrolled.store(enrolled, std::sync::atomic::Ordering::SeqCst);
        }

        /// Adds `peer`, or replaces the entry with its id, and reports `Added` / `Updated`.
        pub fn upsert(&self, peer: PeerDevice) {
            let event = {
                let mut peers = self.peers.lock_or_panic();
                match peers.iter_mut().find(|p| p.client == peer.client) {
                    Some(slot) => {
                        *slot = peer.clone();
                        PeerEvent::Updated(peer)
                    }
                    None => {
                        peers.push(peer.clone());
                        PeerEvent::Added(peer)
                    }
                }
            };

            let _ = self.events.send(event);
        }

        /// Removes the peer and reports `Removed`.
        pub fn remove(&self, client: &DcgClient) {
            self.peers.lock_or_panic().retain(|p| &p.client != client);
            let _ = self.events.send(PeerEvent::Removed(client.clone()));
        }
    }

    impl PeerDirectory for StaticPeers {
        fn peers(&self) -> Vec<PeerDevice> {
            self.peers.lock_or_panic().clone()
        }
        fn subscribe(&self) -> broadcast::Receiver<PeerEvent> {
            self.events.subscribe()
        }
        fn is_enrolled(&self) -> bool {
            self.enrolled.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
}

#[cfg(test)]
mod tests;
