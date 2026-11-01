//! The Cross Device contract: pulling and pushing the status, and `/remotedevicestatus`.

use super::{CdehStatus, DeviceStatusEvent, Inner, Prompt, Received, cdeh, decode_received};
use phonelink_app::PhoneRecord;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_transport::platform::DrmSendOutcome;
use tracing::{debug, error, info, warn};

impl Inner {
    /// The status pull's first trigger: every phone of the account that is already available and
    /// has `DEVICE_STATUS`.
    pub(super) fn start_pulls(&self) {
        if !self.env.peers.is_enrolled() {
            debug!("device-status pulls at start skipped; no enrolled account");
            return;
        }

        for peer in self.env.peers.peers() {
            if self.env.sessions.is_connected(&peer.client) && cdeh::has_device_status(&peer) {
                self.spawn_pull(peer.client);
            }
        }
    }

    /// One status pull through CDEH's request wrapper, with its sender preconditions: nothing is
    /// sent without a current account or when the phone is not in its device list; an `OK` reply
    /// replaces the CDEH model; everything else is logged and the model kept. No retry. The request
    /// runs in its own task, which does not keep the feature alive (dropping it cancels the request).
    pub(super) fn spawn_pull(&self, phone: DcgClient) {
        if !self.env.peers.is_enrolled() {
            debug!(%phone, "device-status pull not sent; no current account");
            return;
        }
        let Some(peer) = self.env.peers.peer(&phone) else {
            debug!(%phone, "device-status pull not sent; the phone is not in the account's device list");
            return;
        };
        let phone_enabled = match self.env.switches.is_enabled(&peer.client) {
            Ok(enabled) => enabled,
            Err(e) => {
                error!(%phone, error = %e, "device-status pull not sent; the phone switch is not readable");
                return;
            }
        };

        let request = cdeh::pull_request(
            &peer,
            phone_enabled,
            self.env.config.platform.request_timeout,
            self.shutdown.child_token(),
        );

        let (weak, drm) = (self.weak.clone(), self.drm.clone());
        self.runtime.spawn(async move {
            let outcome = drm.send(request).await;
            let Some(inner) = weak.upgrade() else { return };
            match outcome {
                DrmSendOutcome::Success { payload } => match decode_received(&payload) {
                    Ok(Received::Ok(status)) => inner.set_cdeh_status(&peer.client, status),
                    Ok(Received::NotOk(status)) => {
                        info!(%phone, status, "device-status pull reply not OK; status kept")
                    }
                    Err(e) => warn!(%phone, error = %e, "device-status pull reply does not decode; status kept"),
                },
                other => {
                    warn!(%phone, outcome = ?other, error = other.cause(), "device-status pull failed; status kept")
                }
            }
        });
    }

    /// An `OK` pull reply or push replaces `phone`'s CDEH model.
    pub(crate) fn set_cdeh_status(&self, phone: &DcgClient, status: CdehStatus) {
        let record = self.record_for(phone);
        let withdrawn = {
            let mut state = self.state.lock_or_panic();
            let ps = state.phone(phone);
            ps.cdeh = Some(status);
            let record = record.unwrap_or_else(|| PhoneRecord::new(phone));
            Self::selected_changed(ps, &record);
            Self::withdraw_failure_prompt(ps, &record)
        };

        if withdrawn {
            self.emit(DeviceStatusEvent::PromptWithdrawn { phone: phone.clone(), prompt: Prompt::DndTurnOffFailed });
        }
        self.changed(phone);
        self.check_battery();
    }
}
