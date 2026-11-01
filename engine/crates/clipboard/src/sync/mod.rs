//! Feature-state sync (PC → phone).
//!
//! It covers the send rule and its triggers, the feature-state request the PC initiates, and
//! startup and shutdown (one manager per Android phone, a re-run on a global-switch change).
//!
//! One task owns the managers and handles every trigger in arrival order: device-list changes
//! ([`phonelink_features::PeerDirectory`] events), availability ([`SessionEvent`]s), the per-phone
//! enabled switch ([`PhoneSwitchChanged`]), and the user's changes of the global switch and the
//! per-phone toggle.
//!
//! The send rule, whose steps comments here cite by number:
//! 1. Nothing is sent while the phone's enabled switch is off.
//! 2. A phone that is not available gets nothing and is marked not up to date.
//! 3. The request type: `FEATURE_DISABLE` when the global switch is off or the phone lacks
//!    `CLIPBOARD`, else `FEATURE_ON` or `FEATURE_OFF` by the phone's toggle.

use phonelink_core::config::features::ClipboardConfig;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::wire::{self, RESOURCE_PATH};
use super::{Clipboard, ClipboardEvent, listed_capabilities};
use phonelink_core::seams::SessionEvent;
use phonelink_core::types::{DcgClient, Environment, TraceContext};
use phonelink_features::PhoneSwitchChanged;
use phonelink_proto::clipboard::v1::ClipboardRequestType;
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use phonelink_transport::platform::{DrmAddress, DrmSendOutcome, DrmSendRequest};

/// What the user changed (from the feature handle).
#[derive(Debug)]
pub(super) enum Command {
    GlobalSwitchChanged,
    ToggleChanged(DcgClient),
}

/// A phone, whatever case a source writes its id in.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PhoneKey(String, Environment);

pub(super) fn key(phone: &DcgClient) -> PhoneKey {
    PhoneKey(phone.id.to_ascii_lowercase(), phone.env)
}

/// One Android phone's feature-state manager: the last values its triggers compare against.
#[derive(Debug, Clone)]
pub(super) struct Manager {
    /// The phone as the device list names it.
    pub phone: DcgClient,
    /// The device-list `capabilities`, a missing list stored as the empty one; any change (an
    /// element or only the order) is a trigger.
    pub capabilities: Vec<String>,
    /// The device-list `clientVersion`, for the request wrapper's minimum-version check.
    pub client_version: Option<String>,
    pub available: bool,
    pub enabled: bool,
}

impl Manager {
    pub(super) fn capable(&self) -> bool {
        super::has_capability(&self.capabilities)
    }
}

pub(super) type Managers = HashMap<PhoneKey, Manager>;

/// The event streams the loop follows, subscribed before any state is read.
pub(super) struct Triggers {
    pub commands: mpsc::UnboundedReceiver<Command>,
    pub peers: broadcast::Receiver<PeerEvent>,
    pub sessions: broadcast::Receiver<SessionEvent>,
    pub switches: broadcast::Receiver<PhoneSwitchChanged>,
}

impl Clipboard {
    fn new_manager(&self, peer: &PeerDevice) -> Manager {
        let enabled = self.switches.is_enabled(&peer.client).unwrap_or_else(|e| {
            warn!(phone = %peer.client, error = %e, "phone switch unreadable; assumed on");
            true
        });

        Manager {
            phone: peer.client.clone(),
            capabilities: listed_capabilities(&peer.info).to_vec(),
            client_version: peer.info.client_version.clone(),
            available: self.sessions.is_connected(&peer.client),
            enabled,
        }
    }

    /// Startup: one manager per Android phone of the account, each sending its feature state with a
    /// forced session start.
    pub(super) fn start_managers(self: &Arc<Self>) -> Managers {
        let mut managers = Managers::new();
        for peer in self.peers.peers().into_iter().filter(|p| p.info.is_android()) {
            let manager = self.new_manager(&peer);
            self.send_feature_state(&manager, true, "startup");
            managers.insert(key(&peer.client), manager);
        }
        managers
    }

    /// The feature-state sync of one phone: nothing for a disabled phone; an unavailable phone is
    /// marked not up to date; otherwise `FEATURE_DISABLE` / `FEATURE_ON` / `FEATURE_OFF` goes out
    /// through the request wrapper, and only a success marks the phone up to date. No retry.
    /// Nothing after shutdown: the request is abandoned wherever it is when the feature stops.
    pub(super) fn send_feature_state(self: &Arc<Self>, manager: &Manager, force_session_start: bool, trigger: &str) {
        let phone = &manager.phone;
        let Some(request_type) = self.feature_state_to_send(manager, trigger) else {
            return;
        };

        debug!(%phone, trigger, request_type = request_type.as_str_name(), force_session_start, "sending the feature state");

        let me = self.clone();
        let manager = manager.clone();
        let trigger = trigger.to_owned();
        let cancel = self.request_cancel();
        tokio::spawn(me.deliver_feature_state(manager, request_type, force_session_start, trigger, cancel));
    }

    /// The checks before a feature state goes out: the request type to send, `None` (logged) when
    /// nothing is sent. An unavailable phone is marked not up to date here.
    fn feature_state_to_send(&self, manager: &Manager, trigger: &str) -> Option<ClipboardRequestType> {
        let phone = &manager.phone;
        if self.is_stopped() {
            debug!(%phone, trigger, "feature stopped; no feature state sent");
            return None;
        }
        if !self.phone_switch_on(phone, trigger) {
            return None;
        }
        if !self.sessions.is_connected(phone) {
            debug!(%phone, trigger, "phone not available; feature state marked not up to date");
            self.set_up_to_date(phone, false);
            return None;
        }

        match self.feature_state_type(manager) {
            Ok(t) => Some(t),
            Err(e) => {
                warn!(%phone, trigger, error = %e, "settings unreadable; no feature state sent");
                None
            }
        }
    }

    /// The phone's enabled switch, `false` (logged) when it is off or unreadable.
    fn phone_switch_on(&self, phone: &DcgClient, trigger: &str) -> bool {
        match self.switches.is_enabled(phone) {
            Ok(true) => true,
            Ok(false) => {
                debug!(%phone, trigger, "phone disabled; no feature state sent");
                false
            }
            Err(e) => {
                warn!(%phone, trigger, error = %e, "phone switch unreadable; no feature state sent");
                false
            }
        }
    }

    /// The spawned send of one feature state through the request wrapper; only a success marks the
    /// phone up to date.
    async fn deliver_feature_state(
        self: Arc<Self>,
        manager: Manager,
        request_type: ClipboardRequestType,
        force_session_start: bool,
        trigger: String,
        cancel: CancellationToken,
    ) {
        let phone = manager.phone.clone();
        if cancel.is_cancelled() {
            debug!(%phone, %trigger, "feature stopped; feature state not sent");
            return;
        }

        // The wrapper reads the per-phone switch again when it sends.
        let phone_enabled = self.switches.is_enabled(&phone).unwrap_or_else(|e| {
            warn!(%phone, %trigger, error = %e, "phone switch unreadable; the feature state is sent as for a disabled phone");
            false
        });

        let request =
            feature_state_send(&manager, request_type, phone_enabled, force_session_start, &self.config, cancel);
        let Some(outcome) = self.send_request(request).await else {
            debug!(%phone, %trigger, "feature stopped; feature state not sent");
            return;
        };

        log_delivery(&phone, &trigger, request_type, &outcome);
        // The phone's answer payload is not read.
        if matches!(outcome, DrmSendOutcome::Success { .. }) {
            self.set_up_to_date(&phone, true);
        }
    }

    /// Step 3: global switch off or the phone lacks `CLIPBOARD` → `FEATURE_DISABLE`; else the toggle
    /// selects `FEATURE_ON` / `FEATURE_OFF`.
    fn feature_state_type(&self, manager: &Manager) -> Result<ClipboardRequestType, phonelink_features::StoreError> {
        if !self.store.global_switch()? || !manager.capable() {
            return Ok(ClipboardRequestType::FeatureDisable);
        }
        Ok(if self.store.toggle(&manager.phone)? {
            ClipboardRequestType::FeatureOn
        } else {
            ClipboardRequestType::FeatureOff
        })
    }

    fn set_up_to_date(&self, phone: &DcgClient, up_to_date: bool) {
        match self.store.set_up_to_date(phone, up_to_date) {
            Ok(true) => self.emit(ClipboardEvent::PhoneChanged { phone: phone.clone() }),
            Ok(false) => {}
            Err(e) => {
                error!(%phone, is_up_to_date = up_to_date, error = %e, "clipboardFeatureStatusUpToDate not stored")
            }
        }
    }

    /// The trigger "availability or enabled flag changed while not up to date".
    fn update_if_not_up_to_date(self: &Arc<Self>, manager: &Manager, trigger: &str) {
        match self.store.up_to_date(&manager.phone) {
            Ok(false) => self.send_feature_state(manager, false, trigger),
            Ok(true) => {}
            Err(e) => {
                warn!(phone = %manager.phone, trigger, error = %e, "clipboardFeatureStatusUpToDate unreadable; no feature state sent")
            }
        }
    }

    fn on_command(self: &Arc<Self>, managers: &mut Managers, command: Command) {
        match command {
            // The startup sequence re-runs — every phone's feature state again (no
            // forced session start), a fresh tag-9 subscription.
            Command::GlobalSwitchChanged => {
                for manager in managers.values() {
                    self.send_feature_state(manager, false, "global switch changed");
                }
                self.subscribe_announcements();
            }
            Command::ToggleChanged(phone) => match managers.get(&key(&phone)) {
                Some(manager) => self.send_feature_state(manager, true, "toggle changed"),
                None => debug!(%phone, "toggle of a phone without a feature-state manager"),
            },
        }
    }

    fn on_peer_added(self: &Arc<Self>, managers: &mut Managers, peer: PeerDevice) {
        if !peer.info.is_android() {
            return;
        }
        if managers.contains_key(&key(&peer.client)) {
            return self.on_peer_updated(managers, peer);
        }

        // A phone newly linked: a manager, its feature state with a forced session start.
        let manager = self.new_manager(&peer);
        self.send_feature_state(&manager, true, "phone linked");
        managers.insert(key(&peer.client), manager);
        self.emit(ClipboardEvent::PhoneChanged { phone: peer.client });
    }

    fn on_peer_updated(self: &Arc<Self>, managers: &mut Managers, peer: PeerDevice) {
        let Some(manager) = managers.get_mut(&key(&peer.client)) else {
            return self.on_peer_added(managers, peer);
        };

        manager.client_version = peer.info.client_version.clone();

        // A missing list and an empty one are the same list.
        let listed = listed_capabilities(&peer.info);
        if manager.capabilities.as_slice() != listed {
            manager.capabilities = listed.to_vec();
            self.emit(ClipboardEvent::PhoneChanged { phone: manager.phone.clone() });
            let manager = manager.clone();
            self.send_feature_state(&manager, false, "capabilities changed");
        }
    }

    fn on_peer_removed(&self, managers: &mut Managers, phone: &DcgClient) {
        if managers.remove(&key(phone)).is_some() {
            info!(%phone, "phone left the account; its feature-state manager is dropped");
            self.emit(ClipboardEvent::PhoneChanged { phone: phone.clone() });
        }
    }

    fn on_availability(self: &Arc<Self>, managers: &mut Managers, phone: &DcgClient, available: bool) {
        let Some(manager) = managers.get_mut(&key(phone)) else { return };
        if manager.available == available {
            return;
        }

        manager.available = available;
        let manager = manager.clone();
        self.emit(ClipboardEvent::PhoneChanged { phone: manager.phone.clone() });
        self.update_if_not_up_to_date(&manager, "availability changed");
    }

    fn on_enabled(self: &Arc<Self>, managers: &mut Managers, phone: &DcgClient, enabled: bool) {
        let Some(manager) = managers.get_mut(&key(phone)) else { return };
        if manager.enabled == enabled {
            return;
        }

        manager.enabled = enabled;
        let manager = manager.clone();
        self.emit(ClipboardEvent::PhoneChanged { phone: manager.phone.clone() });

        // Sent only when the phone is now enabled (the send rule's step 1) …
        self.send_feature_state(&manager, false, "enabled flag changed");
        // … and, as a device update, again while not up to date.
        self.update_if_not_up_to_date(&manager, "enabled flag changed");
    }

    /// After a lagged device-list stream: compare every manager with the current list.
    fn resync_peers(self: &Arc<Self>, managers: &mut Managers) {
        let peers = self.peers.peers();
        let gone: Vec<DcgClient> = managers
            .values()
            .filter(|m| !peers.iter().any(|p| key(&p.client) == key(&m.phone) && p.info.is_android()))
            .map(|m| m.phone.clone())
            .collect();

        for phone in gone {
            self.on_peer_removed(managers, &phone);
        }

        for peer in peers {
            self.on_peer_updated(managers, peer);
        }
    }

    fn resync_availability(self: &Arc<Self>, managers: &mut Managers) {
        let phones: Vec<DcgClient> = managers.values().map(|m| m.phone.clone()).collect();
        for phone in phones {
            let available = self.sessions.is_connected(&phone);
            self.on_availability(managers, &phone, available);
        }
    }

    fn resync_enabled(self: &Arc<Self>, managers: &mut Managers) {
        let phones: Vec<DcgClient> = managers.values().map(|m| m.phone.clone()).collect();
        for phone in phones {
            match self.switches.is_enabled(&phone) {
                Ok(enabled) => self.on_enabled(managers, &phone, enabled),
                Err(e) => warn!(%phone, error = %e, "phone switch unreadable; its change is not applied"),
            }
        }
    }
}

/// The one line of a feature-state send's outcome.
fn log_delivery(phone: &DcgClient, trigger: &str, request_type: ClipboardRequestType, outcome: &DrmSendOutcome) {
    let request_type = request_type.as_str_name();
    match outcome {
        DrmSendOutcome::Success { .. } => info!(%phone, trigger, request_type, "feature state delivered"),
        outcome => {
            warn!(%phone, trigger, request_type, ?outcome, "feature state not delivered; up-to-date flag unchanged, no retry");
        }
    }
}

/// A feature-state request through the request wrapper: `GET /clipboard` (path-addressed) with `{
/// request_type }` and no correlation id, the platform request timeout, and no cancellation of its
/// own (`cancel` is the feature's shutdown).
fn feature_state_send(
    manager: &Manager,
    request_type: ClipboardRequestType,
    phone_enabled: bool,
    force_session_start: bool,
    config: &ClipboardConfig,
    cancel: CancellationToken,
) -> DrmSendRequest {
    DrmSendRequest {
        target: manager.phone.clone(),
        address: DrmAddress::Path(RESOURCE_PATH.to_owned()),
        request_type: ProtoDeviceResourceRequestType::Get,
        payload: wire::feature_state_request(request_type),
        client_version: manager.client_version.clone(),
        phone_enabled,
        force_session_start,
        timeout: config.request_timeout,
        trace: TraceContext::new_root(Vec::new()),
        cancel,
    }
}

/// Spawns the task that owns `managers` and handles the triggers until shutdown.
pub(super) fn spawn_loop(clipboard: &Arc<Clipboard>, mut managers: Managers, triggers: Triggers) {
    let weak = Arc::downgrade(clipboard);
    let shutdown = clipboard.shutdown.clone();
    let Triggers { mut commands, peers: mut peer_events, sessions: mut session_events, switches: mut switch_events } =
        triggers;

    tokio::spawn(async move {
        let (mut peers_open, mut sessions_open, mut switches_open) = (true, true, true);
        loop {
            tokio::select! {
                // Shutdown first: triggers still queued when the feature stops are not handled.
                biased;
                _ = shutdown.cancelled() => break,
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    let Some(c) = weak.upgrade() else { break };
                    c.on_command(&mut managers, command);
                }
                event = peer_events.recv(), if peers_open => {
                    let Some(c) = weak.upgrade() else { break };
                    match event {
                        Ok(PeerEvent::Added(peer)) => c.on_peer_added(&mut managers, peer),
                        Ok(PeerEvent::Updated(peer)) => c.on_peer_updated(&mut managers, peer),
                        Ok(PeerEvent::Removed(phone)) => c.on_peer_removed(&mut managers, &phone),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "missed device-list events; resyncing");
                            c.resync_peers(&mut managers);
                        }
                        Err(broadcast::error::RecvError::Closed) => peers_open = false,
                    }
                }
                event = session_events.recv(), if sessions_open => {
                    let Some(c) = weak.upgrade() else { break };
                    match event {
                        Ok(SessionEvent::Connected(phone)) => c.on_availability(&mut managers, &phone, true),
                        Ok(SessionEvent::Disconnected { partner, .. }) => c.on_availability(&mut managers, &partner, false),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "missed session events; resyncing");
                            c.resync_availability(&mut managers);
                        }
                        Err(broadcast::error::RecvError::Closed) => sessions_open = false,
                    }
                }
                event = switch_events.recv(), if switches_open => {
                    let Some(c) = weak.upgrade() else { break };
                    match event {
                        Ok(PhoneSwitchChanged { phone, enabled }) => c.on_enabled(&mut managers, &phone, enabled),
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "missed phone-switch events; resyncing");
                            c.resync_enabled(&mut managers);
                        }
                        Err(broadcast::error::RecvError::Closed) => switches_open = false,
                    }
                }
            }
        }

        debug!("feature-state loop stopped");
    });
}

#[cfg(test)]
mod tests;
