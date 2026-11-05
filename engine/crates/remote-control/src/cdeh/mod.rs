//! The Cross Device Experience Host contract: the `/remotecontrol` receiver on
//! `/DeviceResourceManager`, its gates, and the `remote_control.v1` answer.
//!
//! A lock or a sleep is answered `SUCCESS` first, and the Mac acts only once that answer was sent;
//! the other actions are answered after the Mac acted, with their outcome.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use prost::Message;
use tracing::{error, info, warn};

use super::Inner;
use super::edge::{PcPowerControl, PowerAction, perform};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::peers::is_phone;
use phonelink_proto::remote_control::v1::{
    RemoteControlRequestMessage, RemoteControlResponseMessage, RemoteControlResult, RemoteControlType,
};
use phonelink_transport::platform::{AnswerDelivery, DrmAnswer, DrmFollowUpReceiver, DrmReceiverError, DrmRequest};

/// The receiver path.
pub const RECEIVER_PATH: &str = "/remotecontrol";
/// The phone capability every command needs (exact string).
pub const CAPABILITY: &str = "REMOTE_CONTROL";
/// The phone capability the commands after `LOCK` need (exact string).
pub const CAPABILITY_V2: &str = "REMOTE_CONTROL_V2";

/// `control_type` 3…8 (`SLEEP` … `QUERY_UPDATE_AVAILABILITY`) needs the phone's `REMOTE_CONTROL_V2`;
/// `LOCK`, `TRIGGER_TOGGLE_ON` and every number outside 3…8 do not.
pub fn requires_v2(control_type: i32) -> bool {
    (3..=8).contains(&control_type)
}

/// The answer bytes: `control_result`, and `pc_update_available` when given.
pub fn answer(result: RemoteControlResult, pc_update_available: Option<bool>) -> Vec<u8> {
    RemoteControlResponseMessage { control_result: result as i32, pc_update_available }.encode_to_vec()
}

/// What one gate pass decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Act on the request.
    Act,
    /// Answer with this result without acting.
    Answer(RemoteControlResult),
    /// The phone is unknown or lacks a capability the request needs: refresh the device list (pass 1),
    /// or answer `FAILED` (pass 2).
    Refresh,
}

/// The `/remotecontrol` receiver.
pub(crate) struct RemoteControlReceiver(pub(crate) Weak<Inner>);

impl Inner {
    /// One gate pass on the source phone's current device-list entry, its toggle and its enabled
    /// flag. A store failure reading either flag answers `FAILED`.
    pub(crate) fn gate(&self, source: &DcgClient, control_type: i32) -> Gate {
        if self.incapable(source, control_type).is_some() {
            return Gate::Refresh;
        }
        if let Some(answer) = self.toggle_gate(source) {
            return answer;
        }
        self.enabled_gate(source)
    }

    /// Why the source is not the account's phone with the capabilities `control_type` needs;
    /// `None` when it is.
    fn incapable(&self, source: &DcgClient, control_type: i32) -> Option<&'static str> {
        let Some(peer) = self.cdeh_peers.peer(source).filter(|p| is_phone(&p.info)) else {
            return Some("not the account's phone");
        };
        if !peer.info.has_capability(CAPABILITY) {
            return Some("the phone lacks REMOTE_CONTROL");
        }
        if requires_v2(control_type) && !peer.info.has_capability(CAPABILITY_V2) {
            return Some("the phone lacks REMOTE_CONTROL_V2");
        }
        None
    }

    /// The answer the phone's remote-control toggle decides, `None` when it is on.
    fn toggle_gate(&self, source: &DcgClient) -> Option<Gate> {
        match self.store.toggle(source) {
            Ok(true) => None,
            Ok(false) => {
                info!(partner = %source, "remote control refused; it is turned off for this phone");
                Some(Gate::Answer(RemoteControlResult::Disabled))
            }
            Err(e) => {
                error!(partner = %source, error = %e, "remote-control toggle not read; answered FAILED");
                Some(Gate::Answer(RemoteControlResult::Failed))
            }
        }
    }

    /// The gate's last step: the phone's enabled flag on this Mac.
    fn enabled_gate(&self, source: &DcgClient) -> Gate {
        match self.switches.is_enabled(source) {
            Ok(true) => Gate::Act,
            Ok(false) => {
                info!(partner = %source, "remote control refused; the phone is disabled on this Mac");
                Gate::Answer(RemoteControlResult::Disabled)
            }
            Err(e) => {
                error!(partner = %source, error = %e, "phone enabled flag not read; answered FAILED");
                Gate::Answer(RemoteControlResult::Failed)
            }
        }
    }

    /// The whole request: the answer, after any action — or, for a lock or a sleep, with the action
    /// to take once the answer was sent.
    pub(crate) async fn handle_cdeh(&self, request: &DrmRequest) -> DrmAnswer {
        let source = &request.source;
        let control_type = match self.control_type(request) {
            Ok(control_type) => control_type,
            Err(answer) => return answer.into(),
        };

        info!(partner = %source, control_type, "remote control request received");
        if let Gate::Answer(result) = self.decide(source, control_type).await {
            return answer(result, None).into();
        }

        let action = match power_action(source, control_type) {
            Ok(action) => action,
            Err(answer) => return answer.into(),
        };

        if answered_first(action) {
            let (power, source) = (self.power.clone(), source.clone());
            return DrmAnswer::then(answer(RemoteControlResult::Success, None), move |delivery| {
                act_once_answered(power, source, action, delivery)
            });
        }

        let done = perform(self.power.clone(), action).await;
        answer(if done { RemoteControlResult::Success } else { RemoteControlResult::Failed }, None).into()
    }

    /// The request's `control_type`, or the answer (logged) when the payload is empty, remote
    /// control is off on this Mac or the payload does not decode.
    fn control_type(&self, request: &DrmRequest) -> Result<i32, Vec<u8>> {
        let source = &request.source;
        if request.payload.is_empty() {
            warn!(partner = %source, "remote control request with an empty payload; answered FAILED");
            return Err(answer(RemoteControlResult::Failed, None));
        }
        if !self.config.enabled {
            info!(partner = %source, "remote control refused; it is off on this Mac");
            return Err(answer(RemoteControlResult::Disabled, None));
        }

        match RemoteControlRequestMessage::decode(request.payload.as_slice()) {
            Ok(m) => Ok(m.control_type),
            Err(e) => {
                warn!(partner = %source, error = %e, "remote control request does not decode; answered FAILED");
                Err(answer(RemoteControlResult::Failed, None))
            }
        }
    }

    /// The gate's decision: a first pass that asks for a refresh refreshes the device list and
    /// passes again, a second refresh answering `FAILED`. A failed refresh is logged where it
    /// fails; the refusal is logged once, after the second pass.
    async fn decide(&self, source: &DcgClient, control_type: i32) -> Gate {
        match self.gate(source, control_type) {
            Gate::Refresh => {
                self.device_list.refresh_device_list().await;

                if let Some(reason) = self.incapable(source, control_type) {
                    info!(partner = %source, control_type, reason, "remote control refused after a device-list refresh; answered FAILED");
                    return Gate::Answer(RemoteControlResult::Failed);
                }

                match self.gate(source, control_type) {
                    Gate::Refresh => Gate::Answer(RemoteControlResult::Failed),
                    other => other,
                }
            }
            other => other,
        }
    }
}

/// A lock and a sleep are answered `SUCCESS` before the Mac acts, so the answer leaves before the
/// session the lock stops or the sleep suspends; the other actions are answered with their outcome.
pub fn answered_first(action: PowerAction) -> bool {
    matches!(action, PowerAction::LockScreen | PowerAction::Sleep)
}

/// The follow-up of an action answered first: act once the answer was sent, and not at all when it
/// was not. The action's outcome is only logged ([`perform`] logs a failure); the phone already has
/// its answer.
async fn act_once_answered(
    power: Arc<dyn PcPowerControl>,
    source: DcgClient,
    action: PowerAction,
    delivery: AnswerDelivery,
) {
    match delivery {
        AnswerDelivery::Sent => {
            perform(power, action).await;
        }
        AnswerDelivery::NotSent => {
            warn!(partner = %source, ?action, "remote control answer not sent; the Mac does not act");
        }
    }
}

/// The power action `control_type` asks for, or the answer it gets without one.
fn power_action(source: &DcgClient, control_type: i32) -> Result<PowerAction, Vec<u8>> {
    match RemoteControlType::try_from(control_type) {
        Ok(RemoteControlType::Lock) => Ok(PowerAction::LockScreen),
        Ok(RemoteControlType::Sleep) => Ok(PowerAction::Sleep),
        // macOS has no "install updates while shutting down": the plain action.
        Ok(RemoteControlType::Shutdown | RemoteControlType::UpdateAndShutdown) => Ok(PowerAction::ShutDown),
        Ok(RemoteControlType::Restart | RemoteControlType::UpdateAndRestart) => Ok(PowerAction::Restart),
        // No update restart is ever pending on this Mac.
        Ok(RemoteControlType::QueryUpdateAvailability) => Err(answer(RemoteControlResult::Success, Some(false))),
        Ok(RemoteControlType::Unspecified | RemoteControlType::TriggerToggleOn) | Err(_) => {
            warn!(partner = %source, control_type, "remote control type unrecognized; answered FAILED");
            Err(answer(RemoteControlResult::Failed, None))
        }
    }
}

#[async_trait]
impl DrmFollowUpReceiver for RemoteControlReceiver {
    /// Answers every dispatched request, whatever its request type or query.
    async fn receive(&self, request: DrmRequest) -> Result<Option<DrmAnswer>, DrmReceiverError> {
        let Some(inner) = self.0.upgrade() else {
            return Err(DrmReceiverError("remote control is gone".into()));
        };
        Ok(Some(inner.handle_cdeh(&request).await))
    }
}

#[cfg(test)]
mod tests;
