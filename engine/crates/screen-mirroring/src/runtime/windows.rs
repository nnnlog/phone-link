//! Each window's channel set: binding, opening and its messages.

use super::{Inner, Scoped, merged, send_on};
use crate::input::InputSink;
use crate::machine::{Details, SessionCommand, WindowState};
use crate::session::{Bound, ChannelSet, Grouped, Session};
use crate::wire::{Body, DisconnectingReason, PHONE_SCREEN_PACKAGE, scope};
use crate::{ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_nano::channel::blob::BlobChannel;
use phonelink_nano::channel::input::InputSource;
use phonelink_nano::channel::message::MessageChannel;
use phonelink_nano::channel::video::{VideoEvent, VideoTarget};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

impl Inner {
    pub(super) async fn window_channels(self: Arc<Self>, mut others: super::super::link::WorkflowChannels) {
        while let Some(c) = others.next().await {
            let grouped = self.state.lock_or_panic().grouping.offer(c);
            if let Grouped::Complete { session_id, set } = grouped {
                self.bind(session_id, *set);
            }
        }
        debug!("remote workflow channels ended");
    }

    pub(super) fn bind(self: &Arc<Self>, session_id: String, set: ChannelSet) {
        let stop = {
            let mut st = self.state.lock_or_panic();
            let Some(s) = st.sessions.get_mut(&session_id) else {
                warn!(session = %session_id, "window channel set for no window; closed");
                set.close();
                return;
            };
            s.unbind();
            s.stop.child_token()
        };

        info!(session = %session_id, "window channel set complete");
        let me = self.clone();
        self.spawn(async move { me.open_set(session_id, set, stop).await });
    }

    pub(super) async fn open_set(self: Arc<Self>, id: String, set: ChannelSet, stop: CancellationToken) {
        let ChannelSet { message, video, input, blob, .. } = set;
        let (message, video, input, blob) =
            (message.expect("complete"), video.expect("complete"), input.expect("complete"), blob.expect("complete"));

        let scopes: Vec<String> = wire::PER_APP_INBOUND.iter().map(|s| s.to_string()).collect();
        let (m, v, i, b) = tokio::join!(
            MessageChannel::open_listening(message, &scopes),
            VideoTarget::open(video),
            InputSource::open(input),
            BlobChannel::open(blob),
        );

        let errors = [
            m.as_ref().err().map(|e| format!("message: {e}")),
            v.as_ref().err().map(|e| format!("video: {e}")),
            i.as_ref().err().map(|e| format!("input: {e}")),
            b.as_ref().err().map(|e| format!("blob: {e}")),
        ];
        let (Ok((m, inbound)), Ok(v), Ok(mut i), Ok(b)) = (m, v, i, b) else {
            let error = errors.into_iter().flatten().collect::<Vec<_>>().join("; ");
            warn!(session = %id, %error, "window channels did not open; the set counts as closed");
            return;
        };

        let message = Arc::new(m);
        let (input_pipe, ready_notify) = {
            let mut st = self.state.lock_or_panic();
            let Some(s) = st.sessions.get_mut(&id) else {
                message.close();
                v.close();
                i.close();
                b.close();
                return;
            };

            let blob = Arc::new(b);
            if let Some(d) = &s.drag {
                d.set_channels(Some((message.clone(), blob.clone())));
            }
            s.bound = Some(Bound { message: message.clone(), blob, stop: stop.clone() });
            (s.input.clone(), s.window_ready_notify.clone())
        };

        let me = self.clone();
        let (mid, mstop) = (id.clone(), stop.clone());
        let inbound = merged(inbound, message.closed());
        self.spawn(async move { me.per_app_inbound(mid, inbound, mstop).await });

        // Input: once the phone's handshake arrived.
        let istop = stop.clone();
        let iid = id.clone();
        let ipipe = input_pipe.clone();
        let me = self.clone();
        self.spawn(async move {
            let ready = tokio::select! {
                r = i.ready() => r,
                _ = istop.cancelled() => return,
            };
            if let Err(e) = ready {
                return warn!(error = %e, session = %iid, "window input channel did not start; no input reaches the phone");
            }

            let i = Arc::new(i);
            {
                let initial = me.state.lock_or_panic().sessions.get(&iid).and_then(|s| s.display.device_resolution());
                let mut p = ipipe.lock_or_panic();
                p.set_sink(Some(i.clone() as Arc<dyn InputSink>));
                if initial.is_some() {
                    p.set_device_resolution(initial);
                }
            }
            istop.cancelled().await;
            i.close();
        });

        // Video: started once the window can present (within the wait).
        let me = self.clone();
        self.spawn(async move { me.video(id, v, ready_notify, stop).await });
    }

    pub(super) async fn video(
        self: Arc<Self>,
        id: String,
        mut video: VideoTarget,
        ready: Arc<tokio::sync::Notify>,
        stop: CancellationToken,
    ) {
        if self.window_ready_in_time(&id, &ready).await {
            video.start();
        } else {
            warn!(session = %id, "window not ready in time; video not started");
        }

        let mut first = true;
        let mut frame_count: u64 = 0;
        let mut presenter_failed = false;

        loop {
            let event = tokio::select! {
                e = video.next_event() => e,
                _ = stop.cancelled() => break,
            };
            match event {
                Some(VideoEvent::Ready { format, .. }) | Some(VideoEvent::FormatChanged(format)) => {
                    self.with_session(&id, |s| s.display.video_size((format.width, format.height)));
                }
                Some(VideoEvent::Frame(frame)) => {
                    let shown = self.present_frame(&id, frame.data, frame.keyframe, &mut presenter_failed).await;
                    if shown {
                        frame_count += 1;
                        video.frame_presented(frame.frame_id, phonelink_nano::channel::video::presented_time_us());
                        if first {
                            first = false;
                            self.session_command(&id, SessionCommand::PeerConnectionVideoReady, None);
                        }
                    }
                }
                Some(VideoEvent::BitrateChanged(_)) => {}
                Some(VideoEvent::Closed) | None => break,
            }
        }

        // Phone Link on Windows closes the channel without a StopStream; so does this.
        video.close();
        self.presenter.reset_video(id.clone());
        info!(session = %id, frame_count, "window video ended");
    }

    /// Waits, within the configured time, for window `id` to be ready to present (`ready` notifies a
    /// change); `false` when it was not in time.
    async fn window_ready_in_time(&self, id: &str, ready: &tokio::sync::Notify) -> bool {
        let wait = async {
            loop {
                let notified = ready.notified();
                if self.state.lock_or_panic().sessions.get(id).is_some_and(|s| s.window_ready) {
                    return;
                }
                notified.await;
            }
        };

        tokio::time::timeout(self.config.window_wait, wait).await.is_ok()
    }

    /// Presents one frame of window `id`'s video: `true` when it was shown. A presenter that panicked
    /// is logged once per stream (`failed` remembers it), since every later frame fails alike.
    async fn present_frame(&self, id: &str, data: Vec<u8>, keyframe: bool, failed: &mut bool) -> bool {
        let presenter = self.presenter.clone();
        let sid = id.to_string();
        match tokio::task::spawn_blocking(move || presenter.video_frame(sid, data, keyframe)).await {
            Ok(shown) => shown,
            Err(e) => {
                if !*failed {
                    *failed = true;
                    warn!(session = %id, error = %e, "video frame not presented; later failures of this stream are not logged");
                }
                false
            }
        }
    }

    pub(super) fn with_session(&self, id: &str, f: impl FnOnce(&mut Session)) {
        let changed = {
            let mut st = self.state.lock_or_panic();
            match st.sessions.get_mut(id) {
                Some(s) => {
                    f(s);
                    let res = s.display.device_resolution();
                    s.input.lock_or_panic().set_device_resolution(res);
                    true
                }
                None => false,
            }
        };
        if changed {
            let _ = self.events.send(ScreenMirroringEvent::SessionChanged { session_id: id.to_string() });
        }
    }

    pub(super) async fn per_app_inbound(
        self: Arc<Self>,
        id: String,
        mut inbound: mpsc::UnboundedReceiver<Scoped>,
        stop: CancellationToken,
    ) {
        loop {
            let m = tokio::select! {
                m = inbound.recv() => m,
                _ = stop.cancelled() => return,
            };
            let Some(m) = m else { break };
            self.per_app_message(&id, &m);
        }

        debug!(session = %id, "window message channel closed");
        let still_bound = self.state.lock_or_panic().sessions.get(&id).is_some_and(|s| s.bound.is_some());
        if still_bound && !stop.is_cancelled() {
            self.with_session(&id, |s| s.unbind());
        }
    }

    /// One message on a window's channel: `requestMoveToVD` is taken at whatever target its scope
    /// receives, the rest only at their own target.
    pub(super) fn per_app_message(self: &Arc<Self>, id: &str, scoped: &Scoped) {
        let m = &scoped.m;
        let target =
            if scoped.scope == scope::REQUEST_MOVE_TO_VD { scope::REQUEST_MOVE_TO_VD } else { m.target.as_str() };

        if target == scope::BRING_TO_FOREGROUND {
            let _ = self.events.send(ScreenMirroringEvent::BringToFront { session_id: id.to_string() });
            return;
        }
        if target == scope::ACCESSIBILITY_EVENT {
            return trace!(session = id, len = m.body.len(), "accessibilityEvent not presented");
        }
        if target.starts_with("/drag/") {
            let drag = self.state.lock_or_panic().sessions.get(id).and_then(|s| s.drag.clone());
            if let Some(d) = drag {
                d.on_message(target, &m.body);
            }
            return;
        }

        let body = match Body::parse(&m.body) {
            Ok(b) => b,
            Err(e) => return warn!(error = %e, route = %scoped.scope, "malformed window message; dropped"),
        };
        self.window_body_message(id, target, &m.body, &body);
    }

    /// One message on a window's channel at `target` other than the drag's, with its raw `bytes`
    /// and their parsed `body`.
    fn window_body_message(self: &Arc<Self>, id: &str, target: &str, bytes: &[u8], body: &Body) {
        match target {
            scope::APP_READY => {
                let task = body.raw("recentTaskId").and_then(|v| v.as_i64()).unwrap_or(-1);
                self.with_session(id, |s| s.recent_task_id = task);
            }
            scope::DEVICE_RESOLUTION_CHANGED => self.on_device_resolution_changed(id, body),
            scope::ORIENTATION_CHANGED => match body.int("orientationInfo") {
                Ok(o) => self.with_session(id, |s| s.display.orientation = o),
                Err(e) => warn!(error = %e, "malformed orientationChanged; dropped"),
            },
            scope::POSTURE => self.on_posture(id, body),
            scope::NAVIGATION_MODE => self.on_navigation_mode(id, body),
            scope::SECURE_CONTENT_CHANGED => self.on_secure_content_changed(id, body),
            scope::SCREEN_LAYOUT_STATUS => self.on_screen_layout_status(id, body),
            scope::REQUEST_MOVE_TO_VD => self.on_request_move_to_vd(id, body),
            scope::DISCONNECTING => match wire::parse_disconnecting(bytes) {
                Ok((reason, tx)) => self.disconnecting(id, reason, tx),
                Err(e) => warn!(error = %e, "malformed disconnecting; dropped"),
            },
            other => warn!(raw_target = other, session = id, "window message without a handler; dropped"),
        }
    }

    /// `deviceResolutionChanged`: the phone's display size.
    fn on_device_resolution_changed(&self, id: &str, body: &Body) {
        match (body.int("width"), body.int("height")) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => {
                self.with_session(id, |s| s.display.device_resolution_changed((w as u32, h as u32)))
            }
            _ => warn!("deviceResolutionChanged without a size; dropped"),
        }
    }

    /// `posture`: the area of the display that shows the content.
    fn on_posture(&self, id: &str, body: &Body) {
        match (body.int("left"), body.int("top"), body.int("right"), body.int("bottom")) {
            (Ok(l), Ok(t), Ok(r), Ok(b)) => self.with_session(id, |s| s.display.content_area = Some((l, t, r, b))),
            _ => warn!("posture without its four values; dropped"),
        }
    }

    /// `navigationMode`: the phone's navigation mode and button order.
    fn on_navigation_mode(&self, id: &str, body: &Body) {
        match (body.int("NavigationMode"), body.str("ButtonOrder")) {
            (Ok(mode), Ok(order)) => self.with_session(id, |s| {
                s.display.navigation_mode = mode;
                s.display.button_order = Some(order);
            }),
            _ => warn!("navigationMode without its values; dropped"),
        }
    }

    /// `secureContentChanged`: the window shows secure content when the configuration enables it.
    fn on_secure_content_changed(&self, id: &str, body: &Body) {
        match body.bool("isSecureContent") {
            Ok(true) if self.config.secure_content => self.session_command(id, SessionCommand::SecureContent, None),
            Ok(secure) => debug!(is_secure = secure, "secure content changed"),
            Err(e) => warn!(error = %e, "malformed secureContentChanged; dropped"),
        }
    }

    /// `screenLayoutStatus`: the app's current layout, saved for an app other than the phone screen.
    fn on_screen_layout_status(&self, id: &str, body: &Body) {
        match body.int("currentScreenLayout") {
            Ok(layout) => {
                let package = self.state.lock_or_panic().sessions.get(id).map(|s| s.package.clone());
                if let Some(p) = package.filter(|p| p != PHONE_SCREEN_PACKAGE) {
                    if let Err(e) = self.save_package(&p, |d| d.screen_layout = layout) {
                        error!(error = %e, "app layout not saved; the app opens in its earlier layout");
                    }
                }
                self.with_session(id, |s| s.current_layout = Some(layout));
            }
            Err(e) => warn!(error = %e, "malformed screenLayoutStatus; dropped"),
        }
    }

    /// `requestMoveToVD`: asks the user when the settings say to, otherwise accepts at once.
    fn on_request_move_to_vd(&self, id: &str, body: &Body) {
        let tx = body.opt_str("transactionId").unwrap_or_default();
        let task_transfer = body.opt_bool("remoteTaskTransferSupported").unwrap_or(false);
        let s = self.settings();
        let ask = (!s.do_not_ask_restart && !task_transfer) || (!s.do_not_ask_task_transfer && task_transfer);

        if ask {
            self.with_session(id, |s| s.move_to_virtual_display = Some((tx, task_transfer)));
            self.session_command(id, SessionCommand::MoveToVirtualDisplay, Some(Details::Restart { task_transfer }));
        } else {
            self.with_session(id, |s| s.move_to_virtual_display = Some((tx, task_transfer)));
            let _ = self.answer_move_to_virtual_display(id, true, false);
        }
    }

    pub(super) fn disconnecting(self: &Arc<Self>, id: &str, reason: DisconnectingReason, tx: String) {
        use DisconnectingReason as R;
        info!(session = id, disconnecting_kind = ?reason, "disconnecting received");

        let (message, state, pending_move) = {
            let mut st = self.state.lock_or_panic();
            let Some(s) = st.sessions.get_mut(id) else { return };
            // The window no longer continues a recent task.
            s.transfer_task_id = None;
            (s.bound.as_ref().map(|b| b.message.clone()), s.machine.state, s.move_to_virtual_display.is_some())
        };

        let ack = |m: &Option<Arc<MessageChannel>>| {
            send_on(m, scope::DISCONNECTING_RESULT, &wire::disconnecting_result(&tx));
        };

        match reason {
            R::CloseAll | R::CloseWindow | R::AppIsMovedToAnotherDisplay => {
                ack(&message);
                if reason == R::CloseWindow && state == WindowState::Restart && pending_move {
                    return;
                }
                let _ = self.close_session(id);
            }
            R::Heat => {
                self.session_command(id, SessionCommand::DeviceOverheat, Some(Details::Closed { reason: None }));
                ack(&message);
            }
            R::MaxApps => {
                self.session_command(
                    id,
                    SessionCommand::PeerConnectionClosedSuspended,
                    Some(Details::Closed { reason: None }),
                );
                ack(&message);
            }
            R::PowerSavingMode => {
                self.session_command(id, SessionCommand::PowerSavingMode, Some(Details::Closed { reason: None }));
                ack(&message);
            }
            _ => {
                self.session_command(id, SessionCommand::PeerConnectionClosed, Some(Details::Closed { reason: None }));
                ack(&message);
            }
        }
    }
}
