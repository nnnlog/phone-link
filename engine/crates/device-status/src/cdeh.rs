//! The Cross Device Experience Host (CDEH) contract:
//! the status pull `GET /devicestatus`, the MSAEP tag-8 push subscriber and the
//! `/remotedevicestatus` receiver that answers the PC's own status.
//!
//! The set (`UPDATE /devicestatus`)
//! and DND permission requests are part of the contract but never sent, as on Windows — nothing here
//! builds them.

use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use prost::Message;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::Inner;
use super::edge::{PcStatusSource, pc_status_response, unknown_error_response};
use super::model::{Received, decode_received};
use phonelink_core::types::TraceContext;
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{
    DrmAddress, DrmReceiver, DrmReceiverError, DrmRequest, DrmSendRequest, Publication, PublicationObserver,
};

/// The resource path of the pull.
pub const STATUS_PATH: &str = "/devicestatus";
/// The receiver path of the PC's status.
pub const REMOTE_STATUS_PATH: &str = "/remotedevicestatus";
/// The MSAEP tag of the push.
pub const STATUS_TAG: i16 = 8;
/// The phone capability the pull requires (exact string).
pub const DEVICE_STATUS_CAPABILITY: &str = "DEVICE_STATUS";

/// The phone's device-list entry has `DEVICE_STATUS`.
pub fn has_device_status(peer: &PeerDevice) -> bool {
    peer.info.capabilities.as_ref().is_some_and(|c| c.iter().any(|s| s == DEVICE_STATUS_CAPABILITY))
}

/// The pull through CDEH's request wrapper: path-addressed (`UNKNOWN`), `GET`, an empty payload, no
/// forced session start, bounded by the platform request timeout.
pub fn pull_request(
    peer: &PeerDevice,
    phone_enabled: bool,
    timeout: Duration,
    cancel: CancellationToken,
) -> DrmSendRequest {
    DrmSendRequest {
        target: peer.client.clone(),
        address: DrmAddress::Path(STATUS_PATH.to_owned()),
        request_type: ProtoDeviceResourceRequestType::Get,
        payload: Vec::new(),
        client_version: peer.info.client_version.clone(),
        phone_enabled,
        force_session_start: false,
        timeout,
        trace: TraceContext::new_root(Vec::new()),
        cancel,
    }
}

/// The tag-8 subscriber: decodes `PubSubPayload.additional`, ignores a decode failure or a status
/// other than `OK`, and replaces the status of the phone the publication arrived from — the partner
/// of its platform connection (`MsaepMessage.dcg_client_id` is not read) — when it is one of the
/// account's phones. Runs on the router's task: it only updates memory and spawns the rest.
pub(crate) struct StatusPushObserver(pub(crate) Weak<Inner>);

impl PublicationObserver for StatusPushObserver {
    fn on_publication(&self, publication: &Publication) {
        let Some(inner) = self.0.upgrade() else { return };
        let status = match decode_received(&publication.payload.additional) {
            Ok(Received::Ok(status)) => status,
            Ok(Received::NotOk(status)) => {
                debug!(partner = %publication.source, status, "device-status push not OK; ignored");
                return;
            }
            Err(e) => {
                warn!(partner = %publication.source, error = %e, "device-status push does not decode; ignored");
                return;
            }
        };

        let Some(peer) = inner.env.peers.peer(&publication.source) else {
            warn!(partner = %publication.source, "device-status push from a device not in the account's list; ignored");
            return;
        };
        inner.set_cdeh_status(&peer.client, status);
    }
}

/// The `/remotedevicestatus` receiver: answers every dispatched request — whatever its request
/// type, query or payload, none of which is parsed — with the PC's status.
pub(crate) struct RemoteStatusReceiver {
    source: Arc<dyn PcStatusSource>,
}

impl RemoteStatusReceiver {
    pub(crate) fn new(source: Arc<dyn PcStatusSource>) -> Self {
        Self { source }
    }
}

#[async_trait]
impl DrmReceiver for RemoteStatusReceiver {
    async fn receive(&self, request: DrmRequest) -> Result<Option<Vec<u8>>, DrmReceiverError> {
        debug!(partner = %request.source, "PC status requested");
        let source = self.source.clone();
        let answer = match tokio::task::spawn_blocking(move || pc_status_response(&*source)).await {
            Ok(answer) => answer,
            Err(e) => {
                warn!(error = %e, "PC status read panicked; answered UNKNOWN_ERROR");
                unknown_error_response()
            }
        };
        Ok(Some(answer.encode_to_vec()))
    }
}
