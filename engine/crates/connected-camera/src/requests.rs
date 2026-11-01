//! The requests the PC sends the phone.

use super::{Inner, wire};
use phonelink_core::types::TraceContext;
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{DrmAddress, DrmSendOutcome, DrmSendRequest};

impl Inner {
    /// One forced-session-start-free request through the CDEH request wrapper, addressed by type.
    pub(super) async fn drm_send(
        &self,
        peer: &PeerDevice,
        resource_type: ProtoDeviceResourceType,
        payload: Vec<u8>,
        trigger: &str,
    ) -> DrmSendOutcome {
        self.drm_request(peer, resource_type, ProtoDeviceResourceRequestType::Sync, payload, true, trigger).await
    }

    pub(super) async fn drm_request(
        &self,
        peer: &PeerDevice,
        resource_type: ProtoDeviceResourceType,
        request_type: ProtoDeviceResourceRequestType,
        payload: Vec<u8>,
        force_session_start: bool,
        trigger: &str,
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
                trace: TraceContext::new_root(phonelink_core::types::trace_state(wire::trace_name::SCENARIO, trigger)),
                cancel: self.shutdown.child_token(),
            })
            .await
    }
}
