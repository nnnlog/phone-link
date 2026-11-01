//! The camera stream: starting, receiving and stopping it.

use super::{
    Action, Applied, CameraConfig, CameraError, CameraFace, CameraState, ConnectedCameraError, Effect, HandleFailure,
    Inbound, Inner, OpenedStream, PhoneRuntime, RetrieveError, Session, StreamChannel, StreamInbound, StreamOpenError,
    device_id, key, low_network, wire,
};
use phonelink_core::edge::LockState;
use phonelink_core::seams::PlatformRequestStatus;
use phonelink_core::sync::MutexExt;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_nano::device::AcceptanceReason;
use phonelink_proto::platform_sdk::v1::{ProtoDeviceResourceRequestType, ProtoDeviceResourceType};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{ContextHandle, DrmSendOutcome};
use std::fmt::Display;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use wire::ActionMessage;

impl Inner {
    /// Opens the stream again unless none is wanted.
    pub(super) fn refresh(&self, peer: &PeerDevice) {
        if !self.with_phone(&peer.client, |p| p.state).is_idle() {
            self.spawn_open_stream(peer.clone());
        }
    }

    pub(super) fn spawn_open_stream(&self, peer: PeerDevice) {
        let weak = self.weak.clone();
        self.runtime.spawn(async move {
            if let Some(inner) = weak.upgrade() {
                inner.open_stream(peer).await;
            }
        });
    }

    pub(super) async fn open_stream(self: Arc<Self>, peer: PeerDevice) {
        let phone = &peer.client;
        let Some((id, cancel)) = self.begin_session(phone) else { return };

        info!(%phone, "opening the camera stream");
        self.set_state(phone, CameraState::Connecting, CameraError::None);
        self.set_control_window(phone, true);

        let data = self.data_or_default(phone);
        if self.permission_required(&peer, &data) {
            self.fail_on_missing_permission(&peer, id).await;
            return;
        }

        let Some(handle) = self.handle_or_fail(&peer, id, &cancel).await else { return };
        if self.env.edges.lock.lock_state() != LockState::Unlocked {
            self.fail_session(
                phone,
                id,
                CameraError::ChannelNotCreated,
                Some(&"the Mac is locked or its lock state is unknown"),
            );
            return;
        }

        let Some(opened) = self.open_channel(phone, id, &handle, &cancel).await else { return };
        let attached = self.with_phone(phone, |p| match &mut p.session {
            Some(s) if s.id == id && !s.cancel.is_cancelled() => {
                s.channel = Some(opened.channel.clone());
                true
            }
            _ => false,
        });
        if !attached {
            opened.channel.close();
            return;
        }

        info!(%phone, "camera stream channel open");
        self.run_session(peer, id, cancel, opened).await;
    }

    /// Starts a new session for a registered phone without an open channel, cancelling the previous one;
    /// `None` when no stream is to be opened.
    fn begin_session(&self, phone: &DcgClient) -> Option<(u64, CancellationToken)> {
        let mut state = self.state.lock_or_panic();
        state.next_session += 1;
        let id = state.next_session;
        let p = state.phones.entry(key(phone)).or_insert_with(PhoneRuntime::new);

        if !p.registered {
            None
        } else if p.session.as_ref().is_some_and(|s| s.channel.is_some()) {
            debug!(%phone, "a stream channel is open; not opened again");
            None
        } else {
            if let Some(s) = p.session.take() {
                s.cancel.cancel();
            }
            let cancel = self.shutdown.child_token();
            p.session = Some(Session { id, cancel: cancel.clone(), channel: None });
            // A new stream logs its repeating failures again.
            p.decode_panic_logged = false;
            p.metrics_failure_logged = false;
            Some((id, cancel))
        }
    }

    /// Ends session `id` for a missing permission, auto-prompting the phone first when configured.
    async fn fail_on_missing_permission(&self, peer: &PeerDevice, id: u64) {
        let phone = &peer.client;
        if self.config.auto_prompt {
            let outcome = self
                .drm_request(
                    peer,
                    ProtoDeviceResourceType::CameraMetadata,
                    ProtoDeviceResourceRequestType::Update,
                    wire::AUTO_PROMPT_PAYLOAD.to_vec(),
                    true,
                    wire::trace_name::AUTO_PROMPT,
                )
                .await;

            let error = match &outcome {
                DrmSendOutcome::Success { payload } if wire::auto_prompt_permanently_denied(payload) => {
                    CameraError::CameraPermissionPermanentlyDenied
                }
                DrmSendOutcome::Success { .. } => CameraError::CameraPermissionNeededAutoprompt,
                DrmSendOutcome::Request(PlatformRequestStatus::RemoteMeteredSyncDisabled) => {
                    CameraError::MeteredConnection
                }
                DrmSendOutcome::Request(PlatformRequestStatus::RemoteAllSyncDisabled) => CameraError::LtwDisabled,
                _ => CameraError::ChannelDisconnected,
            };

            match &outcome {
                // The phone answered: the error it selects is the permission state, not a failure.
                DrmSendOutcome::Success { .. } => self.fail_session(phone, id, error, None),
                outcome => {
                    self.fail_session(
                        phone,
                        id,
                        error,
                        Some(&format_args!("camera auto-prompt not answered: {outcome:?}")),
                    );
                }
            }
        } else {
            self.fail_session(phone, id, CameraError::CameraPermissionNeeded, None);
        }
    }

    /// The phone's context handle; `None` (ending session `id` unless cancelled) when there is none.
    async fn handle_or_fail(&self, peer: &PeerDevice, id: u64, cancel: &CancellationToken) -> Option<ContextHandle> {
        let phone = &peer.client;
        match self.retrieve_handle(peer, cancel).await {
            Ok(h) => Some(h),
            Err(RetrieveError::Cancelled) => None,
            Err(e) => {
                let error = match e {
                    RetrieveError::Failed(HandleFailure::Metered) => CameraError::MeteredConnection,
                    RetrieveError::Failed(HandleFailure::AllSyncDisabled) => CameraError::LtwDisabled,
                    _ => CameraError::ChannelDisconnected,
                };
                self.fail_session(phone, id, error, Some(&e));
                None
            }
        }
    }

    /// Opens the stream channel over `handle`; `None` when cancelled or it did not open.
    async fn open_channel(
        &self,
        phone: &DcgClient,
        id: u64,
        handle: &ContextHandle,
        cancel: &CancellationToken,
    ) -> Option<OpenedStream> {
        // The creation request carries the trace's ids only.
        let trace = TraceContext::new_root(Vec::new());
        let opened = tokio::select! {
            r = self.channels.open(handle, &trace) => r,
            _ = cancel.cancelled() => return None,
        };

        match opened {
            Ok(o) => Some(o),
            Err(StreamOpenError::Open(e)) => {
                // As Phone Link on Windows: a channel that did not open leaves the camera connecting.
                warn!(%phone, error = %e, "camera stream channel did not open; the camera stays connecting");
                None
            }
            Err(e) => {
                let error = match e.acceptance() {
                    Some(AcceptanceReason::MobileDataDisabled) => CameraError::MeteredConnection,
                    Some(AcceptanceReason::AppDisabled) => CameraError::LtwDisabled,
                    _ => CameraError::ChannelDisconnected,
                };
                self.fail_session(phone, id, error, Some(&e));
                None
            }
        }
    }

    /// Ends session `id` (when still current) in error `error`; `cause` is the PC's own failure that
    /// ended it, `None` when the error is the phone's permission state.
    pub(super) fn fail_session(&self, phone: &DcgClient, id: u64, error: CameraError, cause: Option<&dyn Display>) {
        let current = self.with_phone(phone, |p| match &p.session {
            Some(s) if s.id == id => {
                p.session = None;
                true
            }
            _ => false,
        });

        if current {
            match cause {
                Some(cause) => self.set_failed_state(phone, error, cause),
                None => self.set_state(phone, CameraState::Error, error),
            }
        }
    }

    pub(super) async fn run_session(
        &self,
        peer: PeerDevice,
        id: u64,
        cancel: CancellationToken,
        mut opened: OpenedStream,
    ) {
        let phone = &peer.client;
        let channel = opened.channel.clone();
        let mut network: Option<tokio::time::Interval> = None;

        loop {
            let tick = async {
                match network.as_mut() {
                    Some(i) => {
                        i.tick().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                _ = cancel.cancelled() => break,
                item = opened.inbound.recv() => match item {
                    Some(StreamInbound::Message { scope, body }) => {
                        if self.on_message(&peer, &channel, &scope, &body) && network.is_none() {
                            network = Some(self.start_network(phone, &channel));
                        }
                    }
                    Some(StreamInbound::Frame(f)) => self.on_frame(phone, id, &channel, f).await,
                    Some(StreamInbound::Closed) | None => {
                        self.on_channel_closed(phone, id);
                        break;
                    }
                },
                _ = tick => self.check_network(phone, &channel),
            }
        }
    }

    /// One control message; `true` for the phone's `Start`.
    pub(super) fn on_message(
        &self,
        peer: &PeerDevice,
        channel: &Arc<dyn StreamChannel>,
        scope: &str,
        body: &[u8],
    ) -> bool {
        let phone = &peer.client;
        let message = match wire::inbound(scope, body) {
            Ok(m) => m,
            Err(e) => {
                warn!(%phone, raw_scope = %scope, error = %e, "camera control message does not parse; dropped");
                return false;
            }
        };

        match message {
            Inbound::Action(a) => return self.on_action(phone, channel, &a),
            Inbound::Status(s) => {
                if s.current_status == wire::status::TAP_NOTIF_TO_START {
                    self.set_state(phone, CameraState::StartStreamNotification, CameraError::None);
                }
            }
            Inbound::Battery(b) => {
                if self.update_logged(phone, |d| {
                    d.battery_level = b.battery_level;
                    d.is_charging = b.is_charging;
                }) {
                    self.changed(phone);
                }
            }
            Inbound::Configuration(_) => debug!(%phone, "camera configuration message; not used"),
            Inbound::Network(_) => debug!(%phone, "network message from the phone; not used"),
        }
        false
    }

    /// One action from the phone; `true` for its `Start`.
    fn on_action(&self, phone: &DcgClient, channel: &Arc<dyn StreamChannel>, a: &ActionMessage) -> bool {
        let Some(action) = Action::from_wire(a.action) else {
            warn!(%phone, action = a.action, "unknown camera action; dropped");
            return false;
        };
        debug!(%phone, ?action, "camera action from the phone");

        let needs_config = matches!(action, Action::Initialize | Action::Start | Action::Update);
        if needs_config && a.camera_config_payload.is_none() {
            warn!(%phone, ?action, "camera action without its configuration; dropped");
            return false;
        }

        if let Some(config) = &a.camera_config_payload {
            if needs_config {
                self.apply_config(phone, config);
            }
        }

        let error = a.camera_error.map(CameraError::from_wire);
        self.apply_action(phone, channel, action, error)
    }

    /// Applies a known action carrying `error`; `true` for `Start`.
    fn apply_action(
        &self,
        phone: &DcgClient,
        channel: &Arc<dyn StreamChannel>,
        action: Action,
        error: Option<CameraError>,
    ) -> bool {
        match action {
            Action::Initialize => self.set_state(phone, CameraState::Connecting, CameraError::None),
            Action::Start => {
                channel.start_video();
                return true;
            }
            Action::Pause => self.set_state(phone, CameraState::Paused, error.unwrap_or(CameraError::None)),
            Action::Resume => self.set_state(phone, CameraState::Streaming, CameraError::None),
            Action::Stop => {
                self.set_control_window(phone, false);
                self.close_stream(phone);
            }
            Action::Error => match error {
                Some(
                    e @ (CameraError::CallInProgress
                    | CameraError::ScreenLocked
                    | CameraError::DeviceHeat
                    | CameraError::Disconnected
                    | CameraError::CameraDeviceFatal),
                ) => self.set_state(phone, CameraState::TerminalError, e),
                e => self.set_state(phone, CameraState::Error, e.unwrap_or(CameraError::Unknown)),
            },
            Action::Update => {}
            Action::Unknown => {}
        }
        false
    }

    /// Applies the phone's configuration: its face, the effect list and the selection.
    pub(super) fn apply_config(&self, phone: &DcgClient, c: &CameraConfig) {
        let mut effects = vec![Effect::None];
        match c.supported_oem_extensions.as_ref().filter(|o| !o.is_empty()) {
            Some(oem) => effects.extend(oem.iter().cloned().map(Effect::Oem)),
            None => effects.extend(
                wire::DEFAULT_EXTENSIONS
                    .iter()
                    .filter(|f| c.supported_default_extensions & **f != 0)
                    .map(|f| Effect::BuiltIn(*f)),
            ),
        }

        let selected = match &c.selected_oem_extension {
            Some(o) => Effect::Oem(o.clone()),
            None if c.selected_default_extension == 0 => Effect::None,
            None => Effect::BuiltIn(c.selected_default_extension),
        };

        self.with_phone(phone, |p| {
            p.applied = Applied { face: CameraFace::from_wire(c.camera_face), effects, selected: Some(selected) };
        });
        self.changed(phone);
    }

    pub(super) async fn on_frame(
        &self,
        phone: &DcgClient,
        id: u64,
        channel: &Arc<dyn StreamChannel>,
        frame: phonelink_nano::channel::video::VideoFrame,
    ) {
        let state = self.with_phone(phone, |p| p.state);
        let show = !matches!(state, CameraState::Paused | CameraState::TerminalError);
        let (camera, device) = (self.camera.clone(), device_id(phone));

        let decoded = match tokio::task::spawn_blocking(move || camera.video_frame(device, frame.data, show)).await {
            Ok(decoded) => decoded,
            Err(e) => {
                if !self.with_phone(phone, |p| std::mem::replace(&mut p.decode_panic_logged, true)) {
                    error!(%phone, error = %e, "camera frame decode panicked; the frame is dropped, repeats in this stream are not logged");
                }
                false
            }
        };
        if !decoded {
            return;
        }

        channel.frame_presented(frame.frame_id, phonelink_nano::channel::video::presented_time_us());

        let current = self.with_phone(phone, |p| p.session.as_ref().is_some_and(|s| s.id == id));
        if current {
            let state = self.with_phone(phone, |p| p.state);
            if !matches!(state, CameraState::Streaming | CameraState::Paused | CameraState::TerminalError)
                && !state.is_idle()
            {
                self.set_state(phone, CameraState::Streaming, CameraError::None);
            }
        }
    }

    /// Starts watching the network: the first reading is shown, not sent.
    pub(super) fn start_network(&self, phone: &DcgClient, channel: &Arc<dyn StreamChannel>) -> tokio::time::Interval {
        let low = channel.realtime_metrics().and_then(|m| low_network(&m, self.config.low_loss_rate));
        self.with_phone(phone, |p| p.low_network = Some(low.unwrap_or(false)));
        self.changed(phone);

        let poll = self.config.network_poll;
        tokio::time::interval_at(tokio::time::Instant::now() + poll, poll)
    }

    pub(super) fn check_network(&self, phone: &DcgClient, channel: &Arc<dyn StreamChannel>) {
        let Some(metrics) = channel.realtime_metrics() else { return };
        let Some(low) = low_network(&metrics, self.config.low_loss_rate) else {
            if !self.with_phone(phone, |p| std::mem::replace(&mut p.metrics_failure_logged, true)) {
                warn!(%phone, raw_metrics = %metrics, "connection metrics do not parse; the network quality stays, repeats in this stream are not logged");
            }
            return;
        };

        let changed = self.with_phone(phone, |p| p.low_network.replace(low) != Some(low));
        if changed {
            info!(%phone, is_low = low, "camera network quality changed");
            self.changed(phone);
            if let Err(e) = channel.send(wire::scope::NETWORK, &wire::network(low)) {
                warn!(%phone, is_low = low, error = %e, "network message not sent; the phone keeps its network state");
            }
        }
    }

    /// The channel closed under session `id`.
    pub(super) fn on_channel_closed(&self, phone: &DcgClient, id: u64) {
        let current = self.with_phone(phone, |p| match &p.session {
            Some(s) if s.id == id => {
                p.session = None;
                p.low_network = None;
                true
            }
            _ => false,
        });
        if !current {
            return;
        }

        self.invalidate_handle(phone, None);
        self.camera.reset_decoder(device_id(phone));
        self.set_failed_state(phone, CameraError::ChannelDisconnected, &"the stream channel closed");
    }

    /// Closes the stream without a message: the phone sees the channel close and stops.
    pub(super) fn close_stream(&self, phone: &DcgClient) {
        let (session, registered) = self.with_phone(phone, |p| {
            p.low_network = None;
            (p.session.take(), p.registered)
        });

        if let Some(s) = session {
            s.cancel.cancel();
            if let Some(c) = s.channel {
                c.close();
            }
            self.camera.reset_decoder(device_id(phone));
            info!(%phone, "camera stream closed");
        }

        if registered {
            self.set_state(phone, CameraState::Registered, CameraError::None);
        }
    }

    /// Sends an action on the open stream channel.
    pub(super) fn send_action(
        &self,
        phone: &DcgClient,
        action: Action,
        error: Option<CameraError>,
        config: Option<CameraConfig>,
    ) -> Result<(), ConnectedCameraError> {
        let channel = self
            .with_phone(phone, |p| p.session.as_ref().and_then(|s| s.channel.clone()))
            .ok_or(ConnectedCameraError::NoStream)?;

        if let Err(e) = channel.send(wire::scope::ACTION, &wire::action(action, error, config)) {
            warn!(%phone, ?action, error = %e, "camera action not sent; the phone does not hear it");
        }
        Ok(())
    }
}
