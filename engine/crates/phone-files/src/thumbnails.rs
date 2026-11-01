//! Thumbnails: one file's thumbnail (cache, thumbnail channel, `/DeviceResourceManager`) and the
//! prefetch of the phone's media folders over the thumbnail channel.

use std::sync::Arc;

use prost::Message;
use tracing::{debug, warn};

use super::channel::{TransferError, open_error};
use super::path;
use super::store::{Entry, Row, Store};
use super::wire::{self, PrefetchItem, Status, Trigger, target};
use super::{Inner, PhoneLocks};
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_proto::phone_files::v1::{
    FilesConfigurationPayload, FilesGetRequest, FilesGetResponse, FilesThumbnailPrefetchMessage,
    PrefetchThumbnailResponse, files_get_response,
};
use phonelink_proto::platform_sdk::v1::ProtoDeviceResourceRequestType;
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::DrmSendOutcome;

impl Inner {
    /// The row has a thumbnail: a file of a type the phone makes thumbnails of, outside its recycle bin.
    pub(super) fn has_thumbnail(&self, row: &Row, config: &FilesConfigurationPayload) -> bool {
        !row.is_directory
            && config.thumbnail_supported_file_types.contains(&row.file_type)
            && !path::recycle_bin(&config.recycle_bin_path).is_some_and(|bin| path::is_within(&row.location, &bin))
    }

    pub(super) async fn thumbnail(
        self: Arc<Self>,
        peer: &PeerDevice,
        identifier: &str,
    ) -> Result<Option<Vec<u8>>, TransferError> {
        let phone = &peer.client;
        let store = self.running_store(phone).map_err(|e| TransferError::Failed(e.to_string()))?;
        let Some(Entry::Row(row)) = store.entry(identifier).map_err(|e| TransferError::Failed(e.to_string()))? else {
            return Ok(None);
        };

        let config = self.phone_config(phone);
        if !self.has_thumbnail(&row, &config) {
            return Ok(None);
        }

        let size = self.config.thumbnail_size;
        if let Ok(Some((checksum, cached, jpeg))) = store.thumbnail(row.id) {
            if checksum == row.checksum && cached >= size {
                return Ok(Some(jpeg));
            }
        }

        let request = wire::thumbnail_request(&row.uri, row.checksum, size);
        // One trace for the thumbnail: the channel, the handle it may need and the relay request.
        let trace = TraceContext::new_root(phonelink_core::types::trace_state(
            wire::trace_name::GET_THUMBNAIL,
            wire::trace_name::THUMBNAIL_REQUESTED,
        ));

        let locks = self.locks(phone);
        let mut over_channel = None;
        if self.config.thumbnails_over_nano
            && config.is_get_thumbnail_over_nano_mobile_enabled
            && self.with_phone(phone, |p| p.breaker.attempt())
        {
            let _one = locks.thumbnail.lock().await;
            let r = self
                .thumbnail_over_channel(
                    peer,
                    &request,
                    target::GET_THUMBNAIL_REQUEST,
                    Trigger::GetThumbnail,
                    true,
                    &trace,
                )
                .await;

            let recorded = r.as_ref().map(|_| ()).map_err(Clone::clone);
            self.record_channel_attempt(phone, &recorded);
            match r {
                Ok(reply) => over_channel = Some(reply),
                Err(e) => debug!(%phone, error = %e, "thumbnail over the channel failed; over the relay"),
            }
        }

        let jpeg = match over_channel {
            Some(reply) => match reply.response {
                Some(files_get_response::Response::ThumbnailResponse(t)) => t.thumbnail,
                _ => return Err(TransferError::Status(Status::NotSupported)),
            },
            None => match self.thumbnail_over_drm(peer, &request, &trace).await? {
                Some(jpeg) => jpeg,
                None => return Ok(None),
            },
        };

        // The thumbnail is returned all the same; one is asked for per file shown.
        if let Err(e) = store.put_thumbnail(row.id, row.checksum, size, &jpeg) {
            debug!(%phone, error = %e, "thumbnail not cached");
        }
        Ok(Some(jpeg))
    }

    /// The `/DeviceResourceManager` thumbnail: `None` when the file no longer exists.
    async fn thumbnail_over_drm(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        trace: &TraceContext,
    ) -> Result<Option<Vec<u8>>, TransferError> {
        let outcome = self
            .files_request(
                peer,
                ProtoDeviceResourceRequestType::Get,
                request.encode_to_vec(),
                true,
                trace.for_request(trace.trace_state.clone()),
                self.shutdown.child_token(),
            )
            .await;

        let payload = match outcome {
            DrmSendOutcome::Success { payload } => payload,
            other => return Err(TransferError::Request(other)),
        };

        match wire::parse_get_response(&payload) {
            Ok(FilesGetResponse { response: Some(files_get_response::Response::ThumbnailResponse(t)), .. }) => {
                Ok(Some(t.thumbnail))
            }
            Ok(_) => Err(TransferError::Status(Status::NotSupported)),
            Err(Status::NotFound) => Ok(None),
            Err(s) => Err(TransferError::Status(s)),
        }
    }

    /// One request on a new thumbnail channel and its reply.
    async fn thumbnail_over_channel(
        &self,
        peer: &PeerDevice,
        request: &FilesGetRequest,
        to: &str,
        trigger: Trigger,
        user: bool,
        trace: &TraceContext,
    ) -> Result<FilesGetResponse, TransferError> {
        let phone = &peer.client;
        let handle = self.ensure_handle(peer, user, trigger, trace).await?;

        if !self.unlocked().await {
            return Err(TransferError::UserSessionLocked);
        }

        let open = tokio::time::timeout(
            self.config.thumbnail_channel_timeout,
            self.channels.open(&handle, wire::THUMBNAIL_SCENARIO, &target::THUMBNAIL_REPLIES, trace),
        )
        .await;

        let mut opened = match open {
            Err(_) => return Err(TransferError::ChannelCreationTimedOut),
            Ok(Err(e)) => return Err(open_error(&e, self.drm.metered_sync_disabled(phone), false)),
            Ok(Ok(c)) => c,
        };

        let mut single = opened.take(target::GET_THUMBNAIL_RESPONSE);
        let mut prefetch = opened.take(target::PREFETCH_THUMBNAILS_RESPONSE);
        let mut error = opened.take(target::THUMBNAIL_ERROR_RESPONSE);
        let channel = opened.channel;

        let result = async {
            channel.send_message(to, &request.encode_to_vec()).map_err(|_| TransferError::FailedTransferClosed)?;
            let mut closed = channel.closed();
            let wait = async {
                let m = tokio::select! {
                    m = single.recv() => m,
                    m = prefetch.recv() => m,
                    m = error.recv() => m,
                    _ = closed.wait_for(|c| *c) => None,
                };

                let Some(m) = m else { return Err(TransferError::FailedTransferClosed) };
                let reply = FilesGetResponse::decode(m.body.as_slice())
                    .map_err(|e| TransferError::Failed(format!("the phone's reply does not parse: {e}")))?;
                match Status::from_wire(reply.status) {
                    Status::Success => Ok(reply),
                    other => Err(TransferError::Status(other)),
                }
            };

            tokio::time::timeout(self.config.thumbnail_reply_timeout, wait)
                .await
                .unwrap_or_else(|_| Err(TransferError::Failed("no reply in time".into())))
        }
        .await;

        channel.close();
        result
    }

    /// The prefetch: for each folder of the phone's prefetch list, thumbnails of up to its count of
    /// files directly inside it that have none cached.
    pub(super) async fn prefetch(self: Arc<Self>, peer: &PeerDevice) {
        let phone = &peer.client;
        let config = self.phone_config(phone);
        if !self.config.prefetch_over_nano || !config.is_prefetch_thumbnails_over_nano_mobile_enabled {
            return;
        }

        let locks = self.locks(phone);
        let _one = locks.prefetch.lock().await;
        let Ok(store) = self.running_store(phone) else { return };

        for folder in &config.thumbnail_prefetch_config {
            if !self.prefetch_folder(peer, &store, &config, &locks, folder).await {
                return;
            }
        }
    }

    /// The prefetch of one folder; `false` when the prefetch stops.
    async fn prefetch_folder(
        &self,
        peer: &PeerDevice,
        store: &Store,
        config: &FilesConfigurationPayload,
        locks: &PhoneLocks,
        folder: &FilesThumbnailPrefetchMessage,
    ) -> bool {
        let phone = &peer.client;
        let size = self.config.thumbnail_size;
        let Some(items) = self.prefetch_items(phone, store, config, folder) else { return true };
        let request = wire::prefetch_request(&items, size);

        if !self.with_phone(phone, |p| p.breaker.attempt()) {
            return false;
        }

        let r = {
            let _one = locks.thumbnail.lock().await;
            self.thumbnail_over_channel(
                peer,
                &request,
                target::PREFETCH_THUMBNAILS_REQUEST,
                Trigger::PrefetchThumbnails,
                false,
                // A channel of messages only: no trace state reaches the wire.
                &TraceContext::new_root(Vec::new()),
            )
            .await
        };

        let recorded = r.as_ref().map(|_| ()).map_err(Clone::clone);
        self.record_channel_attempt(phone, &recorded);
        let reply = match r {
            Ok(reply) => reply,
            Err(e) => {
                warn!(%phone, error = %e, "thumbnail prefetch failed; the prefetch stops");
                return false;
            }
        };

        let Some(files_get_response::Response::PrefetchThumbnailResponse(p)) = reply.response else {
            warn!(%phone, "prefetch reply without thumbnails");
            return true;
        };

        let cached = cache_prefetched(store, p, size);
        debug!(%phone, folder = folder.name, cached_count = cached, "thumbnails prefetched");
        true
    }

    /// The files of `folder` to prefetch: up to its count, directly inside it, with no thumbnail cached;
    /// `None` when there are none.
    fn prefetch_items(
        &self,
        phone: &DcgClient,
        store: &Store,
        config: &FilesConfigurationPayload,
        folder: &FilesThumbnailPrefetchMessage,
    ) -> Option<Vec<PrefetchItem>> {
        let size = self.config.thumbnail_size;
        let Ok(location) = path::map_path(&folder.path) else {
            debug!(%phone, path = folder.path, "prefetch folder not in the tree");
            return None;
        };

        let Ok((mut rows, _)) = store.children(&location) else { return None };
        rows.retain(|r| {
            self.has_thumbnail(r, config)
                && !store.thumbnail(r.id).ok().flatten().is_some_and(|(c, s, _)| c == r.checksum && s >= size)
        });

        rows.sort_by(|a, b| b.location.cmp(&a.location));
        rows.truncate(folder.prefetch_count.max(0) as usize);

        if rows.is_empty() {
            return None;
        }

        Some(
            rows.iter()
                .map(|r| PrefetchItem { id: r.id, uri: r.uri.clone(), checksum: r.checksum, path: r.location.clone() })
                .collect(),
        )
    }
}

/// Caches the successful thumbnails of a prefetch reply; returns how many were cached.
fn cache_prefetched(store: &Store, p: PrefetchThumbnailResponse, size: i32) -> i32 {
    let mut cached = 0;
    for t in p.thumbnail_response {
        if Status::from_wire(t.status) != Status::Success {
            continue;
        }
        let Some(m) = t.metadata else { continue };
        if store.put_thumbnail(m.id, m.checksum, size, &t.thumbnail).is_ok() {
            cached += 1;
        }
    }
    cached
}
