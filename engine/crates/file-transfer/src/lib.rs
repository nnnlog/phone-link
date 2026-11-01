//! File transfer, a Cross Device Experience Host feature on the CDEH identity's connection: the user
//! sends files to the phone, and the phone's share sheet sends files to the Mac.
//!
//! The files travel over a Nano `NamedStreams` channel created against the phone's context handle;
//! the relay carries the set-up — the files permission request, the phone's tag-3 publication that
//! delivers the handle, and for a receive the phone's `/fileshare_android2cdeh_phase1` request.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the requests and their replies, the receive request, the control messages, file names |
//! | [`store`] | the per-phone settings (*Share*, the received-files folder, whether its setting shows) |
//! | [`channel`] | creating and opening a transfer's channel |
//! | `send` | a send: gate, permission, handle, channel, the phone's messages, progress, cancel |
//! | `receive` | the receive request and the receive session, destinations, the received notification |
//! | `notify` | the notifications: send states, grant permission, received files |
//! | this module | the options, tag 3 and the context handles, the permission request, the probe and its triggers, the settings |

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod store;
pub mod wire;

mod notify;
mod receive;
mod send;

use phonelink_core::config::features::FileTransferConfig;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::shared::files_permission::{self, PermissionResult};
use phonelink_features::{FeatureEnv, FeatureError, PhoneSwitchChanged};
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::{PeerDevice, PeerEvent};
use phonelink_transport::platform::{
    ContextHandle, DeviceFilter, Drm, DrmAddress, DrmSendOutcome, DrmSendRequest, PlatformRoutes, Publication,
    PublicationObserver, SessionValidator, Subscription,
};

use phonelink_features::shared::nano_channels::ShareChannels;
pub use send::{SendError, SendOrigin};
pub use store::{FEATURE, PhoneData};
pub use wire::PermissionTrigger;

use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;

/// `base` without `FILE_TRANSFER`, plus `FILE_TRANSFER` when the Mac receives files.
pub fn with_file_transfer_capability(base: &[String], config: &FileTransferConfig) -> Vec<String> {
    let mut capabilities: Vec<String> = base.iter().filter(|c| c.as_str() != wire::CAPABILITY).cloned().collect();
    if config.phone_to_pc_enabled {
        capabilities.push(wire::CAPABILITY.to_owned());
    }
    capabilities
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTransferEvent {
    /// What [`FileTransfer::settings`] or [`FileTransfer::transfers`] shows for `phone` may have changed.
    Changed { phone: DcgClient },
    /// Open the received file at `path` (a received notification was clicked).
    Open { path: PathBuf },
    /// Show `path` in the Finder (a received notification's folder action).
    Reveal { path: PathBuf },
}

/// The feature's settings for one phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneSettings {
    /// The phone can be sent files: an Android phone that advertises `FILE_TRANSFER`.
    pub share_capable: bool,
    /// The *Share* toggle shows.
    pub share_toggle_shown: bool,
    /// *Share* (default on): the user may send files to the phone.
    pub share_enabled: bool,
    /// The phone is offered as a send target now.
    pub offered_as_send_target: bool,
    /// The received-files-folder setting shows.
    pub folder_setting_shown: bool,
    /// Where files from the phone go; `None` when there is no folder (no home folder).
    pub received_files_folder: Option<PathBuf>,
}

/// Where a send stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPhase {
    /// The permission check, the context handle, the channel.
    Connecting,
    /// The file list was sent; the phone has not asked for the files yet.
    Preparing,
    Sending,
}

/// A running send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendProgress {
    pub phase: SendPhase,
    pub file_names: Vec<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// A running receive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveProgress {
    /// The phone's file list has not arrived yet.
    pub starting: bool,
    pub file_names: Vec<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// The transfers running with one phone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transfers {
    pub send: Option<SendProgress>,
    pub receive: Option<ReceiveProgress>,
}

/// An operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum FileTransferError {
    /// The phone is not in the account's device list.
    #[error("the phone is not in the account's device list")]
    UnknownPhone,
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

/// Why no context handle was obtained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandleError {
    /// The permission request's result, when it was not *granted*.
    Permission(PermissionResult),
    TimedOut,
    Cancelled,
    Failed,
}

/// A phone's map and store key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// Android and the phone's `FILE_TRANSFER` capability.
fn share_capable(peer: &PeerDevice) -> bool {
    peer.info.is_android() && peer.info.has_capability(wire::CAPABILITY)
}

#[derive(Default)]
struct State {
    /// The context handle of each phone's latest tag-3 publication.
    handles: HashMap<DcgClient, ContextHandle>,
    /// Phones with a send running, and its cancellation.
    sending: HashMap<DcgClient, CancellationToken>,
    /// Phones with a receive starting or running.
    receiving: HashSet<DcgClient>,
    transfers: HashMap<DcgClient, Transfers>,
}

/// The feature's shared state; tasks and callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    config: FileTransferConfig,
    drm: Arc<Drm>,
    validator: Arc<SessionValidator>,
    channels: Arc<dyn ShareChannels>,
    store: PhoneStores,
    state: Mutex<State>,
    /// Serializes read-modify-write of a phone's row.
    data_lock: Mutex<()>,
    events: broadcast::Sender<FileTransferEvent>,
    /// A phone whose stored context handle changed.
    handle_updated: broadcast::Sender<DcgClient>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscription: Mutex<Option<Subscription>>,
    /// The phone switch's last read failed and was logged; reset by a read that works.
    switch_unreadable: AtomicBool,
    /// The last notification post failed and was logged; reset by a post that works.
    post_failing: AtomicBool,
    weak: Weak<Inner>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The file-transfer feature.
pub struct FileTransfer {
    inner: Arc<Inner>,
}

impl FileTransfer {
    /// Installs the feature on the CDEH identity's `env` and `routes`: the notification responder, the
    /// `/fileshare_android2cdeh_phase1` receiver, the tag-3 subscription, the triggers of the probe and
    /// the probe of every phone now. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: FileTransferConfig,
        routes: &PlatformRoutes,
        channels: Arc<dyn ShareChannels>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("file transfer must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            drm: routes.drm.clone(),
            validator: routes.validator.clone(),
            channels,
            store: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
            state: Mutex::new(State::default()),
            data_lock: Mutex::new(()),
            events: broadcast::channel(64).0,
            handle_updated: broadcast::channel(64).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscription: Mutex::new(None),
            switch_unreadable: AtomicBool::new(false),
            post_failing: AtomicBool::new(false),
            weak: weak.clone(),
        });

        env.responses
            .register(FEATURE, Arc::new(notify::Responder(Arc::downgrade(&inner))))
            .map_err(RegistrationError::from)?;
        routes
            .drm
            .register_receiver(wire::RECEIVE_PATH, Arc::new(receive::Receiver(Arc::downgrade(&inner))))
            .map_err(RegistrationError::from)?;

        let triggers = Triggers {
            peers: env.peers.subscribe(),
            sessions: env.sessions.subscribe(),
            switches: env.switches.subscribe(),
        };
        let subscription = routes.msaep.subscribe(
            wire::PERMISSIONS_TAG,
            DeviceFilter::Any,
            Arc::new(PermissionObserver(Arc::downgrade(&inner))),
        );

        *inner.subscription.lock_or_panic() = Some(subscription);
        inner.spawn_loop(triggers);
        for peer in inner.env.peers.peers() {
            inner.probe(&peer, false, "started");
        }

        Ok(Arc::new(Self { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<FileTransferEvent> {
        self.inner.events.subscribe()
    }

    /// `phone`'s settings.
    pub fn settings(&self, phone: &DcgClient) -> Result<PhoneSettings, FileTransferError> {
        let peer = self.inner.env.peers.peer(phone).ok_or(FileTransferError::UnknownPhone)?;
        let data = self.inner.data(phone)?;
        let android = peer.info.is_android();
        let capable = share_capable(&peer);
        let config = &self.inner.config;
        let phone_enabled = self.inner.phone_enabled(phone);

        Ok(PhoneSettings {
            share_capable: capable,
            share_toggle_shown: config.share_enabled && capable,
            share_enabled: data.share_enabled,
            offered_as_send_target: config.share_enabled && capable && data.share_enabled && phone_enabled,
            folder_setting_shown: android && config.phone_to_pc_enabled && data.show_folder_setting,
            received_files_folder: self.inner.folder(&data),
        })
    }

    /// *Share* for `phone`.
    pub fn set_share_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), FileTransferError> {
        let peer = self.inner.env.peers.peer(phone).ok_or(FileTransferError::UnknownPhone)?;
        if self.inner.update(phone, |d| d.share_enabled = enabled)? {
            info!(%phone, enabled, "share setting changed");
            self.inner.changed(phone);
            self.inner.probe(&peer, false, "share setting changed");
        }
        Ok(())
    }

    /// The received-files folder for `phone`.
    pub fn set_received_files_folder(&self, phone: &DcgClient, folder: PathBuf) -> Result<(), FileTransferError> {
        self.inner.env.peers.peer(phone).ok_or(FileTransferError::UnknownPhone)?;
        let text = folder.to_string_lossy().into_owned();
        if self.inner.update(phone, |d| d.received_files_folder = Some(text))? {
            info!(%phone, "received-files folder changed");
            self.inner.changed(phone);
        }
        Ok(())
    }

    /// The transfers running with `phone`.
    pub fn transfers(&self, phone: &DcgClient) -> Transfers {
        self.inner.state.lock_or_panic().transfers.get(&key(phone)).cloned().unwrap_or_default()
    }

    /// Sends `files` to `phone` and waits for the end: the number of files the phone completed.
    pub async fn send(&self, phone: &DcgClient, files: Vec<PathBuf>, origin: SendOrigin) -> Result<u32, SendError> {
        let peer = self.inner.env.peers.peer(phone).ok_or(SendError::UnknownPhone)?;
        self.inner.clone().send(peer, files, origin).await
    }

    /// Cancels the send running to `phone`, if any.
    pub fn cancel_send(&self, phone: &DcgClient) {
        self.inner.cancel_send(phone);
    }

    /// Asks the phone for the files permission (prompting), as the grant-permission notification does.
    pub async fn request_permission(&self, phone: &DcgClient) -> Result<PermissionResult, FileTransferError> {
        let peer = self.inner.env.peers.peer(phone).ok_or(FileTransferError::UnknownPhone)?;
        Ok(self.inner.permission(&peer, false, PermissionTrigger::UserGrantFilesPermission, None, None).await)
    }
}

/// The trigger streams, subscribed before any state is read.
struct Triggers {
    peers: broadcast::Receiver<PeerEvent>,
    sessions: broadcast::Receiver<phonelink_core::seams::SessionEvent>,
    switches: broadcast::Receiver<PhoneSwitchChanged>,
}

impl Inner {
    fn emit(&self, event: FileTransferEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn changed(&self, phone: &DcgClient) {
        self.emit(FileTransferEvent::Changed { phone: phone.clone() });
    }

    fn data(&self, phone: &DcgClient) -> Result<PhoneData, StoreError> {
        store::load(&*self.store.get(&key(phone))?)
    }

    /// Applies `f` to `phone`'s row; `true` when it changed.
    fn update(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneData)) -> Result<bool, StoreError> {
        let _guard = self.data_lock.lock_or_panic();
        let db = self.store.get(&key(phone))?;
        let before = store::load(&db)?;
        let mut after = before.clone();
        f(&mut after);
        if after != before {
            store::save(&db, &after)?;
        }
        Ok(after != before)
    }

    /// The per-phone switch of the CDEH features; an unreadable switch counts as off. It is read on
    /// every settings read and request, so a failure is logged when it starts.
    fn phone_enabled(&self, phone: &DcgClient) -> bool {
        match self.env.switches.is_enabled(phone) {
            Ok(enabled) => {
                self.switch_unreadable.store(false, Ordering::Relaxed);
                enabled
            }
            Err(e) => {
                if !self.switch_unreadable.swap(true, Ordering::Relaxed) {
                    warn!(%phone, error = %e, "phone switch unreadable; counted as off");
                }
                false
            }
        }
    }

    fn folder(&self, data: &PhoneData) -> Option<PathBuf> {
        data.received_files_folder.as_ref().map(PathBuf::from).or_else(|| self.config.default_folder.clone())
    }

    /// Updates `phone`'s running transfers and announces it.
    fn set_transfers(&self, phone: &DcgClient, f: impl FnOnce(&mut Transfers)) {
        {
            let mut state = self.state.lock_or_panic();
            let entry = state.transfers.entry(key(phone)).or_default();
            f(entry);
            if entry.send.is_none() && entry.receive.is_none() {
                state.transfers.remove(&key(phone));
            }
        }
        self.changed(phone);
    }

    /// The Mac's computer name, sent as the sender of a transfer.
    fn sender_name(&self) -> String {
        phonelink_core::config::display_name_from_host(&self.env.edges.host.host_name())
    }

    // --------------------------------------------------------------------------------------------
    // Triggers and the probe
    // --------------------------------------------------------------------------------------------

    fn spawn_loop(&self, mut t: Triggers) {
        use broadcast::error::RecvError;
        let (weak, shutdown) = (self.weak.clone(), self.shutdown.clone());
        self.runtime.spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = t.peers.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PeerEvent::Added(peer)) => inner.probe(&peer, true, "phone added"),
                            Ok(PeerEvent::Removed(phone)) => inner.phone_removed(&phone),
                            Ok(PeerEvent::Updated(peer)) => {
                                if inner.env.sessions.is_connected(&peer.client) {
                                    inner.probe(&peer, false, "phone updated");
                                }
                            }
                            Err(RecvError::Lagged(n)) => {
                                warn!(skipped = n, "device-list events missed; their probes do not run");
                            }
                            Err(RecvError::Closed) => break,
                        }
                    }
                    event = t.sessions.recv() => {
                        // Availability alone registers nothing; the receive needs no session state.
                        if let Err(RecvError::Closed) = event {
                            break;
                        }
                    }
                    event = t.switches.recv() => {
                        let Some(inner) = weak.upgrade() else { break };
                        match event {
                            Ok(PhoneSwitchChanged { phone, .. }) => {
                                inner.changed(&phone);
                                if let Some(peer) = inner.env.peers.peer(&phone) {
                                    inner.probe(&peer, false, "phone switch changed");
                                }
                            }
                            Err(RecvError::Lagged(n)) => {
                                warn!(skipped = n, "phone-switch events missed; their probes do not run");
                            }
                            Err(RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    /// The `FILE_SHARE` probe of an enabled Android phone. Its reply is only logged.
    fn probe(&self, peer: &PeerDevice, force_session_start: bool, trigger: &'static str) {
        let phone = &peer.client;
        if !self.env.peers.is_enrolled() || !peer.info.is_android() || !self.phone_enabled(phone) {
            debug!(%phone, trigger, "no FILE_SHARE probe; no account, not Android, or disabled");
            return;
        }

        let (weak, peer) = (self.weak.clone(), peer.clone());
        self.runtime.spawn(async move {
            let Some(inner) = weak.upgrade() else { return };
            let outcome = inner
                .drm_send(
                    &peer,
                    ProtoDeviceResourceType::FileShare,
                    ProtoDeviceResourceRequestType::Get,
                    wire::probe_request(),
                    force_session_start,
                    wire::trace_name::PROBE,
                    None,
                    inner.shutdown.child_token(),
                )
                .await;

            let phone = &peer.client;
            match outcome {
                DrmSendOutcome::Success { payload } => match wire::probe_status(&payload) {
                    Ok(status) => info!(%phone, trigger, %status, "FILE_SHARE probe answered"),
                    Err(e) => warn!(%phone, trigger, error = %e, "FILE_SHARE probe reply does not parse"),
                },
                other => warn!(%phone, trigger, outcome = ?other, error = other.cause(), "FILE_SHARE probe failed"),
            }
        });
    }

    /// A phone left the account: its context handle goes; its notifications are withdrawn. Its later
    /// receive requests get no reply (it is no longer a phone of the account); a running transfer
    /// continues, and its settings are kept.
    fn phone_removed(&self, phone: &DcgClient) {
        self.state.lock_or_panic().handles.remove(&key(phone));
        if let Err(e) = self.env.notifier.withdraw(vec![notify::send_id(phone), notify::permission_id(phone)]) {
            warn!(%phone, error = %e, "notifications of the removed phone not withdrawn");
        }
        self.changed(phone);
    }

    /// One request through the CDEH request wrapper, addressed by type, bounded by the platform
    /// request timeout; its trace is named `names` and lies within `operation` (same trace id) when
    /// the request is part of one.
    #[expect(clippy::too_many_arguments, reason = "independent inputs of one operation; not yet grouped")]
    async fn drm_send(
        &self,
        peer: &PeerDevice,
        resource_type: ProtoDeviceResourceType,
        request_type: ProtoDeviceResourceRequestType,
        payload: Vec<u8>,
        force_session_start: bool,
        names: (&str, &str),
        operation: Option<&TraceContext>,
        cancel: CancellationToken,
    ) -> DrmSendOutcome {
        let state = phonelink_core::types::trace_state(names.0, names.1);
        let trace = match operation {
            Some(op) => op.for_request(state),
            None => TraceContext::new_root(state),
        };

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

    // --------------------------------------------------------------------------------------------
    // The files permission and the context handle
    // --------------------------------------------------------------------------------------------

    /// The files permission request: `GET` when `silent`, `SYNC` (the phone prompts) otherwise; within
    /// `operation` when a transfer sends it.
    async fn permission(
        &self,
        peer: &PeerDevice,
        silent: bool,
        trigger: PermissionTrigger,
        operation: Option<&TraceContext>,
        cancel: Option<CancellationToken>,
    ) -> PermissionResult {
        let request_type =
            if silent { ProtoDeviceResourceRequestType::Get } else { ProtoDeviceResourceRequestType::Sync };
        let cancel = cancel.unwrap_or_else(|| self.shutdown.child_token());

        let outcome = self
            .drm_send(
                peer,
                ProtoDeviceResourceType::Permissions,
                request_type,
                files_permission::request(),
                true,
                (wire::trace_name::REQUEST_PERMISSION, trigger.wire()),
                operation,
                cancel,
            )
            .await;
        PermissionResult::from_outcome(outcome)
    }

    /// The stored handle of `phone`, unless the platform layer dropped it.
    fn stored_handle(&self, phone: &DcgClient) -> Option<ContextHandle> {
        let mut state = self.state.lock_or_panic();
        match state.handles.get(&key(phone)) {
            Some(h) if !h.is_dropped() => Some(h.clone()),
            Some(_) => {
                state.handles.remove(&key(phone));
                None
            }
            None => None,
        }
    }

    /// The phone's context handle: the stored one, or the one its tag-3 publication delivers after the
    /// `GetContextHandle` request (sent within the transfer's `operation`).
    async fn ensure_handle(
        &self,
        peer: &PeerDevice,
        operation: &TraceContext,
        cancel: Option<CancellationToken>,
    ) -> Result<ContextHandle, HandleError> {
        let phone = &peer.client;
        if let Some(h) = self.stored_handle(phone) {
            return Ok(h);
        }

        let mut updated = self.handle_updated.subscribe();
        let permission =
            self.permission(peer, false, PermissionTrigger::GetContextHandle, Some(operation), cancel.clone()).await;

        let cancel = cancel.unwrap_or_default();
        let wanted = key(phone);
        let wait = async {
            loop {
                if self.stored_handle(phone).is_some() {
                    return;
                }
                match updated.recv().await {
                    Ok(p) if p == wanted => {}
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    // The feature is gone: nothing will arrive.
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                }
            }
        };

        let (timed_out, cancelled) = tokio::select! {
            _ = wait => (false, false),
            _ = tokio::time::sleep(self.config.handle_timeout) => (true, false),
            _ = cancel.cancelled() => (false, true),
        };

        if let Some(h) = self.stored_handle(phone) {
            return Ok(h);
        }
        Err(if permission != PermissionResult::Granted {
            HandleError::Permission(permission)
        } else if timed_out {
            HandleError::TimedOut
        } else if cancelled {
            HandleError::Cancelled
        } else {
            HandleError::Failed
        })
    }

    /// Tag 3. Runs on the router's task: stores the delivered handle first, then parses the payload.
    fn on_permissions(&self, publication: &Publication) {
        if self.adopt_delivered_handle(&publication.source) {
            self.parse_permissions(publication);
        }
    }

    /// Stores the handle the publication from `source` delivered; `false` when it delivered none.
    fn adopt_delivered_handle(&self, source: &DcgClient) -> bool {
        let Some(handle) = self.validator.current_handle(source) else {
            warn!(partner = %source, "permission publication without a context source; no handle");
            return false;
        };

        if self.store_delivered_handle(source, handle) {
            info!(partner = %source, "context handle updated");
            let _ = self.handle_updated.send(key(source));
        }
        true
    }

    /// Parses the permission publication of a phone of the account.
    fn parse_permissions(&self, publication: &Publication) {
        let source = &publication.source;
        if self.env.peers.peer(source).is_none() {
            debug!(partner = %source, "permission publication from a device not in the account; not parsed");
            return;
        }

        // Only checked: its content is not used, and phone files reads the same publication.
        if let Err(e) = wire::parse_publication(&publication.payload.additional) {
            debug!(partner = %source, error = %e, "permission publication does not parse");
        }
    }

    /// Stores `handle` for `source` unless the live one has the same source; `true` when it replaced it.
    fn store_delivered_handle(&self, source: &DcgClient, handle: ContextHandle) -> bool {
        let mut state = self.state.lock_or_panic();
        match state.handles.get(&key(source)) {
            Some(h) if h.same_source(&handle) && !h.is_dropped() => false,
            _ => {
                state.handles.insert(key(source), handle);
                true
            }
        }
    }
}

struct PermissionObserver(Weak<Inner>);

impl PublicationObserver for PermissionObserver {
    fn on_publication(&self, publication: &Publication) {
        if let Some(inner) = self.0.upgrade() {
            inner.on_permissions(publication);
        }
    }
}
