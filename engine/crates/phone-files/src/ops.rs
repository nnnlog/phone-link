//! Changes the Mac makes to the phone's tree over `/DeviceResourceManager`: a new folder, a rename or
//! move inside the tree, and deletes — collected into batches when the phone takes them, one request
//! per item otherwise.

use std::sync::Arc;

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use super::path;
use super::path::Rejected;
use super::store::{Entry, Row};
use super::sync::to_row;
use super::tree::Item;
use super::wire::{self, Status};
use super::{Inner, PhoneFilesError, PhoneFilesEvent};
use phonelink_core::types::TraceContext;
use phonelink_features::shared::db::StoreError;
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::DrmSendOutcome;

/// What the phone answered to a change instead of a success: a status, or no reply at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpFailure {
    #[error("{0:?}")]
    Status(Status),
    /// The `/DeviceResourceManager` request did not get a successful reply.
    #[error("the request failed: {0:?}")]
    Request(DrmSendOutcome),
}

/// Why a new folder, a rename or a move did not happen.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("phone files is not running for this phone")]
    NotRunning,
    #[error("the item is not in the tree")]
    UnknownItem,
    #[error("the folder is not in the tree")]
    UnknownFolder,
    #[error("an item with this name already exists")]
    Exists,
    /// The name stays on the Mac: it is not sent to the phone.
    #[error("items with this name are not sent to the phone")]
    Ignored,
    #[error("the phone does not support this")]
    NotSupported,
    /// The Mac's tree is behind the phone; a refresh corrects it.
    #[error("the item changed on the phone")]
    NotInSync,
    /// The phone answered with this status, or did not answer.
    #[error("the operation failed: {0}")]
    Phone(#[source] OpFailure),
    #[error("the operation failed: {0}")]
    Store(#[from] StoreError),
    #[error("the operation failed: {0}")]
    Tree(#[from] PhoneFilesError),
    /// The phone's metadata names an item the tree cannot hold.
    #[error("the operation failed: {0}")]
    Item(#[from] Rejected),
}

/// Why a delete did not (fully) happen.
#[derive(Debug, thiserror::Error)]
pub enum DeleteError {
    #[error("phone files is not running for this phone")]
    NotRunning,
    #[error("the item is not in the tree")]
    UnknownItem,
    #[error("the phone does not support this")]
    NotSupported,
    #[error("the item changed on the phone")]
    NotInSync,
    /// Some of the items (`failed` of `total`) were not deleted; they stay. `first` is the first
    /// failure.
    #[error("{failed} of {total} items could not be deleted: {first}")]
    Partial { failed: usize, total: usize, first: OpFailure },
    /// The phone answered with this status, or did not answer.
    #[error("the delete failed: {0}")]
    Phone(#[source] OpFailure),
    #[error("the delete failed: {0}")]
    Store(#[from] StoreError),
}

/// One item waiting in the delete batch.
struct PendingDelete {
    row: Row,
    reply: oneshot::Sender<Result<(), OpFailure>>,
}

/// The items collected for the next delete batch and the timer that sends it.
#[derive(Default)]
pub(super) struct DeleteQueue {
    pending: Vec<PendingDelete>,
    timer: Option<CancellationToken>,
}

/// The result of an item's single delete request.
fn single_result(status: Status) -> Result<(), OpFailure> {
    match status {
        Status::Success | Status::NotFound => Ok(()),
        other => Err(OpFailure::Status(other)),
    }
}

impl Inner {
    pub(super) async fn create_folder(&self, peer: &PeerDevice, parent: &str, name: &str) -> Result<Item, OpError> {
        let phone = &peer.client;
        if path::ignored(name) {
            return Err(OpError::Ignored);
        }

        let store = self.running_store(phone).map_err(|_| OpError::NotRunning)?;
        let folder = self.folder(&store, parent).map_err(OpError::from)?.ok_or(OpError::UnknownFolder)?;
        let location = path::join(&folder.location, name);

        if let Some(existing) = store.row_at(&location).map_err(OpError::from)? {
            if existing.is_directory {
                return self.item_of_row(&store, &existing, &self.phone_config(phone)).map_err(OpError::from);
            }
            return Err(OpError::Exists);
        }

        let (uri, checksum) = folder.row.as_ref().map(|r| (r.uri.as_str(), r.checksum)).unwrap_or(("", 0));
        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Update,
                wire::create_folder_request(uri, checksum, name),
                false,
                TraceContext::new_root(phonelink_core::types::scenario_trace_state(wire::trace_name::CREATE_DIRECTORY)),
                self.shutdown.child_token(),
            )
            .await;

        let reply = match outcome {
            DrmSendOutcome::Success { payload } => wire::update_response(&payload).map_err(OpFailure::Status),
            other => Err(OpFailure::Request(other)),
        };
        let reply = reply.map_err(OpError::Phone)?;
        let metadata = match Status::from_wire(reply.status) {
            Status::Success => reply.target_metadata.ok_or(OpError::Phone(OpFailure::Status(Status::IoError)))?,
            other => return Err(OpError::Phone(OpFailure::Status(other))),
        };

        let row = to_row(&metadata).map_err(OpError::from)?;
        store.apply(std::slice::from_ref(&row), &[], None).map_err(OpError::from)?;
        self.items_changed(phone);
        info!(%phone, id = row.id, "folder created");
        self.item_of_row(&store, &row, &self.phone_config(phone)).map_err(OpError::from)
    }

    pub(super) async fn rename(
        &self,
        peer: &PeerDevice,
        identifier: &str,
        parent: &str,
        name: &str,
    ) -> Result<Item, OpError> {
        let phone = &peer.client;
        let store = self.running_store(phone).map_err(|_| OpError::NotRunning)?;
        let entry = store.entry(identifier).map_err(OpError::from)?.ok_or(OpError::UnknownItem)?;
        let folder = self.folder(&store, parent).map_err(OpError::from)?.ok_or(OpError::UnknownFolder)?;
        let location = path::join(&folder.location, name);

        let (uri, checksum, current) = match &entry {
            Entry::Row(r) => (r.uri.clone(), r.checksum, r.location.clone()),
            // The phone has no entry for it: it answers that it does not know the item.
            Entry::Folder(l) => (String::new(), 0, l.clone()),
        };

        if path::key(&location) != path::key(&current) {
            let taken = store.row_at(&location).map_err(OpError::from)?.is_some()
                || store.folder_at(&location).map_err(OpError::from)?.is_some();
            if taken {
                return Err(OpError::Exists);
            }
        }

        if path::ignored(name) {
            return Err(OpError::NotSupported);
        }

        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Update,
                wire::rename_request(&uri, checksum, &path::remote(&location)),
                true,
                TraceContext::new_root(phonelink_core::types::scenario_trace_state(wire::trace_name::RENAME)),
                self.shutdown.child_token(),
            )
            .await;

        let status = match outcome {
            DrmSendOutcome::Success { payload } => {
                wire::update_response(&payload).map(|r| Status::from_wire(r.status)).map_err(OpFailure::Status)
            }
            other => Err(OpFailure::Request(other)),
        };
        match status {
            Ok(Status::Success) => {}
            Ok(Status::NotFound) => return Err(OpError::NotInSync),
            Ok(Status::NotSupported) => return Err(OpError::NotSupported),
            Ok(other) => return Err(OpError::Phone(OpFailure::Status(other))),
            Err(failure) => return Err(OpError::Phone(failure)),
        }

        info!(%phone, identifier, "item renamed");
        let Entry::Row(mut row) = entry else {
            return Err(OpError::NotInSync);
        };

        // The item keeps its id and checksum until the next sync brings the phone's.
        row.location = location;
        row.name = name.to_owned();
        store.apply(std::slice::from_ref(&row), &[], None).map_err(OpError::from)?;
        self.items_changed(phone);
        self.item_of_row(&store, &row, &self.phone_config(phone)).map_err(OpError::from)
    }

    pub(super) async fn delete(self: Arc<Self>, peer: &PeerDevice, identifier: &str) -> Result<(), DeleteError> {
        let phone = &peer.client;
        let store = self.running_store(phone).map_err(|_| DeleteError::NotRunning)?;
        let entry = store.entry(identifier).map_err(DeleteError::from)?.ok_or(DeleteError::UnknownItem)?;

        // A folder goes with everything in it; the phone deletes only empty folders.
        let rows = match &entry {
            Entry::Row(r) if r.is_directory => store.subtree(&r.location).map_err(DeleteError::from)?,
            Entry::Row(r) => vec![r.clone()],
            Entry::Folder(l) => store.subtree(l).map_err(DeleteError::from)?,
        };

        if rows.is_empty() {
            return Ok(());
        }

        let config = self.phone_config(phone);
        let total = rows.len();
        let results = if config.batch_delete_capability {
            self.delete_batched(peer, rows).await
        } else {
            self.delete_single(peer, rows).await
        };

        let mut failed: Vec<OpFailure> = results.into_iter().filter_map(Result::err).collect();
        // A failure is the app's to log: it gets the error. The file manager deletes a selection one
        // item at a time, so a success is detail; a batch's answer is logged where it arrives.
        match failed.as_slice() {
            [] => {
                debug!(%phone, item_count = total, "items deleted");
                Ok(())
            }
            [OpFailure::Status(Status::NotSupported)] if total == 1 => Err(DeleteError::NotSupported),
            [OpFailure::Status(Status::ChecksumMismatch)] if total == 1 => Err(DeleteError::NotInSync),
            [_] if total == 1 => Err(DeleteError::Phone(failed.swap_remove(0))),
            _ => {
                let count = failed.len();
                Err(DeleteError::Partial { failed: count, total, first: failed.swap_remove(0) })
            }
        }
    }

    /// Takes the deleted `ids` out of the tree.
    fn deleted(&self, peer: &PeerDevice, ids: &[i64]) -> Result<(), StoreError> {
        let phone = &peer.client;
        if ids.is_empty() {
            return Ok(());
        }

        self.store(phone).and_then(|s| s.apply(&[], ids, None))?;
        self.items_changed(phone);
        Ok(())
    }

    fn log_not_removed(&self, peer: &PeerDevice, e: &StoreError) {
        error!(phone = %peer.client, error = %e, "deleted items not removed from the tree; they show until the next sync");
    }

    /// One request per item, children before their folders; a folder whose content was not deleted is
    /// not sent.
    async fn delete_single(&self, peer: &PeerDevice, mut rows: Vec<Row>) -> Vec<Result<(), OpFailure>> {
        rows.sort_by_key(|r| std::cmp::Reverse(r.location.matches('\\').count()));
        let mut failed: Vec<String> = Vec::new();
        let mut results = Vec::with_capacity(rows.len());
        // The store is written once per item: a failure is logged once per delete.
        let mut not_removed_logged = false;

        for row in &rows {
            if failed.iter().any(|f| path::is_within(f, &row.location)) {
                results.push(Err(OpFailure::Status(Status::DirectoryNotEmpty)));
                failed.push(row.location.clone());
                continue;
            }

            let outcome = self
                .files_request(
                    peer,
                    ProtoDeviceResourceRequestType::Delete,
                    wire::delete_request(&row.uri, row.checksum),
                    true,
                    TraceContext::new_root(phonelink_core::types::scenario_trace_state(wire::trace_name::DELETE)),
                    self.shutdown.child_token(),
                )
                .await;

            let result = match outcome {
                DrmSendOutcome::Success { payload } => wire::delete_response(&payload)
                    .map_err(OpFailure::Status)
                    .and_then(|r| single_result(Status::from_wire(r.status))),
                other => Err(OpFailure::Request(other)),
            };

            match &result {
                Ok(()) => {
                    if let Err(e) = self.deleted(peer, &[row.id]) {
                        if !std::mem::replace(&mut not_removed_logged, true) {
                            self.log_not_removed(peer, &e);
                        }
                    }
                }
                Err(_) => failed.push(row.location.clone()),
            }

            results.push(result);
        }
        results
    }

    /// Adds the items to the phone's delete batch and waits for their results.
    async fn delete_batched(self: &Arc<Self>, peer: &PeerDevice, rows: Vec<Row>) -> Vec<Result<(), OpFailure>> {
        let phone = &peer.client;
        let mut waits = Vec::with_capacity(rows.len());
        let timer = self.with_phone(phone, |p| {
            for row in rows {
                if p.deletes.pending.iter().any(|d| d.row.uri == row.uri) {
                    continue;
                }
                let (tx, rx) = oneshot::channel();
                p.deletes.pending.push(PendingDelete { row, reply: tx });
                waits.push(rx);
            }

            if let Some(t) = p.deletes.timer.take() {
                t.cancel();
            }

            let t = CancellationToken::new();
            p.deletes.timer = Some(t.clone());
            t
        });

        let (weak, peer_for_timer, debounce) = (self.weak.clone(), peer.clone(), self.config.delete_debounce);
        self.runtime.spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(debounce) => {}
                _ = timer.cancelled() => return,
            }
            if let Some(inner) = weak.upgrade() {
                inner.send_delete_batch(&peer_for_timer).await;
            }
        });

        let mut results = Vec::with_capacity(waits.len());
        for w in waits {
            results.push(w.await.unwrap_or(Err(OpFailure::Status(Status::Cancelled))));
        }
        results
    }

    async fn send_delete_batch(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        let locks = self.locks(phone);
        let _one = locks.delete.lock().await;

        let mut batch = self.with_phone(phone, |p| {
            p.deletes.timer = None;
            std::mem::take(&mut p.deletes.pending)
        });

        if batch.is_empty() {
            return;
        }

        // Longest leaf name first; the sort is stable, so ties keep the order they were collected in.
        batch.sort_by_key(|d| std::cmp::Reverse(path::leaf(&d.row.location).chars().count()));
        let items: Vec<(String, i64)> = batch.iter().map(|d| (d.row.uri.clone(), d.row.checksum)).collect();
        debug!(%phone, item_count = items.len(), "delete batch sent");

        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Delete,
                wire::delete_batch_request(&items),
                true,
                TraceContext::new_root(phonelink_core::types::scenario_trace_state(wire::trace_name::DELETE_BATCH)),
                self.shutdown.child_token(),
            )
            .await;

        // A failure is each waiting delete's, whose caller gets it.
        let reply = match outcome {
            DrmSendOutcome::Success { payload } => wire::delete_response(&payload).map_err(OpFailure::Status),
            other => Err(OpFailure::Request(other)),
        };

        let reply = match reply {
            Ok(r) => r,
            Err(failure) => {
                for d in batch {
                    let _ = d.reply.send(Err(failure.clone()));
                }
                return;
            }
        };

        let config = self.phone_config(phone);
        let bin = path::recycle_bin(&config.recycle_bin_path);
        let mut deleted = Vec::new();
        let mut any_failed = false;
        let mut file_outside_bin = false;
        let item_count = batch.len();

        for d in batch {
            let status = reply
                .batch_responses
                .iter()
                .find(|r| r.target_uri == d.row.uri)
                .map(|r| Status::from_wire(r.status))
                .unwrap_or(Status::Other(0));

            if status == Status::Success {
                deleted.push(d.row.id);
                if !d.row.is_directory && !bin.as_ref().is_some_and(|b| path::is_within(&d.row.location, b)) {
                    file_outside_bin = true;
                }
                let _ = d.reply.send(Ok(()));
            } else {
                any_failed = true;
                let _ = d.reply.send(Err(OpFailure::Status(status)));
            }
        }

        info!(%phone, item_count, deleted_count = deleted.len(), "delete batch answered");
        if let Err(e) = self.deleted(peer, &deleted) {
            self.log_not_removed(peer, &e);
        }

        let notice_off = self.store(phone).and_then(|s| s.data()).map(|d| d.recycle_notice_off).unwrap_or(true);
        if !any_failed && file_outside_bin && bin.is_some() && !notice_off {
            self.emit(PhoneFilesEvent::RecycleBinNotice { phone: phone.clone() });
        }
    }
}
