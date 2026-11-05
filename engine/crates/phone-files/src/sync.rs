//! Metadata syncs: the full refresh, the incremental sync, the batched full sync, and how a metadata
//! request travels — over the files-request channel while it works, over `/DeviceResourceManager`
//! otherwise.

use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rand::Rng;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::channel::{TransferError, open_error};
use super::path::{self, Rejected};
use super::store::Row;
use super::wire::{self, Status, Trigger, target};
use super::{Inner, SyncState};
use phonelink_core::edge::LockState;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::shared::db::StoreError;
use phonelink_proto::phone_files::v1::{
    FileMetadata, FilesGetRequest, FilesGetResponse, FullFileSyncPayload, files_get_response,
    get_files_metadata_sync_response, incremental_file_sync,
};
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::DrmSendOutcome;

/// Why a metadata request failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum RequestError {
    /// The `/DeviceResourceManager` request did not get a successful reply.
    #[error("the request failed: {0:?}")]
    Drm(DrmSendOutcome),
    /// The phone answered with another status.
    #[error("the phone answered {0:?}")]
    Status(Status),
    /// The phone's replies cannot be used, for `reason`; the sync state shows `status`.
    #[error("{reason}")]
    Unusable { status: Status, reason: &'static str },
    #[error("cancelled")]
    Cancelled,
}

impl RequestError {
    /// The phone could not be reached.
    fn is_connection(&self) -> bool {
        matches!(
            self,
            Self::Drm(
                DrmSendOutcome::SessionStartFailed(_)
                    | DrmSendOutcome::ResponseTimeout
                    | DrmSendOutcome::Request(_)
                    | DrmSendOutcome::Locked
            )
        )
    }
}

/// `m` as a row of the golden state.
pub(super) fn to_row(m: &FileMetadata) -> Result<Row, Rejected> {
    let mapped = path::map_path(&m.path)?;
    let name = path::clean_name(&m.name);

    if name.is_empty() {
        return Err(Rejected::DotComponent);
    }

    Ok(Row {
        id: m.id,
        uri: m.uri.clone(),
        location: path::join(path::parent(&mapped), &name),
        name,
        file_type: m.file_type,
        size: m.size,
        created: m.created_time.as_ref().map(|t| t.seconds),
        modified: m.modified_time.as_ref().map(|t| t.seconds),
        checksum: m.checksum,
        is_directory: m.is_directory,
    })
}

/// The rows of one reply's `items`; the rejected ones are left out, logged once for the reply.
fn rows(phone: &DcgClient, items: &[FileMetadata]) -> Vec<Row> {
    let mut first_rejection = None;
    let rows: Vec<Row> = items
        .iter()
        .filter_map(|m| match to_row(m) {
            Ok(r) => Some(r),
            Err(e) => {
                first_rejection.get_or_insert(e);
                None
            }
        })
        .collect();

    if let Some(e) = first_rejection {
        warn!(
            %phone,
            rejected_count = items.len() - rows.len(),
            item_count = items.len(),
            error = %e,
            "phone items left out of the tree"
        );
    }
    rows
}

impl Inner {
    // --------------------------------------------------------------------------------------------
    // Request transport
    // --------------------------------------------------------------------------------------------

    /// A metadata request over the channel when allowed and working, else (or after a failure) over
    /// `/DeviceResourceManager`, within the trace of the sync that sends it (`sync`, which carries the
    /// sync's names) and for its `trigger`.
    pub(super) async fn metadata_request(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        force_session_start: bool,
        trigger: Trigger,
        sync: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<FilesGetResponse, RequestError> {
        let phone = &peer.client;
        let capable = self.phone_config(phone).is_files_request_over_nano_capable;

        if self.config.sync_over_nano && capable && self.with_phone(phone, |p| p.breaker.attempt()) {
            let over_channel =
                self.request_over_channel(peer, request, force_session_start, trigger, sync, cancel).await;

            let recorded = match &over_channel {
                Ok(_) => Ok(()),
                Err(e) => Err(e.clone()),
            };
            self.record_channel_attempt(phone, &recorded);

            match over_channel {
                Ok(reply) => {
                    self.set_ltw_off(phone, false);
                    return Ok(reply);
                }
                Err(TransferError::UserCancelled | TransferError::Status(Status::Cancelled)) => {
                    return Err(RequestError::Cancelled);
                }
                Err(TransferError::LtwOff) => self.set_ltw_off(phone, true),
                Err(e) => debug!(%phone, error = %e, "metadata request over the channel failed; over the relay"),
            }
        }

        if cancel.is_cancelled() {
            return Err(RequestError::Cancelled);
        }

        let reply = self
            .request_over_drm(peer, request, force_session_start, sync.for_request(sync.trace_state.clone()), cancel)
            .await;
        if reply.is_ok() {
            self.set_ltw_off(phone, false);
        }
        reply
    }

    async fn request_over_drm(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        force_session_start: bool,
        trace: TraceContext,
        cancel: &CancellationToken,
    ) -> Result<FilesGetResponse, RequestError> {
        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Get,
                request.encode_to_vec(),
                force_session_start,
                trace,
                cancel.clone(),
            )
            .await;

        match outcome {
            DrmSendOutcome::Success { payload } => wire::parse_get_response(&payload).map_err(RequestError::Status),
            DrmSendOutcome::Canceled => Err(RequestError::Cancelled),
            other => Err(RequestError::Drm(other)),
        }
    }

    /// Whether the Mac's session is unlocked now.
    pub(super) async fn unlocked(&self) -> bool {
        let lock = self.env.edges.lock.clone();
        tokio::task::spawn_blocking(move || lock.lock_state()).await.unwrap_or(LockState::Unknown)
            == LockState::Unlocked
    }

    async fn request_over_channel(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        force_session_start: bool,
        trigger: Trigger,
        sync: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<FilesGetResponse, TransferError> {
        let phone = &peer.client;
        let handle = self.ensure_handle(peer, force_session_start, trigger, sync).await?;

        if !self.unlocked().await {
            return Err(TransferError::UserSessionLocked);
        }

        let open = tokio::time::timeout(
            self.config.request_channel_timeout,
            self.channels.open(&handle, wire::REQUEST_SCENARIO, &[target::FILES_REQUEST_RESPONSE], sync),
        );
        let mut opened = tokio::select! {
            r = open => match r {
                Err(_) => return Err(TransferError::ChannelCreationTimedOut),
                Ok(Err(e)) => return Err(open_error(&e, self.drm.metered_sync_disabled(phone), false)),
                Ok(Ok(c)) => c,
            },
            _ = cancel.cancelled() => return Err(TransferError::UserCancelled),
        };

        let mut replies = opened.take(target::FILES_REQUEST_RESPONSE);
        let channel = opened.channel;

        let result = async {
            channel
                .send_message(target::FILES_REQUEST, &request.encode_to_vec())
                .map_err(|_| TransferError::FailedTransferClosed)?;

            let mut closed = channel.closed();
            let wait = async {
                let mut unparsed_logged = false;
                loop {
                    tokio::select! {
                        m = replies.recv() => {
                            let Some(m) = m else { return Err(TransferError::FailedTransferClosed) };
                            let Ok(reply) = FilesGetResponse::decode(m.body.as_slice()) else {
                                if !std::mem::replace(&mut unparsed_logged, true) {
                                    warn!(%phone, "metadata reply does not parse; ignored");
                                }
                                continue;
                            };
                            if !reply.request_id.is_empty() && reply.request_id != request.request_id {
                                continue;
                            }
                            return match Status::from_wire(reply.status) {
                                Status::Success => Ok(reply),
                                other => Err(TransferError::Status(other)),
                            };
                        }
                        _ = closed.wait_for(|c| *c) => return Err(TransferError::FailedTransferClosed),
                    }
                }
            };

            tokio::select! {
                r = tokio::time::timeout(self.config.request_reply_timeout, wait) => {
                    r.unwrap_or_else(|_| Err(TransferError::Failed("no reply in time".into())))
                }
                _ = cancel.cancelled() => Err(TransferError::UserCancelled),
            }
        }
        .await;

        channel.close();
        result
    }

    // --------------------------------------------------------------------------------------------
    // Syncs
    // --------------------------------------------------------------------------------------------

    /// Ends when the phone stops being available or the feature stops for it.
    fn sync_cancel(&self, peer: &PeerDevice) -> CancellationToken {
        self.with_phone(&peer.client, |p| p.sync_cancel.child_token())
    }

    fn sync_failed(&self, peer: &PeerDevice, e: &RequestError, sync_kind: &'static str) {
        let phone = &peer.client;
        match e {
            RequestError::Cancelled => debug!(%phone, sync_kind, "metadata sync cancelled"),
            e => warn!(%phone, sync_kind, error = %e, "metadata sync failed"),
        }

        let state = match e {
            RequestError::Cancelled => SyncState::Idle,
            e if e.is_connection() => SyncState::ConnectionFailed,
            RequestError::Status(s) | RequestError::Unusable { status: s, .. } => SyncState::Failed(format!("{s:?}")),
            RequestError::Drm(o) => SyncState::Failed(format!("{o:?}")),
        };
        self.set_sync(phone, state);
    }

    /// The full refresh: everything from the phone (or, with a known sequence, an incremental request),
    /// replacing the golden state.
    pub(super) async fn full_refresh(self: Arc<Self>, peer: &PeerDevice, trigger: Trigger) {
        let phone = &peer.client;
        let locks = self.locks(phone);
        let _sync = locks.sync.lock().await;

        if !self.with_phone(phone, |p| p.running) {
            return;
        }

        let cancel = self.sync_cancel(peer);
        self.set_sync(phone, SyncState::Syncing);
        let sequence = match self.store(phone).and_then(|s| s.data()) {
            Ok(d) => d.sequence,
            Err(e) => return error!(%phone, error = %e, "phone-files state unreadable; no full refresh"),
        };

        info!(%phone, trigger = trigger.wire(), sequence, "full refresh started");
        let reply = match self.refresh_request(peer, sequence, trigger, &cancel).await {
            Ok(r) => r,
            Err(e) => return self.sync_failed(peer, &e, "full refresh"),
        };

        if let Err(e) = self.apply_refresh(peer, reply, sequence) {
            return self.sync_not_stored(phone, "full refresh", &e);
        }

        self.record_full_refresh(phone);
        self.set_sync(phone, SyncState::Succeeded);
        self.gate.notify_waiters();
    }

    /// A sync whose result was not stored: logged, and the sync failed.
    fn sync_not_stored(&self, phone: &DcgClient, sync_kind: &'static str, e: &dyn std::fmt::Display) {
        error!(%phone, sync_kind, error = %e, "metadata sync not stored");
        self.set_sync(phone, SyncState::Failed(e.to_string()));
    }

    /// Stores that the initial sync is done and the full refresh happened now.
    fn record_full_refresh(&self, phone: &DcgClient) {
        let now = self.now_secs();
        if let Err(e) = self.update(phone, |d| {
            d.initial_sync_done = true;
            d.last_full_refresh = Some(now);
        }) {
            error!(%phone, error = %e, "full refresh time not stored; the next start refreshes in full again");
        }
    }

    /// The full refresh's request from stored sequence `sequence`: a full sync (batched, or retried
    /// once after a timeout) without one, an incremental request with one.
    async fn refresh_request(
        &self,
        peer: &PeerDevice,
        sequence: i64,
        trigger: Trigger,
        cancel: &CancellationToken,
    ) -> Result<Reply, RequestError> {
        let phone = &peer.client;
        // One trace for the refresh, its retry and the handle it may need.
        let scenario = if sequence == -1 {
            wire::trace_name::FULL_SYNC_TO_LOCAL_DB
        } else {
            wire::trace_name::INCREMENTAL_SYNC_TO_LOCAL_DB
        };
        let trace = TraceContext::new_root(phonelink_core::types::trace_state(scenario, trigger.wire()));

        if sequence == -1 && self.config.batched_full_sync {
            return self.batched_full_sync(peer, trigger, &trace, cancel).await.map(Reply::Full);
        }

        let request = wire::metadata_sync_request(sequence == -1, sequence, false, None);
        let mut r = self.metadata_request(peer, &request, false, trigger, &trace, cancel).await;

        if matches!(r, Err(RequestError::Drm(DrmSendOutcome::ResponseTimeout))) && sequence == -1 {
            warn!(%phone, "full sync timed out; once more");
            tokio::select! {
                _ = tokio::time::sleep(self.config.full_sync_retry_delay) => {}
                _ = cancel.cancelled() => {}
            }
            if !cancel.is_cancelled() {
                let request = wire::metadata_sync_request(true, -1, false, None);
                r = self.metadata_request(peer, &request, false, trigger, &trace, cancel).await;
            }
        }

        r.and_then(|reply| read_sync_reply(phone, reply))
    }

    /// Applies the full refresh's reply to the tree stored at `sequence`.
    fn apply_refresh(&self, peer: &PeerDevice, reply: Reply, sequence: i64) -> Result<(), StoreError> {
        let phone = &peer.client;
        match reply {
            Reply::Full(full) => self.apply_full(peer, &full),
            Reply::Incremental { starting, ending, upserts, deletes } => {
                if starting < sequence {
                    warn!(%phone, starting_sequence = starting, sequence, "the phone's changes start before the stored sequence; applied");
                }
                self.apply_incremental(peer, &upserts, &deletes, ending)
            }
        }
    }

    /// An incremental sync to (at least) `triggering`; `None` = always sent.
    pub(super) async fn incremental_sync(
        self: Arc<Self>,
        peer: &PeerDevice,
        triggering: Option<i64>,
        user: bool,
        trigger: Trigger,
    ) {
        let phone = &peer.client;
        let locks = self.locks(phone);
        let _sync = locks.sync.lock().await;

        if !self.with_phone(phone, |p| p.running) {
            return;
        }

        let cancel = self.sync_cancel(peer);
        let (mut triggering, mut user, mut trigger) = (triggering, user, trigger);
        let mut expired_once = false;

        loop {
            let data = match self.store(phone).and_then(|s| s.data()) {
                Ok(d) => d,
                Err(e) => return error!(%phone, error = %e, "phone-files state unreadable; no incremental sync"),
            };

            if let Some(t) = triggering {
                if t <= data.sequence && !user {
                    return;
                }
            }

            if user {
                let now = self.now_secs();
                let _ = self.update(phone, |d| d.last_user_interaction = Some(now));
            }

            self.set_sync(phone, SyncState::Preparing);
            let reply = match self.incremental_request(peer, data.sequence, user, trigger, &cancel).await {
                Ok(r) => r,
                Err(e) => return self.sync_failed(peer, &e, "incremental sync"),
            };

            self.set_sync(phone, SyncState::Syncing);
            let applied = match self.apply_incremental_reply(peer, reply, data.sequence) {
                IncrementalApplied::Expired { starting } => {
                    if expired_once {
                        warn!(
                            %phone,
                            sequence_number = starting,
                            sequence = data.sequence,
                            "the phone's changes start before the stored sequence again; the sync fails"
                        );
                        return self.set_sync(phone, SyncState::Failed("sequence expired".into()));
                    }

                    // Phone Link on Windows asks again only once the phone has become unavailable; the
                    // Mac asks at once, as that later request would: not user-initiated and whatever
                    // the triggering sequence.
                    expired_once = true;
                    (triggering, user, trigger) = (None, false, Trigger::SequenceNumberExpired);
                    continue;
                }
                IncrementalApplied::Done(r) => r,
            };

            match applied {
                Ok(()) => self.set_sync(phone, SyncState::Succeeded),
                Err(e) => self.sync_not_stored(phone, "incremental sync", &e),
            }
            return;
        }
    }

    /// The incremental sync's request from stored sequence `sequence`.
    async fn incremental_request(
        &self,
        peer: &PeerDevice,
        sequence: i64,
        user: bool,
        trigger: Trigger,
        cancel: &CancellationToken,
    ) -> Result<Reply, RequestError> {
        let phone = &peer.client;
        info!(%phone, trigger = trigger.wire(), sequence, is_user = user, "incremental sync started");
        let request = wire::metadata_sync_request(false, sequence, user, Some(false));
        let trace = TraceContext::new_root(phonelink_core::types::trace_state(
            wire::trace_name::INCREMENTAL_SYNC,
            trigger.wire(),
        ));

        self.metadata_request(peer, &request, user, trigger, &trace, cancel)
            .await
            .and_then(|reply| read_sync_reply(phone, reply))
    }

    /// Applies the incremental sync's reply to the tree stored at `sequence`, unless the phone's
    /// changes start before it.
    fn apply_incremental_reply(&self, peer: &PeerDevice, reply: Reply, sequence: i64) -> IncrementalApplied {
        let phone = &peer.client;
        match reply {
            Reply::Incremental { starting, .. } if starting < sequence => IncrementalApplied::Expired { starting },
            Reply::Incremental { ending, upserts, deletes, .. } => {
                IncrementalApplied::Done(self.apply_incremental(peer, &upserts, &deletes, ending))
            }
            Reply::Full(full) => {
                let r = self.apply_full(peer, &full);
                if r.is_ok() {
                    let now = self.now_secs();
                    if let Err(e) = self.update(phone, |d| d.last_full_refresh = Some(now)) {
                        error!(%phone, error = %e, "full refresh time not stored; the next start refreshes in full again");
                    }
                }
                IncrementalApplied::Done(r)
            }
        }
    }

    /// The batched full sync: the phone's items in batches, staged, then applied as one full reply.
    async fn batched_full_sync(
        &self,
        peer: &PeerDevice,
        trigger: Trigger,
        trace: &TraceContext,
        cancel: &CancellationToken,
    ) -> Result<FullFileSyncPayload, RequestError> {
        const MAX_BATCHES: u32 = 200;
        const MAX_RESTARTS: u32 = 3;

        let phone = &peer.client;
        let mut staged: Vec<FileMetadata> = Vec::new();
        let (mut index, mut token, mut restarts, mut batches, mut retries) = (0i32, String::new(), 0u32, 0u32, 0u32);
        let failed = |reason| RequestError::Unusable { status: Status::SyncFailed, reason };

        loop {
            if batches >= MAX_BATCHES {
                return Err(failed("the batched full sync took too many batches"));
            }

            let request = wire::batched_full_sync_request(&token);
            let reply = self.batch_request(peer, &request, trigger, trace, cancel, &mut retries).await?;
            batches += 1;

            // A restart is a retry of the whole sync: each one is logged.
            let restart = |restarts: &mut u32, cause: &'static str| -> Result<(), RequestError> {
                *restarts += 1;
                if *restarts > MAX_RESTARTS {
                    return Err(failed("the batched full sync restarted too often"));
                }
                warn!(%phone, attempt = *restarts, cause, "batched full sync restarts");
                Ok(())
            };

            let full = match reply.and_then(|reply| read_sync_reply(phone, reply)) {
                Err(RequestError::Status(Status::BatchRestartRequired)) => {
                    restart(&mut restarts, "the phone asked for a restart")?;
                    (index, token) = (0, String::new());
                    staged.clear();
                    continue;
                }
                Err(e) => return Err(e),
                Ok(Reply::Incremental { .. }) => return Err(failed("an incremental reply in a batched full sync")),
                Ok(Reply::Full(full)) => full,
            };

            if full.full_sync_max_capacity_reached {
                staged.extend(full.metadata);
                return Ok(FullFileSyncPayload {
                    metadata: staged,
                    ending_sequence_number: full.ending_sequence_number,
                    ..Default::default()
                });
            }

            if full.batch_index != index {
                restart(&mut restarts, "a batch out of order")?;
                (index, token) = (0, String::new());
                staged.clear();
                continue;
            }

            if index == 0 {
                debug!(%phone, estimated = full.estimated_total_items, "batched full sync first batch received");
            }

            staged.extend(full.metadata);
            if full.has_more_batches {
                if full.batch_continuation_token.is_empty() {
                    return Err(failed("a batch with more to come and no continuation token"));
                }
                token = full.batch_continuation_token;
                index += 1;
            } else {
                return Ok(FullFileSyncPayload {
                    metadata: staged,
                    ending_sequence_number: full.ending_sequence_number,
                    ..Default::default()
                });
            }
        }
    }

    /// One batch request of the batched full sync, retried with back-off on a transient failure while
    /// the batch's and the sync's (`retries`) retries last; the outer error when cancelled meanwhile.
    async fn batch_request(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        trigger: Trigger,
        trace: &TraceContext,
        cancel: &CancellationToken,
        retries: &mut u32,
    ) -> Result<Result<FilesGetResponse, RequestError>, RequestError> {
        const RETRIES_PER_BATCH: u32 = 3;
        const RETRIES_PER_SYNC: u32 = 10;

        let phone = &peer.client;
        let mut tries = 0;

        loop {
            match self.metadata_request(peer, request, false, trigger, trace, cancel).await {
                Err(RequestError::Drm(o))
                    if transient(&o) && tries < RETRIES_PER_BATCH && *retries < RETRIES_PER_SYNC =>
                {
                    tries += 1;
                    *retries += 1;
                    let base = (1000u64 << (tries - 1)).min(5000);
                    let delay_ms = base + rand::rng().random_range(0..=base / 4);
                    warn!(%phone, tries, outcome = ?o, error = o.cause(), delay_ms, "batch request failed; trying again");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                        _ = cancel.cancelled() => return Err(RequestError::Cancelled),
                    }
                }
                other => return Ok(other),
            }
        }
    }

    fn apply_full(&self, peer: &PeerDevice, full: &FullFileSyncPayload) -> Result<(), StoreError> {
        let phone = &peer.client;
        let rows = rows(phone, &full.metadata);
        let changed = self.store(phone).and_then(|s| s.replace_all(&rows, full.ending_sequence_number))?;
        info!(%phone, item_count = rows.len(), sequence = full.ending_sequence_number, changed, "full sync applied");
        if changed {
            self.items_changed(phone);
        }
        Ok(())
    }

    fn apply_incremental(
        &self,
        peer: &PeerDevice,
        upserts: &[Row],
        deletes: &[i64],
        ending: i64,
    ) -> Result<(), StoreError> {
        let phone = &peer.client;
        let changed = self.store(phone).and_then(|s| s.apply(upserts, deletes, Some(ending)))?;
        info!(
            %phone,
            upserts = upserts.len(),
            deletes = deletes.len(),
            sequence = ending,
            changed,
            "incremental sync applied"
        );
        if changed {
            self.items_changed(phone);
        }
        Ok(())
    }
}

/// A resource failure the batched sync retries.
fn transient(o: &DrmSendOutcome) -> bool {
    matches!(o, DrmSendOutcome::ResponseTimeout | DrmSendOutcome::Request(_) | DrmSendOutcome::SessionStartFailed(_))
}

/// A metadata reply, read.
enum Reply {
    Full(FullFileSyncPayload),
    Incremental { starting: i64, ending: i64, upserts: Vec<Row>, deletes: Vec<i64> },
}

/// What applying an incremental sync's reply did.
enum IncrementalApplied {
    /// The phone's changes start at `starting`, before the stored sequence: nothing applied.
    Expired {
        starting: i64,
    },
    Done(Result<(), StoreError>),
}

fn read_sync_reply(phone: &DcgClient, reply: FilesGetResponse) -> Result<Reply, RequestError> {
    let unusable = |reason| RequestError::Unusable { status: Status::IoError, reason };
    let Some(files_get_response::Response::FilesMetadataSyncResponse(sync)) = reply.response else {
        return Err(unusable("a metadata reply without a sync response"));
    };

    match sync.payload {
        Some(get_files_metadata_sync_response::Payload::FullSync(full)) => Ok(Reply::Full(full)),
        Some(get_files_metadata_sync_response::Payload::IncrementalSync(inc)) => {
            let mut upserted = Vec::new();
            let mut deletes = Vec::new();
            let mut unknown_count = 0usize;

            for op in inc.sync_operation {
                match op.operation {
                    Some(incremental_file_sync::Operation::Create(c)) => upserted.extend(c.metadata),
                    Some(incremental_file_sync::Operation::Update(u)) => upserted.extend(u.metadata),
                    Some(incremental_file_sync::Operation::Delete(d)) => deletes.extend(d.metadata.map(|m| m.id)),
                    None => unknown_count += 1,
                }
            }

            if unknown_count > 0 {
                warn!(%phone, unknown_count, "incremental operations of an unknown kind; ignored");
            }

            Ok(Reply::Incremental {
                starting: inc.starting_sequence_number,
                ending: inc.ending_sequence_number,
                upserts: rows(phone, &upserted),
                deletes,
            })
        }
        None => Err(unusable("a sync response without a payload")),
    }
}
