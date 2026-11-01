//! The account's devices, unlinking a PC, and pairing a phone.

use super::{CoreEvent, CorePhoneLookup, EngineCore, EngineError, FetchMode, auth_err, setup_err};
use crate::auth::PeerDevice;
use crate::auth::pairing::{PairingFlow, PairingStartError};
use crate::features::shared::peers::is_phone;
use crate::types::DcgClient;
use std::sync::Arc;
use tracing::warn;

impl EngineCore {
    /// This Mac's own identities: the primary's and, when there is one, the CDEH identity's.
    pub fn own_clients(&self) -> Vec<DcgClient> {
        let cdeh = self.cdeh.as_ref().and_then(|s| s.auth.own_client());
        self.auth.own_client().into_iter().chain(cdeh).collect()
    }

    /// The account's PCs from the primary identity's device list (no network): every device that is
    /// not a phone, this Mac's own CDEH identity included.
    pub fn account_pcs(&self) -> Vec<PeerDevice> {
        self.auth.peers().into_iter().filter(|p| !is_phone(&p.info)).collect()
    }

    /// Unlinks `id`, one of [`EngineCore::account_pcs`] that is not one of this Mac's identities, from
    /// the account (`UnlinkDevice`, then a forced device-list refresh), and refreshes the CDEH
    /// identity's device list too. A phone or this Mac's own identity is refused.
    pub async fn unlink_pc(&self, id: &str) -> Result<(), EngineError> {
        if self.own_clients().iter().any(|c| c.id.eq_ignore_ascii_case(id)) {
            return Err(setup_err("this Mac's own identity cannot be unlinked here"));
        }
        let Some(pc) = self.account_pcs().into_iter().find(|p| p.client.id.eq_ignore_ascii_case(id)) else {
            return Err(setup_err("the PC is not in the account's device list".to_owned()));
        };

        self.auth.unlink_device(&pc.client).await.map_err(auth_err)?;
        if let Some(cdeh) = &self.cdeh {
            cdeh.auth.sync_trust(FetchMode::ForceCloud).await.map_err(auth_err)?;
        }

        Ok(())
    }

    /// Starts a QR/PIN pairing session ([`PairingManager::start`]); its steps are reported as
    /// [`CoreEvent::Pairing`]. A session that pairs refreshes both identities' device lists and
    /// succeeds when the phone is then one of [`EngineCore::phones`].
    pub fn start_pairing(self: &Arc<Self>, flow: PairingFlow) -> Result<u64, PairingStartError> {
        let events = self.events.clone();
        let lookup = Arc::new(CorePhoneLookup(Arc::downgrade(self)));
        self.pairing.start(
            flow,
            lookup,
            Arc::new(move |session, step| events.core_event(CoreEvent::Pairing { session, step })),
        )
    }

    /// Learns a phone that just joined the account: a forced device-list refresh of the primary
    /// identity, whose phone list the user selects from, and of the CDEH identity, whose connection
    /// must trust the phone too. Whether `id` is one of [`EngineCore::phones`] then.
    pub(super) async fn learn_paired_phone(&self, id: &str) -> bool {
        if let Err(e) = self.auth.sync_trust(FetchMode::ForceCloud).await {
            warn!(error = %e, "device list not refreshed after pairing; the paired phone is not found");
            return false;
        }
        if let Some(cdeh) = &self.cdeh
            && let Err(e) = cdeh.auth.sync_trust(FetchMode::ForceCloud).await
        {
            warn!(error = %e, "the CDEH identity's device list not refreshed after pairing; its session waits for a later refresh");
        }

        self.phones().iter().any(|p| p.client.id.eq_ignore_ascii_case(id))
    }
}
