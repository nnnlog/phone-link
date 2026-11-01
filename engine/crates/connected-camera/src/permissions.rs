//! The camera and notification permissions.

use super::{CameraError, CameraState, Grant, Inner, wire};
use phonelink_core::types::DcgClient;
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{DrmSendOutcome, Publication};
use std::sync::Arc;
use tracing::{error, info, warn};

impl Inner {
    /// Tag 3. Runs on the router's task.
    pub(super) fn on_permissions(&self, publication: &Publication) {
        if !self.camera.is_available() {
            return;
        }

        let source = &publication.source;
        let Some(peer) = self.env.peers.peer(source) else { return };
        let grant = match wire::permission_publication(&publication.payload.additional) {
            Ok(g) => g,
            Err(e) => {
                warn!(partner = %source, error = %e, "permission publication does not parse; dropped");
                return;
            }
        };

        if grant == Grant::Nothing {
            return;
        }

        info!(partner = %source, ?grant, "camera permission publication received");
        let had_camera_permission = self.data_or_default(source).camera_permission;
        self.update_logged(source, |d| d.notification_permission = true);
        if grant == Grant::Camera {
            self.on_camera_granted(source, &peer, had_camera_permission);
        }
        self.changed(source);
    }

    /// The phone granted the camera: enables it, and re-requests or refreshes as the phone's state needs.
    fn on_camera_granted(&self, source: &DcgClient, peer: &PeerDevice, had_camera_permission: bool) {
        if let Err(e) = self.set_camera_enabled(peer, true) {
            error!(partner = %source, error = %e, "camera switch not saved");
        }
        if had_camera_permission && self.with_phone(source, |p| p.registered) {
            self.weak.upgrade().expect("alive").request_permissions_if_needed(peer.clone());
        }
        if self.with_phone(source, |p| p.permission_requested) {
            self.update_logged(source, |d| d.camera_permission = true);
            self.refresh(peer);
        }
    }

    /// The permission request when a permission is missing; otherwise a refresh.
    pub(super) fn request_permissions_if_needed(self: Arc<Self>, peer: PeerDevice) {
        let data = self.data_or_default(&peer.client);
        if !self.permission_required(&peer, &data) {
            self.refresh(&peer);
            return;
        }
        self.runtime.clone().spawn(async move { self.permission_request(&peer).await });
    }

    /// The camera permission request (the phone prompts).
    pub(super) async fn permission_request(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        self.with_phone(phone, |p| p.permission_requested = true);
        let outcome = self
            .drm_send(
                peer,
                ProtoDeviceResourceType::Permissions,
                wire::permission_request(),
                wire::trace_name::PERMISSION_REQUEST,
            )
            .await;

        let Some(reply) = logged_permission_reply(phone, &outcome) else { return };
        self.update_logged(phone, |d| {
            d.notification_permission = reply.notifications;
            d.camera_permission = reply.camera;
        });

        if !reply.notifications {
            if let Some(qr) = reply.qr_code_uri {
                self.with_phone(phone, |p| p.qr_code_uri = Some(qr));
                self.set_state(phone, CameraState::Error, CameraError::PostNotificationPermissionNeeded);
            }
        } else if !reply.camera {
            if self.config.auto_prompt {
                let error = if reply.camera_permanently_denied {
                    CameraError::CameraPermissionPermanentlyDenied
                } else {
                    CameraError::CameraPermissionNeededAutoprompt
                };
                self.set_state(phone, CameraState::Error, error);
            }
        } else {
            self.refresh(peer);
        }
        self.changed(phone);
    }
}

/// The phone's reply in `outcome`, logged; `None` when the request failed or the reply does not parse.
fn logged_permission_reply(phone: &DcgClient, outcome: &DrmSendOutcome) -> Option<wire::PermissionReply> {
    let DrmSendOutcome::Success { payload } = outcome else {
        warn!(%phone, ?outcome, "camera permission request failed; the permissions stay as stored");
        return None;
    };
    let Some(reply) = wire::permission_reply(payload) else {
        warn!(%phone, "camera permission reply does not parse; the permissions stay as stored");
        return None;
    };
    info!(
        %phone,
        has_camera_permission = reply.camera,
        has_notification_permission = reply.notifications,
        is_camera_permanently_denied = reply.camera_permanently_denied,
        "camera permission request answered"
    );
    Some(reply)
}
