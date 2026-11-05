//! Remote control: the phone locks, sleeps, shuts down or restarts the Mac. The Mac acts without
//! asking — no confirmation, countdown or notification — and answers: after acting, except a lock or a
//! sleep the Cross Device contract asks for, which it answers first.
//!
//! | Module | Implements |
//! |---|---|
//! | [`cdeh`] | the Cross Device contract: the `/remotecontrol` receiver, its gates, the `remote_control.v1` answer |
//! | [`phone_link`] | the Phone Link contract: the `remoteControl` content processor |
//! | [`store`] | the per-phone toggle |
//! | [`edge`] | the Mac's actions as an OS edge ([`PcPowerControl`]) and its stand-ins |
//!
//! Both contracts act through the same edge and obey the same per-phone toggle. The Cross Device
//! contract also needs the phone's `REMOTE_CONTROL` capability (`REMOTE_CONTROL_V2` for every command
//! but lock) and the phone's "enabled" switch, and the Mac advertises `REMOTE_CONTROL` and
//! `REMOTE_CONTROL_V2` as its options allow ([`with_remote_control_capabilities`]; both are on by
//! default, so the phone is offered every command the Mac carries out).

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod cdeh;
pub mod edge;
pub mod phone_link;
pub mod store;

use phonelink_core::config::features::RemoteControlConfig;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::warn;

use phonelink_core::types::{DcgClient, DeviceInfoItem};
use phonelink_features::shared::db::StoreError;
use phonelink_features::shared::peers::{PeerDirectory, is_phone};
use phonelink_features::{FeatureEnv, FeatureError, PhoneSwitches};
use phonelink_transport::auth::{AuthService, FetchMode, PeerEvent};
use phonelink_transport::platform::Drm;

pub use edge::PcPowerControl;
#[cfg(feature = "test-support")]
pub use edge::{PowerAction, RefusingPowerControl};
use phonelink_features::RegistrationError;
use store::RemoteControlStore;

/// The feature's name: its store.
pub const FEATURE: &str = "remote_control";

/// `base` with `REMOTE_CONTROL` and `REMOTE_CONTROL_V2` as `config` allows: any already in `base`
/// are removed, and the allowed ones are appended.
///
/// Hand the result to `AuthService::set_capabilities` (with the clipboard's).
pub fn with_remote_control_capabilities(base: &[String], config: &RemoteControlConfig) -> Vec<String> {
    let mut capabilities: Vec<String> =
        base.iter().filter(|c| c.as_str() != cdeh::CAPABILITY && c.as_str() != cdeh::CAPABILITY_V2).cloned().collect();

    if config.enabled {
        capabilities.push(cdeh::CAPABILITY.to_owned());
        if config.v2_enabled {
            capabilities.push(cdeh::CAPABILITY_V2.to_owned());
        }
    }

    capabilities
}

/// Fetches the account's device list from the cloud now; `true` when it was refreshed. A Cross
/// Device request from a phone that is unknown or lacks a capability is checked once more after it.
#[async_trait]
pub trait DeviceListRefresh: Send + Sync {
    async fn refresh_device_list(&self) -> bool;
}

#[async_trait]
impl DeviceListRefresh for AuthService {
    async fn refresh_device_list(&self) -> bool {
        match self.sync_trust(FetchMode::ForceCloud).await {
            Ok(_) => true,
            Err(e) => {
                warn!(error = %e, "device-list refresh failed; the request is gated on the stored list");
                false
            }
        }
    }
}

/// What the Settings window shows for a phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPhone {
    /// The per-phone toggle (default on).
    pub toggle: bool,
    /// The toggle applies: the phone advertises `REMOTE_CONTROL` and remote control is on on this
    /// Mac.
    pub applicable: bool,
    /// The phone advertises `REMOTE_CONTROL`.
    pub capable: bool,
    /// The phone can also put the Mac to sleep, shut it down and restart it.
    ///
    /// It advertises `REMOTE_CONTROL_V2`, and the V2 option is on, so the Mac advertises it too.
    pub v2_capable: bool,
}

impl RemoteControlPhone {
    /// What Settings shows for a phone whose device-list entry is `info` and whose toggle is `toggle`.
    fn new(info: &DeviceInfoItem, toggle: bool, config: &RemoteControlConfig) -> Self {
        let capable = info.has_capability(cdeh::CAPABILITY);
        Self {
            toggle,
            applicable: capable && config.enabled,
            capable,
            v2_capable: info.has_capability(cdeh::CAPABILITY_V2) && config.v2_enabled,
        }
    }
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlEvent {
    /// What [`RemoteControl::phone`] reports for `phone` may have changed.
    PhoneChanged { phone: DcgClient },
}

pub(crate) struct Inner {
    config: RemoteControlConfig,
    /// The Cross Device features' view of the account's phones (the selected phone).
    cdeh_peers: Arc<dyn PeerDirectory>,
    switches: Arc<PhoneSwitches>,
    store: RemoteControlStore,
    power: Arc<dyn PcPowerControl>,
    device_list: Arc<dyn DeviceListRefresh>,
    events: broadcast::Sender<RemoteControlEvent>,
}

/// The installed feature.
pub struct RemoteControl {
    inner: Arc<Inner>,
}

impl RemoteControl {
    /// Registers the `remoteControl` processor on `env`'s App layer and the `/remotecontrol` receiver
    /// on `drm` (the Cross Device identity's), which reads `cdeh_env`'s phones and switches; `refresh`
    /// refreshes that identity's device list. Call it once, inside the tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        cdeh_env: &FeatureEnv,
        config: RemoteControlConfig,
        drm: Arc<Drm>,
        power: Arc<dyn PcPowerControl>,
        refresh: Arc<dyn DeviceListRefresh>,
    ) -> Result<Arc<Self>, FeatureError> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(FeatureError::Programming("remote control must be installed inside a tokio runtime".into()));
        }

        let inner = Arc::new(Inner {
            config,
            cdeh_peers: cdeh_env.peers.clone(),
            switches: cdeh_env.switches.clone(),
            store: RemoteControlStore::open(&env.stores),
            power,
            device_list: refresh,
            events: broadcast::channel(16).0,
        });

        drm.register_follow_up_receiver(
            cdeh::RECEIVER_PATH,
            Arc::new(cdeh::RemoteControlReceiver(Arc::downgrade(&inner))),
        )
        .map_err(RegistrationError::from)?;
        env.app.register_processor_factory(
            &[phone_link::CONTENT_TYPE],
            Arc::new(phone_link::RemoteControlFactory(Arc::downgrade(&inner))),
        )?;

        spawn_peer_listener(&inner, cdeh_env.peers.subscribe());
        Ok(Arc::new(Self { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<RemoteControlEvent> {
        self.inner.events.subscribe()
    }

    pub fn config(&self) -> &RemoteControlConfig {
        &self.inner.config
    }

    /// What the Settings window shows for `phone`; `None` while it is not in the account's device list.
    pub fn phone(&self, phone: &DcgClient) -> Result<Option<RemoteControlPhone>, StoreError> {
        let Some(peer) = self.inner.cdeh_peers.peer(phone) else { return Ok(None) };
        let toggle = self.inner.store.toggle(phone)?;
        Ok(Some(RemoteControlPhone::new(&peer.info, toggle, &self.inner.config)))
    }

    /// Sets `phone`'s toggle; a change is reported.
    pub fn set_phone_toggle(&self, phone: &DcgClient, enabled: bool) -> Result<(), FeatureError> {
        if self.inner.store.set_toggle(phone, enabled)? {
            let _ = self.inner.events.send(RemoteControlEvent::PhoneChanged { phone: phone.clone() });
        }
        Ok(())
    }
}

/// A device-list change of a phone may change what Settings shows for it; a lagged stream reports
/// every phone.
fn spawn_peer_listener(inner: &Arc<Inner>, mut peers: broadcast::Receiver<PeerEvent>) {
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        loop {
            let changed: Vec<DcgClient> = match peers.recv().await {
                Ok(PeerEvent::Added(p) | PeerEvent::Updated(p)) if is_phone(&p.info) => vec![p.client],
                Ok(PeerEvent::Removed(c)) => vec![c],
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "peer events lagged; every phone's remote-control state is re-read");
                    let Some(inner) = weak.upgrade() else { break };
                    inner.cdeh_peers.peers().into_iter().filter(|p| is_phone(&p.info)).map(|p| p.client).collect()
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let Some(inner) = weak.upgrade() else { break };
            for phone in changed {
                let _ = inner.events.send(RemoteControlEvent::PhoneChanged { phone });
            }
        }
    });
}

#[cfg(test)]
mod tests;
