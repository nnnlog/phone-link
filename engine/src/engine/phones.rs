//! Selecting the phone, persisting the selection, and the phone leaving the account.

use super::{CdehStop, CdehTrigger, CoreEvent, EngineCore, EngineError, SELECTED_PHONE_KEY, linked_phone, setup_err};
use crate::app::ConnectTrigger;
use crate::auth::PeerDevice;
use crate::features::shared::peers::is_phone;
use crate::sync::MutexExt;
use crate::types::{DcgClient, DeviceInfoItem};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tracing::{error, info};

impl EngineCore {
    /// The phones the user can select: the account's linked phones, from the primary identity's device
    /// list (no network).
    pub fn phones(&self) -> Vec<PeerDevice> {
        self.auth.peers().into_iter().filter(|p| is_phone(&p.info)).collect()
    }

    /// The user selected `id`, one of [`EngineCore::phones`]. It is persisted; the first selection
    /// starts the phone session; a later one makes it the App layer's phone, sends it the background connect
    /// *selected phone changed*, moves the CDEH session to it, and disconnects the previous phone,
    /// which keeps only its enrollment. Selecting the selected phone changes nothing.
    pub async fn select_phone(self: &Arc<Self>, id: &str) -> Result<PeerDevice, EngineError> {
        if self.needs_rebuild() {
            return Err(setup_err("this engine core cannot start again; it must be rebuilt"));
        }
        if self.auth.own_client().is_none() {
            return Err(EngineError::NotStarted);
        }

        let phone = linked_phone(&self.auth.peers(), id)?;
        self.persist_phone(&phone.client)?;
        if !self.started.load(Ordering::SeqCst) {
            self.begin_phone(&phone)?;
            return Ok(phone);
        }

        let previous = self.phone();
        if previous.as_ref().is_some_and(|p| p.is_same_client(&phone.client)) && !self.phone_unlinked() {
            return Ok(phone);
        }

        info!(phone = %phone.client, "the user selected another phone");
        if let Some(features) = self.features() {
            features.screen_mirroring.selected_phone_changed();
        }
        self.select(&phone.client, &phone.info)?;
        self.app.trigger(ConnectTrigger::SelectedPhoneChanged);
        self.connect_cdeh(CdehTrigger::ProcessSessionChanges);

        if let Some(previous) = previous.filter(|p| !p.is_same_client(&phone.client)) {
            let sessions = self.sessions.clone();
            tokio::spawn(async move { sessions.disconnect(&previous, CdehStop::SessionLoopEnded.trace()).await });
        }

        self.events.core_event(CoreEvent::PhoneChanged);
        Ok(phone)
    }

    pub(super) fn persisted_phone(&self) -> Result<Option<String>, EngineError> {
        match self.edges.store.load(SELECTED_PHONE_KEY.into())? {
            None => Ok(None),
            Some(bytes) => {
                String::from_utf8(bytes).map(Some).map_err(|_| setup_err(format!("{SELECTED_PHONE_KEY} is not UTF-8")))
            }
        }
    }

    pub(super) fn persist_phone(&self, phone: &DcgClient) -> Result<(), EngineError> {
        Ok(self.edges.store.store(SELECTED_PHONE_KEY.into(), phone.id.clone().into_bytes())?)
    }

    /// A start selected `phone` (it is in the account's linked devices): the account gate opens for
    /// it again if an earlier selected phone had left them.
    pub(super) fn select(&self, phone: &DcgClient, info: &DeviceInfoItem) -> Result<(), EngineError> {
        self.app.set_phone(phone.clone(), info).map_err(setup_err)?;
        self.sync_partner_capabilities(phone);
        *self.phone.lock_or_recover() = Some(phone.clone());
        if let Some(displayed) = self.displayed_peers.get() {
            displayed.set_displayed(Some(phone.clone()));
        }
        self.phone_unlinked.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// The selected phone left the account's linked devices ("the phone is not linked to the current
    /// account"): App requests to it fail at the account gate — setting the "all features disabled"
    /// flag `false` — until a start selects a phone again. The UI hears of it once.
    pub(super) fn on_phone_unlinked(&self, phone: &DcgClient) {
        if self.phone_unlinked.swap(true, Ordering::SeqCst) {
            return;
        }
        info!(phone = %phone, "the selected phone left the account's linked devices; App requests stop");
        self.events.core_event(CoreEvent::PhoneUnlinked);
    }

    /// Whether the selected phone left the account's linked devices since the last start selected it.
    pub fn phone_unlinked(&self) -> bool {
        self.phone_unlinked.load(Ordering::SeqCst)
    }

    /// Rewrites the selected phone's record from its entry in `peers`, the linked peers of a
    /// device-list refresh; a phone absent from the list keeps its record.
    pub(super) fn reapply_device_list(&self, peers: &[PeerDevice]) {
        let Some(selected) = self.app.selected_phone() else { return };
        let Some(peer) = peers.iter().find(|p| p.client.is_same_client(&selected)) else { return };
        if let Err(e) = self.app.reapply_device_list(&selected, &peer.info) {
            error!(phone = %selected, error = %e, "device-list entry not applied to the phone record");
        }
    }

    /// The per-partner connection's session control reads the intentional-disconnect bit
    /// (`yppCapabilities & 0x40`) from the phone record the App layer keeps.
    pub(super) fn sync_partner_capabilities(&self, phone: &DcgClient) {
        if let Some(record) = self.app.phone_record() {
            self.sessions.set_partner_ypp_capabilities(phone, u64::from(record.ypp_capabilities));
        }
    }

    /// The selected phone; `None` until the user selects one.
    pub fn phone(&self) -> Option<DcgClient> {
        self.phone.lock_or_recover().clone()
    }
}
