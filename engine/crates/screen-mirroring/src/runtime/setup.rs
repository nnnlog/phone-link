//! The set-up every window waits for: the remote and permission workflows.

use super::{Inner, PhoneArgs, Scoped, SetupFailure, SetupStage, Transaction, merged, send_on};
use crate::errors::{code, setup_code};
use crate::gates::{AppsFeatureType, PhoneValues};
use crate::link::StartedWorkflow;
use crate::machine::{PhoneCommand, SessionCommand, WindowState};
use crate::session::Session;
use crate::wire::{Body, scope, workflow};
use crate::{AudioButton, MirroringError, wire};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use phonelink_core::sync::MutexExt;
use phonelink_nano::channel::message::InboundMessage;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

impl Inner {
    /// The set-up for window `requester`: joins the running one, or starts it. With the
    /// orchestrator already open only the window's own steps run (its consent request for the phone
    /// screen, its audio permissions).
    pub(super) async fn ensure_setup(self: &Arc<Self>, requester: &str) -> Result<(), SetupFailure> {
        if self.state.lock_or_panic().orchestrator.is_some() {
            self.window_setup(requester).await;
            return Ok(());
        }
        let (fut, own) = {
            let mut st = self.state.lock_or_panic();
            match &st.setup {
                Some(f) => (f.clone(), st.setup_requester.as_deref() == Some(requester)),
                None => {
                    let cancel = CancellationToken::new();
                    st.setup_cancel = cancel.clone();
                    st.setup_stage = SetupStage::Start;
                    st.setup_requester = Some(requester.to_string());

                    let me = self.clone();
                    let who = requester.to_string();
                    info!(session = requester, "mirroring set-up started");
                    let f: BoxFuture<'static, Result<(), SetupFailure>> = Box::pin(async move {
                        tokio::select! {
                            r = me.clone().setup(who) => {
                                me.log_setup_end(&r);
                                r
                            }
                            _ = cancel.cancelled() => {
                                let code = me.state.lock_or_panic().setup_cancel_code;
                                info!(code, "mirroring set-up cancelled by a release");
                                Err(SetupFailure::Error(code))
                            }
                        }
                    });

                    let shared = f.shared();
                    st.setup = Some(shared.clone());
                    (shared, true)
                }
            }
        };

        let r = fut.clone().await;
        {
            let mut st = self.state.lock_or_panic();
            if st.setup.as_ref().is_some_and(|f| f.ptr_eq(&fut)) {
                st.setup = None;
                st.setup_requester = None;
            }
        }

        if r.is_ok() && !own {
            self.window_setup(requester).await;
        }
        r
    }

    /// The set-up steps every window runs for itself on an open set-up.
    pub(super) async fn window_setup(self: &Arc<Self>, id: &str) {
        let phone_screen = self.state.lock_or_panic().sessions.get(id).is_some_and(Session::is_phone_screen);
        if phone_screen {
            self.request_permission_if_needed();
        }
        if self.values().is_ok_and(|v| v.audio()) {
            self.clone().audio_flow(id.to_string()).await;
        }
    }

    /// The one line of a set-up's end; the windows waiting for it show the failure without one.
    fn log_setup_end(&self, r: &Result<(), SetupFailure>) {
        match r {
            Ok(()) => info!("mirroring set-up complete"),
            Err(f @ SetupFailure::Closed(code)) => info!(code, reason = %f, "mirroring set-up stopped"),
            Err(f) => {
                let ice = self.values().is_ok_and(|v| v.ice_support());
                let code = setup_code(f.code(), ice);
                warn!(code, error = %f, "mirroring set-up failed; the windows show the error");
            }
        }
    }

    pub(super) async fn setup(self: Arc<Self>, requester: String) -> Result<(), SetupFailure> {
        let values = self.values().map_err(|_| SetupFailure::Error(code::DEVICE_UNREACHABLE))?;
        if !self.settings().allow_apps {
            return Err(SetupFailure::Closed(code::PC_PERMISSION));
        }

        if values.nearby() {
            self.state.lock_or_panic().setup_stage = SetupStage::Nearby;
            self.clone().nearby_check().await?;
        }

        self.state.lock_or_panic().setup_stage = SetupStage::Permission;
        let phone_screen = self.state.lock_or_panic().sessions.get(&requester).is_some_and(Session::is_phone_screen);
        if phone_screen {
            self.request_permission_if_needed();
        }
        self.clone().screen_permission().await?;

        if values.audio() {
            self.state.lock_or_panic().setup_stage = SetupStage::Audio;
            self.clone().audio_flow(requester).await;
        }

        self.state.lock_or_panic().setup_stage = SetupStage::Remote;
        self.open_remote(values).await
    }

    /// The phone's screen-capture consent: the permission workflow (started when not open), a
    /// request unless the phone accepted on it, and the wait for `accept` (no time limit).
    pub(super) async fn screen_permission(self: Arc<Self>) -> Result<(), SetupFailure> {
        let audio = self.values().is_ok_and(|v| v.audio());
        let channel = self.state.lock_or_panic().permission.clone();
        let channel = match channel {
            Some(c) => c,
            None => {
                let started = self.links.start(workflow::PERMISSION, wire::PERMISSION_INBOUND, false, audio).await?;
                let c = started.channel.clone();
                self.state.lock_or_panic().permission = Some(c.clone());
                let inbound = merged(started.inbound, c.closed());
                let me = self.clone();
                self.spawn(async move { me.permission_inbound(inbound).await });
                c
            }
        };

        let mut seen = self.permission_signal.subscribe();
        seen.mark_unchanged();
        if self.state.lock_or_panic().permission_status.as_deref() == Some("accept") {
            return Ok(());
        }

        channel.send(scope::REQUEST_PERMISSION, &wire::request_permission()).map_err(|error| {
            SetupFailure::NotSent { code: code::DISCONNECTED_UNKNOWN, route: scope::REQUEST_PERMISSION, error }
        })?;
        info!("screen capture asked of the phone");

        // The wait ends with `accept`, or when a release cancels the set-up.
        loop {
            if self.state.lock_or_panic().permission_status.as_deref() == Some("accept") {
                return Ok(());
            }
            if seen.changed().await.is_err() {
                return Err(SetupFailure::Error(code::DISCONNECTED_UNKNOWN));
            }
        }
    }

    pub(super) async fn permission_inbound(self: Arc<Self>, mut inbound: mpsc::UnboundedReceiver<Scoped>) {
        while let Some(Scoped { scope: registered, m }) = inbound.recv().await {
            self.permission_message(&registered, &m);
        }
        debug!("permission channel closed");
    }

    /// One message on the permission workflow's channel, received at the scope `registered`.
    fn permission_message(&self, registered: &str, m: &InboundMessage) {
        // `permissionStatus` is taken only at its own target; the audio answers at whatever
        // target their scope receives.
        if registered == scope::PERMISSION_STATUS && m.target != scope::PERMISSION_STATUS {
            warn!(raw_target = %m.target, "permission message without a handler; dropped");
            return;
        }

        let body = match Body::parse(&m.body) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, route = registered, "malformed permission message; dropped");
                return;
            }
        };

        match registered {
            scope::PERMISSION_STATUS => self.on_permission_status(&body),
            _ => match body.opt_str("status").as_deref() {
                Some("accept" | "deny" | "disabled") => {
                    self.complete_transaction(body.opt_str("transactionId").as_deref(), &m.body)
                }
                other => {
                    warn!(raw_status = ?other, route = registered, "audio permission status not known; dropped")
                }
            },
        }
    }

    /// `permissionStatus`: the phone's answer about screen capture.
    fn on_permission_status(&self, body: &Body) {
        let Some(status) = body.opt_str("status") else {
            warn!("permissionStatus without a status; dropped");
            return;
        };

        self.state.lock_or_panic().permission_status = Some(status.clone());
        if status == "accept" {
            info!("screen capture accepted by the phone");
        } else {
            info!(raw_status = %status, "screen capture not accepted by the phone");
            self.phone_command(PhoneCommand::ScreenScrapePermissionNeeded, PhoneArgs::default());
        }

        self.permission_signal.send_modify(|n| *n += 1);
    }

    /// One answer of the phone about an audio permission: asked once per permission workflow (the
    /// check and the prompt), unless `fresh`. `None` when the workflow is not open or closed.
    pub(super) async fn audio_ask(
        self: &Arc<Self>,
        target: &'static str,
        permission: &'static str,
        fresh: bool,
    ) -> Option<String> {
        if !fresh {
            if let Some(a) = self.state.lock_or_panic().audio_answers.get(&(target, permission)) {
                return Some(a.clone());
            }
        }

        let channel = self.state.lock_or_panic().permission.clone()?;
        let (tx, rx) = self.begin_transaction(Transaction::Other);
        if channel.send(target, &wire::permission_transaction(permission, &tx)).is_err() {
            self.state.lock_or_panic().transactions.remove(&tx);
            return None;
        }

        // The permission channel takes only answers whose status is one of the three.
        let status = Body::parse(&rx.await.ok()?).ok()?.opt_str("status")?;
        info!(permission, route = target, permission_status = audio_status(&status), "audio permission answered");
        self.state.lock_or_panic().audio_answers.insert((target, permission), status.clone());
        Some(status)
    }

    /// A prompt for `permission` ended with `status` (not `deny`): the flows waiting on a refused
    /// prompt of it end with that answer.
    pub(super) fn audio_granted(&self, permission: &str, status: &str) {
        self.audio_signal.send_modify(|(oem, apps)| {
            let slot = if permission == "oemAudio" { oem } else { apps };
            *slot = Some(status.to_string());
        });
    }

    /// The audio permissions for window `id`: `oemAudio`, and `audio` with app windows. A refusal
    /// shows *audio permission needed* and asks the phone to prompt; a refused prompt waits until a
    /// later prompt's answer ends it or the user continues without audio. Then the window's flow is
    /// complete, with the *enable audio* button when its own permission (`oemAudio` for the phone
    /// screen, else `audio`) ended other than `accept`. The window shows no audio button while the
    /// flow runs.
    pub(super) async fn audio_flow(self: Arc<Self>, id: String) {
        let cancel = self.state.lock_or_panic().audio_cancel.clone();
        let me = self.clone();
        let sid = id.clone();
        self.set_window_audio_button(&id, AudioButton::None);

        let flow = async move {
            let mut permissions = vec!["oemAudio"];
            if me.apps_feature_type() == AppsFeatureType::MultipleApps {
                permissions.push("audio");
            }

            let mut statuses = Vec::new();
            for p in &permissions {
                statuses.push(me.audio_ask(scope::CHECK_PERMISSION, p, false).await?);
            }

            let mut needed = false;
            let mut ended = Vec::new();
            for (p, mut status) in permissions.into_iter().zip(statuses) {
                if status == "deny" {
                    if !needed {
                        me.session_command(&sid, SessionCommand::AudioStreamingPermissionNeeded, None);
                        needed = true;
                    }
                    status = me.audio_ask(scope::SHOW_PERMISSION, p, false).await?;
                }
                if status == "deny" {
                    let mut answered = me.audio_signal.subscribe();
                    let slot =
                        |s: &(Option<String>, Option<String>)| if p == "oemAudio" { s.0.clone() } else { s.1.clone() };
                    status = answered.wait_for(|s| slot(s).is_some()).await.ok().and_then(|s| slot(&s))?;
                } else {
                    me.audio_granted(p, &status);
                }
                ended.push((p, status));
            }
            Some(ended)
        };

        tokio::select! {
            r = flow => if let Some(ended) = r {
                self.audio_flow_complete(&id, &ended);
            } else {
                debug!(session = %id, "audio permission flow ended unanswered; the permission workflow closed");
            },
            _ = cancel.cancelled() => debug!(session = %id, "audio permission flow cancelled"),
        }
    }

    /// Window `id`'s audio permission flow ended with the statuses `ended`.
    fn audio_flow_complete(&self, id: &str, ended: &[(&str, String)]) {
        let phone_screen = self.state.lock_or_panic().sessions.get(id).is_some_and(Session::is_phone_screen);
        let own = ended
            .iter()
            .find(|(p, _)| *p == "audio" && !phone_screen)
            .or_else(|| ended.iter().find(|(p, _)| *p == "oemAudio"))
            .map(|(_, s)| s.as_str());

        let button = if own == Some("accept") { AudioButton::None } else { AudioButton::EnableAudio };
        self.set_window_audio_button(id, button);
        self.session_command(id, SessionCommand::AudioStreamingPermissionFlowComplete, None);
    }

    /// Window `id`'s audio permission answered `status` to *open settings* or *enable audio*:
    /// `accept` completes the flow; `deny` shows *audio permission needed*; `disabled` shows *audio
    /// always denied* when `deny_takeover` (the recheck), else completes the flow with the *enable
    /// audio* button. `false` when the window is gone.
    pub(super) fn audio_permission_answered(&self, id: &str, status: &str, deny_takeover: bool) -> bool {
        if !self.state.lock_or_panic().sessions.contains_key(id) {
            debug!(
                session = id,
                permission_status = audio_status(status),
                "audio permission answer for a closed window; ignored"
            );
            return false;
        }

        let (button, command) = match status {
            "accept" => (AudioButton::None, SessionCommand::AudioStreamingPermissionFlowComplete),
            "deny" => (AudioButton::None, SessionCommand::AudioStreamingPermissionNeeded),
            _ if deny_takeover => (AudioButton::None, SessionCommand::AudioStreamingPermissionAlwaysDenied),
            _ => (AudioButton::EnableAudio, SessionCommand::AudioStreamingPermissionFlowComplete),
        };

        self.set_window_audio_button(id, button);
        self.session_command(id, command, None);
        true
    }

    /// The *enable audio* button of window `id`: its permission (`oemAudio` for the phone screen,
    /// else `audio`) is checked afresh — `deny` then prompts on the phone afresh. Other windows are
    /// not asked.
    pub(in super::super) async fn enable_audio(self: Arc<Self>, id: &str) -> Result<(), MirroringError> {
        if !self.values()?.audio() {
            return Err(MirroringError::NotSupported("the phone streams no audio"));
        }

        let phone_screen = {
            let st = self.state.lock_or_panic();
            let s = st.sessions.get(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            if st.permission.is_none() {
                return Err(MirroringError::NotNow("the permission workflow is not open"));
            }
            s.is_phone_screen()
        };

        let permission = if phone_screen {
            "oemAudio"
        } else if self.apps_feature_type() == AppsFeatureType::MultipleApps {
            "audio"
        } else {
            return Err(MirroringError::NotSupported("app windows have no audio permission of their own"));
        };

        self.state.lock_or_panic().audio_answers.retain(|(_, p), _| *p != permission);
        let closed = || MirroringError::NotNow("the permission workflow closed");
        let status = self.audio_ask(scope::CHECK_PERMISSION, permission, true).await.ok_or_else(closed)?;
        if self.audio_permission_answered(id, &status, true) && status == "deny" {
            let status = self.audio_ask(scope::SHOW_PERMISSION, permission, true).await.ok_or_else(closed)?;
            self.audio_permission_answered(id, &status, false);
        }
        Ok(())
    }

    /// The user continues without audio: the audio permission flows stop and the window goes on.
    pub(in super::super) fn continue_without_audio(&self, id: &str) -> Result<(), MirroringError> {
        if !self.state.lock_or_panic().sessions.contains_key(id) {
            return Err(MirroringError::UnknownSession(id.to_string()));
        }
        {
            let mut st = self.state.lock_or_panic();
            st.audio_cancel.cancel();
            st.audio_cancel = CancellationToken::new();
        }
        self.session_command(id, SessionCommand::AudioStreamingPermissionFlowComplete, None);
        Ok(())
    }

    /// The audio-permission window's buttons, for the window that shows the request: open the
    /// phone's settings (`launchSettings` for its permission, the answer sets the window), or prompt
    /// on the phone again (the flow runs afresh). Either then runs the flow of the other app windows.
    pub(in super::super) async fn request_audio_permission(
        self: Arc<Self>,
        open_settings: bool,
    ) -> Result<(), MirroringError> {
        if !self.values()?.audio() {
            return Err(MirroringError::NotSupported("the phone streams no audio"));
        }

        let requester = {
            let st = self.state.lock_or_panic();
            if st.permission.is_none() {
                return Err(MirroringError::NotNow("the permission workflow is not open"));
            }

            let mut v: Vec<&Session> = st.sessions.values().collect();
            v.sort_by_key(|s| s.order);
            let asking = v.iter().find(|s| {
                matches!(
                    s.machine.state,
                    WindowState::AudioStreamingPermissionNeeded | WindowState::AudioStreamingPermissionAlwaysDenied
                )
            });
            asking.or(v.first()).map(|s| (s.id.clone(), s.is_phone_screen()))
        };

        let Some((id, phone_screen)) = requester else {
            return Err(MirroringError::NotNow("no window"));
        };

        let permission = if phone_screen { "oemAudio" } else { "audio" };
        if open_settings {
            self.state.lock_or_panic().audio_answers.retain(|(_, p), _| *p != permission);
            let Some(status) = self.audio_ask(scope::LAUNCH_SETTINGS, permission, true).await else {
                return Err(MirroringError::NotNow("the permission workflow closed"));
            };
            if status == "accept" {
                self.audio_granted(permission, &status);
            }
            self.audio_permission_answered(&id, &status, false);
        } else {
            self.state.lock_or_panic().audio_answers.clear();
            self.clone().audio_flow(id.clone()).await;
        }

        if !phone_screen {
            let others: Vec<String> = self
                .state
                .lock_or_panic()
                .sessions
                .values()
                .filter(|s| !s.is_phone_screen() && s.id != id)
                .map(|s| s.id.clone())
                .collect();
            for o in others {
                self.clone().audio_flow(o).await;
            }
        }
        Ok(())
    }

    pub(super) async fn open_remote(self: Arc<Self>, values: PhoneValues) -> Result<(), SetupFailure> {
        self.state.lock_or_panic().remote_started = true;
        self.phone_command(PhoneCommand::EnsureConnection, PhoneArgs::default());

        let started = match self.links.begin(workflow::REMOTE, false, values.audio()).await {
            Ok(pending) => {
                // The `audio` workflow starts as soon as `remote` is asked for, not once it opens.
                if values.audio() && !self.links.is_live(workflow::AUDIO) {
                    let me = self.clone();
                    self.spawn(async move { me.open_audio().await });
                }
                pending.open(wire::ORCHESTRATOR_INBOUND).await
            }
            Err(e) => Err(e),
        };
        let started = match started {
            Ok(s) => s,
            Err(e) => {
                self.state.lock_or_panic().remote_started = false;
                return Err(e.into());
            }
        };

        let orchestrator = started.channel.clone();
        let settings = self.settings();
        let geometry = self.presenter.display_geometry();
        let feature_type = values.apps_feature_type(self.separate_window());
        let fps = if self.config.max_fps_60 {
            60
        } else if feature_type == AppsFeatureType::PhoneScreenOnly {
            30
        } else {
            0
        };
        let config = wire::ConfigValues {
            max_main_display_fps: fps,
            apps_feature_type: feature_type as i64,
            width: geometry.usable_width.into(),
            height: geometry.usable_height.into(),
            dpi: geometry.dpi.into(),
            host_display_width: geometry.width.into(),
            host_display_height: geometry.height.into(),
        };

        let channel = Some(orchestrator.clone());
        send_on(&channel, scope::CONFIG, &wire::config(&config));
        send_on(&channel, scope::ALWAYS_EXPANDED, &wire::always_expanded(settings.always_expanded));
        self.state.lock_or_panic().orchestrator = Some(orchestrator.clone());

        let StartedWorkflow { inbound, others, .. } = started;
        let inbound = merged(inbound, orchestrator.closed());
        let me = self.clone();
        self.spawn(async move { me.orchestrator_inbound(orchestrator, inbound).await });
        let me = self.clone();
        self.spawn(async move { me.window_channels(others).await });

        self.phone_command(PhoneCommand::OnConnected, PhoneArgs::default());
        Ok(())
    }
}

/// An audio permission status the permission channel took, as the constant it equals.
fn audio_status(status: &str) -> &'static str {
    match status {
        "accept" => "accept",
        "deny" => "deny",
        "disabled" => "disabled",
        _ => "other",
    }
}
