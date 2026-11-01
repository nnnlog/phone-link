//! The files permission.

use super::{Cause, Inner, PermissionOrigin, Trigger, wire};
use phonelink_core::types::TraceContext;
use phonelink_features::shared::files_permission;
use phonelink_features::shared::files_permission::PermissionResult;
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use tracing::{error, info};

impl Inner {
    pub(super) async fn request_permission(&self, peer: &PeerDevice, origin: PermissionOrigin) -> PermissionResult {
        let trigger = match origin {
            PermissionOrigin::Dialog => Trigger::PermissionDialogClicked,
            PermissionOrigin::StatusUi => Trigger::PermissionStatusUiClicked,
        };

        let outcome = self
            .drm_send(
                peer,
                ProtoDeviceResourceType::Permissions,
                ProtoDeviceResourceRequestType::Sync,
                files_permission::request(),
                true,
                TraceContext::new_root(phonelink_core::types::trace_state(
                    wire::trace_name::REQUEST_PERMISSION,
                    trigger.wire(),
                )),
                self.shutdown.child_token(),
            )
            .await;

        // The result is the app's to log: it gets it.
        let result = PermissionResult::from_outcome(outcome);
        if result == PermissionResult::Granted {
            self.permission_granted(peer);
        }
        result
    }

    pub(super) fn permission_granted(&self, peer: &PeerDevice) {
        match self.update(&peer.client, |d| d.permission_granted = true) {
            Ok((before, _)) if !before.permission_granted => {
                info!(phone = %peer.client, "files permission granted");
                self.reconcile(peer, Cause::Permission);
                self.status_changed(&peer.client);
            }
            Ok(_) => {}
            Err(e) => error!(phone = %peer.client, error = %e, "files permission not stored; the feature waits for it"),
        }
    }
}
