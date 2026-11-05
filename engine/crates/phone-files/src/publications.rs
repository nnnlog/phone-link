//! The phone's publications: its configuration and changes.

use super::{Cause, Inner, Trigger, key, wire};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::files_permission;
use phonelink_proto::phone_files::v1::{
    FilesConfigurationPayload, FilesSyncType, FilesSyncUpdateMessage, StorageInfoPayload, files_response_payload,
};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::Publication;
use tracing::{debug, error, info, warn};

impl Inner {
    /// Tag 4. Runs on the router's task: stores the delivered handle first, then reads the payload.
    pub(super) fn on_files(&self, publication: &Publication) {
        let source = &publication.source;
        self.adopt_delivered_handle(source);

        let Some(peer) = self.env.peers.peer(source) else {
            debug!(partner = %source, "files publication from a device not in the account; ignored");
            return;
        };

        let payload = match wire::parse_publication(&publication.payload.additional) {
            Ok(p) => p,
            Err(e) => return warn!(partner = %source, error = %e, "files publication does not parse; dropped"),
        };

        match payload.payload {
            Some(files_response_payload::Payload::FileConfiguration(config)) => {
                if peer.info.is_android() {
                    self.on_config(&peer, config);
                }
            }
            Some(files_response_payload::Payload::FileSyncUpdate(update)) => self.on_sync_update(source, peer, &update),
            Some(files_response_payload::Payload::StorageInfo(storage)) => self.on_storage(source, &storage),
            None => warn!(partner = %source, "files publication with an unknown payload; ignored"),
        }
    }

    /// Stores the context handle the publication from `source` delivered, when it is a new one, and
    /// tells a handle request waiting for one.
    fn adopt_delivered_handle(&self, source: &DcgClient) {
        match self.validator.current_handle(source) {
            None => warn!(partner = %source, "files publication without a context source; no handle"),
            Some(handle) => {
                let replaced = self.with_phone(source, |p| match &p.handle {
                    Some(h) if h.same_source(&handle) && !h.is_dropped() => false,
                    _ => {
                        p.handle = Some(handle);
                        p.handle_arrivals.send_modify(|n| *n += 1);
                        true
                    }
                });
                if replaced {
                    info!(partner = %source, "context handle updated");
                }
            }
        }
    }

    /// The phone's sync update: an incremental sync of a phone with a tree.
    fn on_sync_update(&self, source: &DcgClient, peer: PeerDevice, update: &FilesSyncUpdateMessage) {
        if !self.with_phone(source, |p| p.running) {
            return debug!(partner = %source, "sync update for a phone without a tree; ignored");
        }

        if update.sync_type == FilesSyncType::IncrementalSync as i32 {
            let weak = self.weak.clone();
            let seq_num = update.seq_num;
            self.runtime.spawn(async move {
                if let Some(inner) = weak.upgrade() {
                    inner.incremental_sync(&peer, Some(seq_num), false, Trigger::FilesSyncPublished).await;
                }
            });
        } else {
            warn!(partner = %source, sync_type = update.sync_type, "sync update of another type; dropped");
        }
    }

    /// The phone's storage sizes, for a phone with a tree.
    fn on_storage(&self, source: &DcgClient, storage: &StorageInfoPayload) {
        if !self.with_phone(source, |p| p.running) {
            return debug!(partner = %source, "storage for a phone without a tree; ignored");
        }

        match self.update(source, |d| {
            d.storage_free = storage.available_size_bytes;
            d.storage_total = storage.total_size_bytes;
        }) {
            Ok(_) => self.status_changed(source),
            Err(e) => error!(partner = %source, error = %e, "storage sizes not stored"),
        }
    }

    pub(super) fn on_config(&self, peer: &PeerDevice, config: FilesConfigurationPayload) {
        let phone = &peer.client;
        let prefetch = !config.thumbnail_prefetch_config.is_empty();
        let others_enabled = self
            .env
            .peers
            .peers()
            .iter()
            .filter(|p| key(&p.client) != key(phone))
            .any(|p| self.store(&p.client).and_then(|s| s.data()).is_ok_and(|d| d.enabled));

        let result = self.update(phone, |d| {
            let became_capable = !d.capable && config.capability;
            d.capable = config.capability;
            d.permission_granted = config.permission;

            let mut stored = config.clone();
            if stored.thumbnail_prefetch_config.is_empty() {
                if let Some(old) = &d.config {
                    stored.thumbnail_prefetch_config = old.thumbnail_prefetch_config.clone();
                }
            }
            d.config = Some(stored);

            if became_capable && !d.enabled && !others_enabled {
                d.enabled = true;
            }
        });

        let (before, after) = match result {
            Ok(r) => r,
            Err(e) => {
                return error!(%phone, error = %e, "configuration not stored; the feature does not start or stop for it");
            }
        };

        let first = self.with_phone(phone, |p| !std::mem::replace(&mut p.config_up_to_date, true));
        info!(
            %phone,
            capable = after.capable,
            has_permission = after.permission_granted,
            enabled = after.enabled,
            "configuration received"
        );

        let running = self.reconcile(
            peer,
            if first || before.permission_granted != after.permission_granted {
                Cause::Config
            } else {
                Cause::Registered
            },
        );
        self.status_changed(phone);
        self.gate.notify_waiters();

        if running && prefetch && after.permission_granted && after.initial_sync_done {
            let (weak, peer) = (self.weak.clone(), peer.clone());
            self.runtime.spawn(async move {
                if let Some(inner) = weak.upgrade() {
                    inner.prefetch(&peer).await;
                }
            });
        }
    }

    /// Tag 3: a grant of `FILES` sets the permission (never clears it).
    pub(super) fn on_permissions(&self, publication: &Publication) {
        let source = &publication.source;
        let Some(peer) = self.env.peers.peer(source) else { return };
        match wire::parse_permissions(&publication.payload.additional) {
            Ok(p) if files_permission::grants_files(&p) => self.permission_granted(&peer),
            Ok(_) => {}
            Err(e) => warn!(partner = %source, error = %e, "permission publication does not parse; dropped"),
        }
    }
}
