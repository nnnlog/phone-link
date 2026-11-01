//! The requests the PC sends the phone.

use super::{Inner, Status, TransferError, Trigger, request_trace, wire};
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{ContextHandle, DrmAddress, DrmSendOutcome, DrmSendRequest};
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
                self.shutdown.child_token(),
            )
            .await;

        match outcome {
            DrmSendOutcome::Success { payload } => Ok(wire::config_sync_status(&payload)),
            other => Err(ConfigSyncError::Request(other)),
        }
    }

    /// A config sync on its own, whose answer only the log shows.
    pub(super) fn spawn_config_sync(&self, peer: PeerDevice, user: bool, trigger: Trigger) {
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let (phone, trigger_name) = (&peer.client, trigger.wire());
            match inner.config_sync(&peer, user, trigger, None).await {
                Ok(status) => info!(%phone, trigger = trigger_name, ?status, "config sync answered"),
                Err(ConfigSyncError::NotAvailable) => {
                    debug!(%phone, trigger = trigger_name, "config sync not sent; the phone is not available");
                }
                Err(ConfigSyncError::Request(outcome)) => {
                    warn!(%phone, trigger = trigger_name, ?outcome, "config sync failed");
                }
            }
        });
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

    /// The phone's context handle: the stored one while the phone is available, else whatever a config
    /// sync (within `operation`) left stored (the phone publishes before it answers).
    pub(super) async fn ensure_handle(
        &self,
        peer: &PeerDevice,
        user: bool,
        trigger: Trigger,
        operation: &TraceContext,
    ) -> Result<ContextHandle, TransferError> {
        let phone = &peer.client;
        if self.available(phone) {
            if let Some(h) = self.stored_handle(phone) {
                return Ok(h);
            }
        }

        match self.config_sync(peer, user, trigger, Some(operation)).await {
            Ok(Status::Success) => self.stored_handle(phone).ok_or(TransferError::NoContextHandle),
            Ok(other) => Err(TransferError::Status(other)),
            Err(ConfigSyncError::NotAvailable) => Err(TransferError::Status(Status::NotFound)),
            Err(ConfigSyncError::Request(outcome)) => Err(TransferError::Request(outcome)),
        }
    }

    /// Forgets `phone`'s handle (the phone said it is no longer valid).
    pub(super) fn forget_handle(&self, phone: &DcgClient) {
        self.with_phone(phone, |p| p.handle = None);
    }
}
