//! The phone's context handle: obtaining it and the channels over it.

use super::{HandleFailure, HandleSlot, Inner, RetrieveError, wire};
use phonelink_core::seams::PlatformRequestStatus;
use phonelink_core::types::DcgClient;
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{ContextHandle, DrmSendOutcome, Publication};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

impl Inner {
    /// The camera metadata sync: the phone publishes tag 2. At most one per phone in flight.
    pub(super) fn send_sync(&self, peer: &PeerDevice, force_session_start: bool) {
        let phone = &peer.client;
        let generation = self.with_phone(phone, |p| {
            if p.sync_in_flight {
                return None;
            }
            p.sync_in_flight = true;
            p.handle_gen += 1;
            p.handle = HandleSlot::Pending;
            Some(p.handle_gen)
        });
        let Some(generation) = generation else {
            debug!(%phone, "camera metadata sync already in flight");
            return;
        };

        self.handle_changed.send_modify(|v| *v += 1);
        let (weak, peer) = (self.weak.clone(), peer.clone());
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let outcome = inner
                .drm_request(
                    &peer,
                    ProtoDeviceResourceType::CameraMetadata,
                    ProtoDeviceResourceRequestType::Sync,
                    Vec::new(),
                    force_session_start,
                    wire::trace_name::SYNC,
                )
                .await;

            let failure = match &outcome {
                DrmSendOutcome::Success { .. } => None,
                DrmSendOutcome::Request(PlatformRequestStatus::RemoteMeteredSyncDisabled) => {
                    Some(HandleFailure::Metered)
                }
                DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled) => {
                    Some(HandleFailure::AllSyncDisabled)
                }
                DrmSendOutcome::Request(PlatformRequestStatus::RemoteSyncNotAllowed)
                | DrmSendOutcome::Locked
                | DrmSendOutcome::UserDisabled
                | DrmSendOutcome::Canceled => Some(HandleFailure::ExpectedNoHandle),
                _ => Some(HandleFailure::RequestFailed),
            };

            match failure {
                None => debug!(phone = %peer.client, "camera metadata sync answered"),
                Some(HandleFailure::RequestFailed) => {
                    warn!(phone = %peer.client, ?outcome, "camera metadata sync failed; no context handle");
                }
                Some(_) => info!(phone = %peer.client, ?outcome, "camera metadata sync left no context handle"),
            }

            if let Some(failure) = failure {
                inner.complete_handle(&peer.client, generation, Err(failure));
            }
        });
    }

    /// Completes the pending result of sync `generation`; a result already completed or replaced stays.
    pub(super) fn complete_handle(
        &self,
        phone: &DcgClient,
        generation: u64,
        result: Result<ContextHandle, HandleFailure>,
    ) -> bool {
        let done = self.with_phone(phone, |p| {
            if p.handle_gen != generation || !matches!(p.handle, HandleSlot::Pending) {
                return false;
            }
            p.handle = HandleSlot::Done(result);
            p.sync_in_flight = false;
            true
        });

        if done {
            self.handle_changed.send_modify(|v| *v += 1);
        }
        done
    }

    /// Discards `phone`'s handle result (only result `generation` when given): a waiter sees it invalidated,
    /// and the next stream sends a new sync.
    pub(super) fn invalidate_handle(&self, phone: &DcgClient, generation: Option<u64>) {
        let done = self.with_phone(phone, |p| {
            if generation.is_some_and(|g| g != p.handle_gen) {
                return false;
            }
            p.handle_gen += 1;
            p.handle = HandleSlot::Discarded;
            p.sync_in_flight = false;
            true
        });

        if done {
            debug!(%phone, "camera context handle invalidated");
            self.handle_changed.send_modify(|v| *v += 1);
        }
    }

    /// The phone's context handle for a stream.
    pub(super) async fn retrieve_handle(
        &self,
        peer: &PeerDevice,
        cancel: &CancellationToken,
    ) -> Result<ContextHandle, RetrieveError> {
        let phone = &peer.client;
        let mut changes = self.handle_changed.subscribe();

        let usable = self.with_phone(phone, |p| match &p.handle {
            HandleSlot::Done(Ok(h)) if !h.is_dropped() => Some(Some(h.clone())),
            HandleSlot::Pending => Some(None),
            _ => None,
        });
        match usable {
            Some(Some(h)) => return Ok(h),
            Some(None) => {}
            None => self.send_sync(peer, true),
        }

        let generation = self.with_phone(phone, |p| p.handle_gen);
        let wait = async {
            loop {
                let slot = self.with_phone(phone, |p| (p.handle_gen, p.handle.clone()));
                match slot {
                    (g, _) if g != generation => return Err(RetrieveError::Invalidated),
                    (_, HandleSlot::Done(Ok(h))) => return Ok(h),
                    (_, HandleSlot::Done(Err(f))) => return Err(RetrieveError::Failed(f)),
                    (_, HandleSlot::Discarded | HandleSlot::Empty) => return Err(RetrieveError::Invalidated),
                    (_, HandleSlot::Pending) => {}
                }
                if changes.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        };

        let result = tokio::select! {
            r = wait => r,
            _ = cancel.cancelled() => return Err(RetrieveError::Cancelled),
            _ = tokio::time::sleep(self.config.handle_timeout) => {
                self.send_sync(peer, true);
                return Err(RetrieveError::TimedOut);
            }
        };

        if let Err(RetrieveError::Failed(_)) = result {
            self.send_sync(peer, true);
        }
        result
    }

    /// Tag 2. Runs on the router's task.
    pub(super) fn on_camera(&self, publication: &Publication) {
        if !self.camera.is_available() {
            return;
        }

        let source = &publication.source;
        let Some(peer) = self.env.peers.peer(source) else {
            debug!(partner = %source, "camera publication from a device not in the account; ignored");
            return;
        };

        let parsed = match wire::camera_publication(&publication.payload.data) {
            Ok(p) => p,
            Err(e) => {
                warn!(partner = %source, error = %e, "camera publication does not parse; dropped");
                return;
            }
        };

        info!(
            partner = %source,
            is_capable = parsed.capable,
            has_camera_permission = parsed.camera_permission,
            has_notification_permission = parsed.notification_permission,
            "camera publication received"
        );

        self.update_logged(source, |d| {
            d.capable = parsed.capable;
            d.camera_permission = parsed.camera_permission;
            d.notification_permission = parsed.notification_permission;
        });
        self.apply_default_enable(source, &peer);
        if self.with_phone(source, |p| p.listening) {
            self.register_if_needed(&peer);
        }

        self.adopt_context_handle(source);
        self.changed(source);
    }

    /// Default enable: on the first refresh of a supported phone, enables its camera unless another
    /// phone already would register.
    fn apply_default_enable(&self, source: &DcgClient, peer: &PeerDevice) {
        let data = self.data_or_default(source);
        if self.supported(peer, &data) && data.initial_refresh_pending {
            let none_should = self.env.peers.peers().iter().all(|other| {
                let d = self.data_or_default(&other.client);
                !self.should_register(other, &d)
            });
            let enable = self.config.multiple_cameras || none_should;
            self.update_logged(source, |d| {
                d.initial_refresh_pending = false;
                if enable {
                    d.camera_enabled = true;
                }
            });
        }
    }

    /// Completes the pending handle with the phone's current context handle and watches it for drops.
    fn adopt_context_handle(&self, source: &DcgClient) {
        match self.validator.current_handle(source) {
            None => warn!(partner = %source, "camera publication without a context source; no handle"),
            Some(handle) => {
                let generation = self.with_phone(source, |p| p.handle_gen);
                if self.complete_handle(source, generation, Ok(handle.clone())) {
                    info!(partner = %source, "camera context handle obtained");
                    let (weak, phone, dropped) = (self.weak.clone(), source.clone(), handle.dropped());
                    self.runtime.spawn(async move {
                        dropped.cancelled().await;
                        if let Some(inner) = weak.upgrade() {
                            inner.invalidate_handle(&phone, Some(generation));
                        }
                    });
                }
            }
        }
    }
}
