//! The requests the PC sends the phone, the config sync and what follows its answer, and the context
//! handle the feature's channels are created against.

use super::{Inner, Status, TransferError, Trigger, request_trace, wire};
use phonelink_core::seams::PlatformRequestStatus;
use phonelink_core::types::{DcgClient, DcgSendStatus, TraceContext, WakeStatus};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{ContextHandle, DrmAddress, DrmSendOutcome, DrmSendRequest};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Why a config sync got no answer from the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfigSyncError {
    /// Not sent: the phone is not available and the user did not ask.
    NotAvailable,
    /// The request did not get a successful reply.
    Request(DrmSendOutcome),
}

/// What a config sync sent on its own does with the phone's answer besides logging it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConfigFollowUp {
    /// Nothing more.
    None,
    /// Records whether Link to Windows is off on the phone: the config sync sent when the feature starts
    /// for the phone.
    RecordLtw,
    /// Records it, and after a success refreshes the tree: the sync of a stale configuration after the
    /// phone's device-list entry changed.
    Reconnect,
}

/// The request failed because Link to Windows is off on the phone: the phone rejected it as
/// all-sync-disabled, or the wake of its send found Link to Windows disabled.
pub(super) fn ltw_disabled(outcome: &DrmSendOutcome) -> bool {
    matches!(
        outcome,
        DrmSendOutcome::Request(
            PlatformRequestStatus::RemoteAllSyncDisabled
                | PlatformRequestStatus::RequestSendFailed(DcgSendStatus::WakeFailed {
                    status: WakeStatus::LtwDisabled,
                    ..
                })
        )
    )
}

/// Ends when a new handle of the phone is stored. Once the phone's state is gone it never ends.
async fn handle_arrival(arrivals: &mut watch::Receiver<u64>) {
    if arrivals.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

impl Inner {
    /// One request through the CDEH request wrapper, addressed by type, bounded by the platform
    /// request timeout, with `trace`.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    pub(super) async fn drm_send(
        &self,
        peer: &PeerDevice,
        resource_type: ProtoDeviceResourceType,
        request_type: ProtoDeviceResourceRequestType,
        payload: Vec<u8>,
        force_session_start: bool,
        trace: TraceContext,
        cancel: CancellationToken,
    ) -> DrmSendOutcome {
        self.drm
            .send(DrmSendRequest {
                target: peer.client.clone(),
                address: DrmAddress::Type(resource_type),
                request_type,
                payload,
                client_version: peer.info.client_version.clone(),
                phone_enabled: self.phone_enabled(&peer.client),
                force_session_start,
                timeout: self.env.config.platform.request_timeout,
                trace,
                cancel,
            })
            .await
    }

    /// A `FILES_METADATA` request.
    pub(super) async fn files_request(
        &self,
        peer: &PeerDevice,
        request_type: ProtoDeviceResourceRequestType,
        payload: Vec<u8>,
        force_session_start: bool,
        trace: TraceContext,
        cancel: CancellationToken,
    ) -> DrmSendOutcome {
        self.drm_send(
            peer,
            ProtoDeviceResourceType::FilesMetadata,
            request_type,
            payload,
            force_session_start,
            trace,
            cancel,
        )
        .await
    }

    /// The config sync: the phone publishes its configuration (and its context handle) on tag 4. Sent
    /// within `operation` (sharing its trace) when an operation needs the handle, on its own otherwise.
    /// The status the phone answered.
    pub(super) async fn config_sync(
        &self,
        peer: &PeerDevice,
        user: bool,
        trigger: Trigger,
        operation: Option<&TraceContext>,
        cancel: CancellationToken,
    ) -> Result<Status, ConfigSyncError> {
        let phone = &peer.client;
        if !user && !self.available(phone) {
            return Err(ConfigSyncError::NotAvailable);
        }

        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Sync,
                wire::config_sync_request(),
                user,
                request_trace(
                    operation,
                    phonelink_core::types::trace_state(wire::trace_name::CONFIG_REQUEST, trigger.wire()),
                ),
                cancel,
            )
            .await;

        match outcome {
            DrmSendOutcome::Success { payload } => Ok(wire::config_sync_status(&payload)),
            other => Err(ConfigSyncError::Request(other)),
        }
    }

    /// A config sync on its own; the log shows its answer, and `follow_up` says what else it does.
    pub(super) fn spawn_config_sync(&self, peer: PeerDevice, user: bool, trigger: Trigger, follow_up: ConfigFollowUp) {
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let result = inner.config_sync(&peer, user, trigger, None, inner.shutdown.child_token()).await;
            inner.config_sync_answered(&peer, trigger, &result, follow_up);
        });
    }

    fn config_sync_answered(
        &self,
        peer: &PeerDevice,
        trigger: Trigger,
        result: &Result<Status, ConfigSyncError>,
        follow_up: ConfigFollowUp,
    ) {
        let (phone, trigger_name) = (&peer.client, trigger.wire());
        match result {
            Ok(status) => info!(%phone, trigger = trigger_name, ?status, "config sync answered"),
            Err(ConfigSyncError::NotAvailable) => {
                debug!(%phone, trigger = trigger_name, "config sync not sent; the phone is not available");
            }
            Err(ConfigSyncError::Request(outcome)) => {
                warn!(%phone, trigger = trigger_name, ?outcome, "config sync failed");
            }
        }

        if follow_up == ConfigFollowUp::None {
            return;
        }

        match result {
            Ok(Status::Success) => self.set_ltw_off(phone, false),
            Err(ConfigSyncError::Request(outcome)) if ltw_disabled(outcome) => self.set_ltw_off(phone, true),
            _ => {}
        }

        if follow_up == ConfigFollowUp::Reconnect && *result == Ok(Status::Success) {
            self.reconnected(peer);
        }
    }

    /// The phone answered the sync of its stale configuration: a phone that was synced and holds the
    /// permission gets its changes since.
    fn reconnected(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        let data = match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => d,
            Err(e) => {
                return warn!(%phone, error = %e, "phone-files state unreadable; no refresh after the reconnection");
            }
        };

        if self.phone_enabled(phone) && data.initial_sync_done && data.permission_granted {
            self.spawn_refresh(peer.clone(), false, Trigger::DeviceReconnectedSync);
        }
    }

    /// The stored handle of `phone`, unless the platform layer dropped it.
    pub(super) fn stored_handle(&self, phone: &DcgClient) -> Option<ContextHandle> {
        self.with_phone(phone, |p| match &p.handle {
            Some(h) if !h.is_dropped() => Some(h.clone()),
            Some(_) => {
                p.handle = None;
                None
            }
            None => None,
        })
    }

    /// The stored handle while the phone is available.
    fn handle_while_available(&self, phone: &DcgClient) -> Option<ContextHandle> {
        if self.available(phone) { self.stored_handle(phone) } else { None }
    }

    /// The phone's context handle: the stored one while the phone is available; else the config sync
    /// (within `operation`) races a new handle's arrival, and after its answer a stored handle counts
    /// only when a new one arrived or the phone is available.
    pub(super) async fn ensure_handle(
        &self,
        peer: &PeerDevice,
        user: bool,
        trigger: Trigger,
        operation: &TraceContext,
    ) -> Result<ContextHandle, TransferError> {
        let phone = &peer.client;
        if let Some(h) = self.handle_while_available(phone) {
            return Ok(h);
        }

        // Watching before the second look, so a handle stored in between is seen either way.
        let mut arrivals = self.with_phone(phone, |p| p.handle_arrivals.subscribe());
        if let Some(h) = self.handle_while_available(phone) {
            return Ok(h);
        }

        // The sync runs as a task of its own, so that it can run on to its answer after a new handle
        // came first; otherwise it ends with the operation.
        let cancel = self.shutdown.child_token();
        let cancel_with_operation = cancel.clone().drop_guard();
        let inner = self.weak.upgrade().expect("the feature's operations run while it is installed");
        let (task_peer, task_operation) = (peer.clone(), operation.clone());
        let mut config = self
            .runtime
            .spawn(async move { inner.config_sync(&task_peer, user, trigger, Some(&task_operation), cancel).await });

        let mut arrived = false;
        let joined = tokio::select! {
            joined = &mut config => joined,
            () = handle_arrival(&mut arrivals) => {
                arrived = true;
                if let Some(h) = self.stored_handle(phone) {
                    cancel_with_operation.disarm();
                    return Ok(h);
                }
                (&mut config).await
            }
        };

        let result = match joined {
            Ok(r) => r,
            Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Err(_) => Err(ConfigSyncError::Request(DrmSendOutcome::Canceled)),
        };

        arrived |= arrivals.has_changed().unwrap_or(false);
        if arrived || self.available(phone) {
            if let Some(h) = self.stored_handle(phone) {
                return Ok(h);
            }
        }

        // The caller gets no handle whatever the phone answered; the answer says why.
        debug!(%phone, trigger = trigger.wire(), outcome = ?result, "no context handle after the config sync");
        Err(TransferError::NoContextHandle)
    }

    /// Forgets `phone`'s handle (the phone said it is no longer valid).
    pub(super) fn forget_handle(&self, phone: &DcgClient) {
        self.with_phone(phone, |p| p.handle = None);
    }
}

#[cfg(test)]
mod tests;
