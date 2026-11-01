//! The tasks that carry the layers' events into the core: device lists, sessions, the App layer, the clipboard switch.

use super::{CdehTrigger, CoreEvent, EngineCore, IdentityStack};
use crate::app::{AppEvent, ConnectKind};
use crate::auth::{AccountRemoved, PeerEvent};
use crate::features::clipboard::ClipboardEvent;
use crate::features::shared::peers::is_phone;
use crate::features::{Features, PhoneSwitchChanged};
use crate::seams::{PartnerSessionControl, SessionEvent};
use crate::types::DcgClient;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{debug, error, info, warn};

impl EngineCore {
    /// A change of the clipboard's global switch changes the advertised `CLIPBOARD` capability; the
    /// client-info refresh of the startup sequence sends it. Each change — and a lagged event stream,
    /// which may have lost one — re-reads the switch, so the advertised value always ends at the
    /// switch's current state. Once the account is removed the bridge stops: nothing more is
    /// advertised for it.
    pub(super) fn spawn_capability_bridge(self: &Arc<Self>, features: &Features) {
        let mut events = features.clipboard.subscribe();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(ClipboardEvent::GlobalSwitchChanged { .. }) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "clipboard events lagged; re-reading the global switch");
                    }
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }

                let Some(core) = weak.upgrade() else { break };
                if core.account_removed.load(Ordering::SeqCst) {
                    debug!("capability bridge stopped; the account was removed");
                    break;
                }

                let Some(features) = core.features() else { continue };
                let (auth, _, _) = core.cdeh_parts();
                match features.clipboard.advertised_capabilities(&core.base_capabilities) {
                    Ok(capabilities) => auth.set_capabilities(capabilities),
                    Err(e) => {
                        warn!(error = %e, "the clipboard's global switch not read; the advertised capabilities stay");
                        continue;
                    }
                }

                if let Err(e) = auth.refresh_client_info().await {
                    warn!(error = %e, "client info not refreshed after the clipboard switch changed; the account keeps the old capabilities");
                }
            }
        });
    }

    /// Reports session and App events as [`CoreEvent`]s, keeps the per-partner
    /// connection's copy of the phone's YPP capabilities current, re-applies the selected phone's
    /// device-list entry after every device-list refresh, changed or not — each application rewrites
    /// the phone record, and the device list and the connect response write the same fields, the last
    /// writer winning — and reacts to the removal of the account.
    pub(super) fn spawn_event_bridge(self: &Arc<Self>) {
        self.spawn_device_list_bridge();
        self.spawn_account_removal_bridges();
        self.spawn_peer_event_bridge();
        if let Some(cdeh) = &self.cdeh {
            self.spawn_cdeh_session_bridge(cdeh);
        }
        self.spawn_session_event_bridge();
        self.spawn_app_event_bridge();
    }

    /// Re-applies the selected phone's device-list entry after every device-list refresh (the cached
    /// list after a lag).
    fn spawn_device_list_bridge(self: &Arc<Self>) {
        let mut lists = self.auth.subscribe_device_list();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let peers = match lists.recv().await {
                    Ok(applied) => applied.peers,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "device-list events lagged; re-applying the cached list");
                        let Some(core) = weak.upgrade() else { break };
                        core.auth.peers()
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };

                let Some(core) = weak.upgrade() else { break };
                core.reapply_device_list(&peers);
            }
        });
    }

    /// Stops the features when either identity's account is removed.
    fn spawn_account_removal_bridges(self: &Arc<Self>) {
        // Either identity's account removal stops the features.
        let removals = std::iter::once(self.auth.subscribe_account_removed())
            .chain(self.cdeh.iter().map(|s| s.auth.subscribe_account_removed()));

        for mut accounts in removals {
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                // A lagged receiver may have lost a removal: it counts as one.
                while let Ok(AccountRemoved { .. }) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =
                    accounts.recv().await
                {
                    let Some(core) = weak.upgrade() else { break };
                    core.on_account_removed();
                }
            });
        }
    }

    /// Reacts to the selected phone's removal from the linked peers.
    fn spawn_peer_event_bridge(self: &Arc<Self>) {
        let mut peers = self.auth.subscribe_peer_events();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match peers.recv().await {
                    // The selected phone's record follows every device-list refresh (above).
                    Ok(PeerEvent::Added(peer)) => {
                        info!(
                            peer = %peer.client,
                            is_phone = is_phone(&peer.info),
                            "a device joined the account's linked devices"
                        );
                    }
                    Ok(PeerEvent::Updated(_)) => {}
                    Ok(PeerEvent::Removed(client)) => {
                        let Some(core) = weak.upgrade() else { break };
                        if core.phone().is_some_and(|p| p.is_same_client(&client)) {
                            core.on_phone_unlinked(&client);
                        } else {
                            info!(peer = %client, "a device left the account's linked devices");
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // A lost event may have been the selected phone's removal: the cached
                        // linked peers say.
                        warn!(n, "peer events lagged; checking the selected phone against the linked peers");
                        let Some(core) = weak.upgrade() else { break };
                        if let Some(phone) = core.phone() {
                            if !core.auth.peers().iter().any(|p| p.client.is_same_client(&phone)) {
                                core.on_phone_unlinked(&phone);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Reports the selected phone's session events as [`CoreEvent::PhoneSession`].
    fn spawn_session_event_bridge(self: &Arc<Self>) {
        let mut sessions = self.sessions.subscribe();
        let events = self.events.clone();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let event = sessions.recv().await;
                let Some(core) = weak.upgrade() else { break };

                // The UI shows the selected phone's session; a previous phone's disconnect is not it.
                let selected = |partner: &DcgClient| core.phone().is_some_and(|p| p.is_same_client(partner));
                match event {
                    Ok(SessionEvent::Connected(partner)) if selected(&partner) => {
                        events.core_event(CoreEvent::PhoneSession { connected: true, failure: None })
                    }
                    Ok(SessionEvent::Disconnected { partner, failure }) if selected(&partner) => {
                        events.core_event(CoreEvent::PhoneSession {
                            connected: false,
                            failure: Some(format!("{failure:?}")),
                        })
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "session events lagged; the UI may show a stale session state")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// Reports App events as [`CoreEvent`]s and keeps the partner's YPP capabilities current.
    fn spawn_app_event_bridge(self: &Arc<Self>) {
        let mut app_events = self.app.subscribe_events();
        let weak = Arc::downgrade(self);
        let events = self.events.clone();
        tokio::spawn(async move {
            loop {
                match app_events.recv().await {
                    Ok(AppEvent::PhoneRecordChanged { phone }) => {
                        let Some(core) = weak.upgrade() else { break };
                        core.sync_partner_capabilities(&phone);
                        events.core_event(CoreEvent::PhoneChanged);
                    }
                    Ok(AppEvent::PermissionsChanged { .. }) => events.core_event(CoreEvent::PhoneChanged),
                    Ok(AppEvent::BringWindowForward { uri }) => {
                        events.core_event(CoreEvent::BringWindowForward { uri })
                    }
                    // Screen mirroring serves it.
                    Ok(AppEvent::RemotingLaunch { .. }) => {}
                    Ok(AppEvent::AllFeaturesDisabled { disabled, .. }) => {
                        events.core_event(CoreEvent::AllFeaturesDisabled { disabled })
                    }
                    Ok(AppEvent::ConnectCompleted { kind, outcome, .. }) => events
                        .core_event(CoreEvent::ConnectCompleted { immediate: kind == ConnectKind::Immediate, outcome }),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "app events lagged; the UI may show a stale phone")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    /// The CDEH session's other triggers: the selected phone's per-phone switch turning off stops the
    /// session and turning on starts it (unless the Mac is locked); the CDEH identity's device list
    /// gaining the selected phone starts it, as a new session loop does, and losing it stops it.
    pub(super) fn spawn_cdeh_session_bridge(self: &Arc<Self>, cdeh: &IdentityStack) {
        let selected = |core: &EngineCore, phone: &DcgClient| core.phone().is_some_and(|p| p.is_same_client(phone));

        let mut switches = self.switches.subscribe();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match switches.recv().await {
                    Ok(PhoneSwitchChanged { phone, enabled }) => {
                        let Some(core) = weak.upgrade() else { break };
                        if !selected(&core, &phone) {
                            continue;
                        }

                        info!(%phone, enabled, "the phone's switch changed");
                        if enabled {
                            core.connect_cdeh(CdehTrigger::DeviceEnabled);
                        } else {
                            // A disabled phone's device connection is lost: mirroring releases with 81.
                            if let Some(features) = core.features() {
                                features.screen_mirroring.device_connection_lost();
                            }
                            core.stop_cdeh_session();
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // A lost change may have been the selected phone's: its switch now says.
                        warn!(n, "per-phone switch events lagged; applying the selected phone's switch");
                        let Some(core) = weak.upgrade() else { break };
                        let Some(phone) = core.phone() else { continue };
                        match core.switches.is_enabled(&phone) {
                            Ok(true) => core.connect_cdeh(CdehTrigger::DeviceEnabled),
                            Ok(false) => {
                                if let Some(features) = core.features() {
                                    features.screen_mirroring.device_connection_lost();
                                }
                                core.stop_cdeh_session();
                            }
                            Err(e) => {
                                error!(%phone, error = %e, "the per-phone switch not read; the CDEH session stays as it is")
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        let mut peers = cdeh.auth.subscribe_peer_events();
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                match peers.recv().await {
                    Ok(PeerEvent::Added(peer)) => {
                        let Some(core) = weak.upgrade() else { break };
                        if selected(&core, &peer.client) {
                            info!(phone = %peer.client, "the CDEH identity's device list gained the selected phone");
                            core.connect_cdeh(CdehTrigger::ProcessSessionChanges);
                        }
                    }
                    Ok(PeerEvent::Removed(client)) => {
                        let Some(core) = weak.upgrade() else { break };
                        if selected(&core, &client) {
                            info!(phone = %client, "the CDEH identity's device list lost the selected phone");
                            core.stop_cdeh_session();
                        }
                    }
                    Ok(PeerEvent::Updated(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "CDEH peer events lagged; the next device-list change applies");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}
