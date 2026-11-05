//! The orchestrator workflow and the early launch.

use super::{Inner, Out, PendingPhoneScreen, PhoneArgs, Scoped, Transaction, merged};
use crate::errors::{code, launch_result_code, setup_code};
use crate::gates::AppsFeatureType;
use crate::machine::{Details, PhoneCommand, SessionCommand};
use crate::session::TrueTouchBypass;
use crate::wire::{Body, LockedState, PHONE_SCREEN_PACKAGE, scope, screen_layout, workflow};
use crate::{MirroringError, ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use phonelink_nano::channel::message::MessageChannel;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

impl Inner {
    pub(super) async fn orchestrator_inbound(
        self: Arc<Self>,
        channel: Arc<MessageChannel>,
        mut inbound: mpsc::UnboundedReceiver<Scoped>,
    ) {
        while let Some(m) = inbound.recv().await {
            self.orchestrator_message(&channel, &m);
        }

        debug!("orchestrator channel closed");
        let mut st = self.state.lock_or_panic();
        if st.orchestrator.as_ref().is_some_and(|c| Arc::ptr_eq(c, &channel)) {
            st.orchestrator = None;
            st.remote_started = false;
        }

        // No answer can come: a waiting resume ends as not resumable.
        st.transactions.retain(|_, (kind, _)| *kind != Transaction::AppResumable);
    }

    fn send_black_screen(&self, channel: &MessageChannel) {
        if let Err(e) = channel.send(scope::BLACK_SCREEN, &wire::black_screen(self.settings().black_screen)) {
            warn!(error = %e, route = scope::BLACK_SCREEN, "message not sent");
        }
    }

    /// One message on the orchestrator or the early-launch channel. The black-screen request and the
    /// resume answer are taken at whatever target their scope receives; the rest only at their own.
    pub(super) fn orchestrator_message(self: &Arc<Self>, channel: &Arc<MessageChannel>, scoped: &Scoped) {
        let (registered, m) = (scoped.scope.as_str(), &scoped.m);
        let target = m.target.as_str();
        if registered == scope::REQUEST_BLACK_SCREEN_STATE {
            return self.send_black_screen(channel);
        }

        let body = match Body::parse(&m.body) {
            Ok(b) => b,
            Err(e) => return warn!(error = %e, route = registered, "malformed orchestrator message; dropped"),
        };
        if registered == scope::APP_RESUMABLE_RESULT {
            return self.complete_transaction(body.opt_str("transactionId").as_deref(), &m.body);
        }

        match target {
            scope::APP_LAUNCH_RESULT => self.on_app_launch_result(&body),
            scope::REQUEST_SESSION | scope::REQUEST_PHONE_SCREEN_SESSION => self.request_session(channel, &body),
            scope::SET_LOCKED_STATE => self.on_set_locked_state(&m.body),
            scope::LAUNCH_NOTIFICATION_RESULT => self.notification_result(&m.body),
            scope::ACCESSIBILITY_SETTINGS => log_accessibility_settings(&body),
            other => warn!(raw_target = other, "orchestrator message without a handler; dropped"),
        }
    }

    /// `appLaunchResult`, with the window it is for and the outcome.
    fn on_app_launch_result(self: &Arc<Self>, body: &Body) {
        match (body.str("sessionId"), body.str("result")) {
            (Ok(session), Ok(result)) => self.app_launch_result(&session, &result),
            _ => warn!("appLaunchResult without sessionId or result; dropped"),
        }
    }

    /// `setLockedState`, from its raw `body`.
    fn on_set_locked_state(&self, body: &[u8]) {
        match wire::parse_locked_state(body) {
            Ok(l) => self.locked_state(l),
            Err(e) => warn!(error = %e, "malformed setLockedState; dropped"),
        }
    }

    pub(super) fn locked_state(&self, l: LockedState) {
        let command = if l.is_folded && !l.can_draw {
            PhoneCommand::OnFoldedCannotDraw
        } else if l.is_secure_locked {
            PhoneCommand::OnSecureLocked
        } else if l.is_locked {
            PhoneCommand::OnLocked
        } else if !l.is_screen_on {
            PhoneCommand::OnScreenOff
        } else {
            PhoneCommand::OnUnlocked
        };
        self.phone_command(command, PhoneArgs::lock(l));
    }

    pub(super) fn app_launch_result(self: &Arc<Self>, id: &str, result: &str) {
        let phone_screen = {
            let st = self.state.lock_or_panic();
            match st.sessions.get(id) {
                Some(s) => s.is_phone_screen(),
                None => return debug!(session = id, "appLaunchResult for no window; ignored"),
            }
        };

        info!(session = id, raw_result = result, "appLaunchResult received");
        if result.eq_ignore_ascii_case("success") {
            return self.session_command(id, SessionCommand::AppLaunchSuccess, None);
        }

        let true_touch = result == "trueTouchWarning";
        let Some(do_not_show) = self.retire_failed_launch(id, true_touch) else { return };
        if true_touch {
            self.session_command(id, SessionCommand::TrueTouchWarning, None);
            if do_not_show {
                let _ = self.relaunch(id, true);
            }
            return;
        }

        if result == "duplicateLaunch" {
            let _ = self.close_session(id);
            return;
        }

        let code = launch_result_code(result);
        let dismissible = phone_screen && (code == code::APP_DOES_NOT_EXIST || code == code::APP_UNAVAILABLE);
        let command =
            if dismissible { SessionCommand::PhoneScreenAppLaunchError } else { SessionCommand::AppLaunchError };
        self.session_command(id, command, Some(Details::Error { code }));
    }

    /// Retires the channel set of the window `id` whose launch did not succeed, marking a touch
    /// warning's bypass available: the window's "do not show the touch confirmation", or `None` when
    /// there is no such window or a touch warning is already pending.
    fn retire_failed_launch(&self, id: &str, true_touch: bool) -> Option<bool> {
        let mut st = self.state.lock_or_panic();
        let s = st.sessions.get_mut(id)?;

        if true_touch {
            // A second warning while the first one's launch is pending changes nothing.
            if s.true_touch != TrueTouchBypass::Unavailable {
                debug!(session = id, "touch warning while one is pending; ignored");
                return None;
            }
            s.true_touch = TrueTouchBypass::Available;
        }

        // A launch that did not succeed retires the window's channel set.
        s.unbind();
        Some(s.machine.do_not_show_true_touch)
    }

    /// `requestSession` / `requestPhoneScreenSession` on `channel` (the orchestrator or early launch).
    /// On the orchestrator the reply's layout follows the settings (*`layout_override`*); on the
    /// early-launch channel it is the saved one.
    pub(super) fn request_session(self: &Arc<Self>, channel: &Arc<MessageChannel>, body: &Body) {
        let (Ok(package), Ok(transaction)) = (body.str("packageName"), body.str("transactionId")) else {
            return warn!("requestSession without packageName or transactionId; dropped");
        };

        let on_orchestrator = self.state.lock_or_panic().orchestrator.as_ref().is_some_and(|o| Arc::ptr_eq(o, channel));
        let existing = (package == PHONE_SCREEN_PACKAGE)
            .then(|| Self::find_package(&self.state.lock_or_panic(), PHONE_SCREEN_PACKAGE))
            .flatten();
        if let Some(id) = existing {
            return self.raise_phone_screen(channel, &id, &transaction, on_orchestrator);
        }

        let mut out = Out::default();
        let id = {
            let mut st = self.state.lock_or_panic();
            if st.served.get(&transaction).is_some_and(|(p, _)| *p == package) {
                return debug!(package = %package, "requestSession repeated; ignored");
            }
            let id = new_guid_lower();
            st.served.insert(transaction.clone(), (package.clone(), id.clone()));
            self.create_session(&mut st, id.clone(), &package, false, &mut out);
            id
        };
        self.flush(out);

        let saved = if package == PHONE_SCREEN_PACKAGE { 0 } else { self.package(&package).screen_layout };
        let body = wire::request_session_result(&id, &transaction, false, self.reply_layout(on_orchestrator, saved));
        send_request_session_result(channel, &body);
        self.spawn_launch(id, false);
    }

    /// The phone shows the phone screen: the open window `id` is raised as a launch of it would, and
    /// the request answered.
    fn raise_phone_screen(&self, channel: &MessageChannel, id: &str, transaction: &str, on_orchestrator: bool) {
        self.ensure_ready_for_launch();
        let _ = self.events.send(ScreenMirroringEvent::BringToFront { session_id: id.to_string() });
        self.wake_with_permission();
        let layout = self.reply_layout(on_orchestrator, screen_layout::DEFAULT);
        let body = wire::request_session_result(id, transaction, true, layout);
        send_request_session_result(channel, &body);
    }

    /// The layout a `requestSessionResult` gives: on the orchestrator the settings' override of
    /// `saved`, on the early-launch channel `saved` itself.
    fn reply_layout(&self, on_orchestrator: bool, saved: i32) -> i32 {
        if on_orchestrator { self.layout_override(saved) } else { saved }
    }

    pub(super) fn notification_result(&self, body: &[u8]) {
        let r = match wire::parse_notification_result(body) {
            Ok(r) => r,
            Err(e) => return warn!(error = %e, "malformed launchNotificationResult; dropped"),
        };

        let tx = r
            .transaction_id
            .clone()
            .or_else(|| r.key.as_ref().and_then(|k| self.state.lock_or_panic().notification_keys.get(k).cloned()));

        // The phone answers a V1 launch too, with only `launchResult`; nothing waits for it.
        let Some(tx) = tx else {
            return debug!(launch_result = r.launch_result, "launchNotificationResult for a V1 launch");
        };
        self.complete_transaction(Some(&tx), body);
    }

    /// The channel a notification launch or the keyboard-settings request goes on: the
    /// orchestrator once the `remote` workflow's start has begun (after it completes), else the
    /// `earlyLaunch` workflow's, started once on a new connection.
    pub(super) async fn permission_independent_channel(
        self: &Arc<Self>,
    ) -> Result<Arc<MessageChannel>, MirroringError> {
        let (remote_started, running) = {
            let st = self.state.lock_or_panic();
            (st.remote_started, st.setup.clone())
        };
        if remote_started {
            if let Some(setup) = running {
                setup.await.map_err(|f| MirroringError::Status(f.code()))?;
            }
            return self
                .state
                .lock_or_panic()
                .orchestrator
                .clone()
                .ok_or(MirroringError::NotNow("the set-up did not open"));
        }

        let _one = self.early_lock.lock().await;
        if let Some(c) = self.state.lock_or_panic().early.clone() {
            return Ok(c);
        }

        let audio = self.values().is_ok_and(|v| v.audio());
        let started = match self.links.start(workflow::EARLY_LAUNCH, wire::EARLY_LAUNCH_INBOUND, true, audio).await {
            Ok(s) => s,
            Err(e) => {
                let ice = self.values().is_ok_and(|v| v.ice_support());
                let code = setup_code(e.code(), ice);
                warn!(code, error = %e, "earlyLaunch workflow did not open; the windows show the error");
                self.phone_command(PhoneCommand::OnConnectionError, PhoneArgs::error(code));
                // The windows waiting for the set-up send no launch, and the audio-permission flows
                // stop; the set-up itself goes on.
                {
                    let mut st = self.state.lock_or_panic();
                    st.setup_epoch += 1;
                    st.audio_cancel.cancel();
                    st.audio_cancel = CancellationToken::new();
                }
                return Err(MirroringError::Status(code));
            }
        };

        let channel = started.channel.clone();
        self.state.lock_or_panic().early = Some(channel.clone());

        let inbound = merged(started.inbound, channel.closed());
        let me = self.clone();
        let c = channel.clone();
        self.spawn(async move {
            let mut inbound = inbound;
            while let Some(m) = inbound.recv().await {
                me.orchestrator_message(&c, &m);
            }
            let mut st = me.state.lock_or_panic();
            if st.early.as_ref().is_some_and(|e| Arc::ptr_eq(e, &c)) {
                st.early = None;
            }
        });
        Ok(channel)
    }

    pub(in super::super) async fn launch_notification(
        self: Arc<Self>,
        key: &str,
        action_index: i32,
    ) -> Result<(), MirroringError> {
        let v = self.check_launchable()?;
        if v.apps_feature_type() != AppsFeatureType::MultipleApps {
            // Without app windows the phone-screen window opens it, the next time that window becomes
            // active (connected, or focused again while connected) — unless another launch of the
            // phone screen comes first. A window already active and focused waits for its next
            // activation.
            self.open_phone_screen(Some(PendingPhoneScreen::Notification(key.to_string(), action_index)))?;
            return Ok(());
        }

        self.ensure_ready_for_launch();
        let channel = self.permission_independent_channel().await?;
        if !v.notification_fallback() {
            return self.launch_notification_v1(&channel, key, action_index);
        }

        let (tx, rx) = self.begin_transaction(Transaction::Other);
        self.state.lock_or_panic().notification_keys.insert(key.to_string(), tx.clone());
        if let Err(e) = channel.send(scope::LAUNCH_NOTIFICATION, &wire::launch_notification_v2(&tx, key, action_index))
        {
            self.state.lock_or_panic().transactions.remove(&tx);
            return Err(e.into());
        }
        debug!("launchNotification sent");

        let result = tokio::time::timeout(self.config.notification_result, rx).await;
        {
            let mut st = self.state.lock_or_panic();
            st.transactions.remove(&tx);
            st.notification_keys.remove(key);
        }

        let is_timed_out = result.is_err();
        let launch = match result {
            Ok(Ok(body)) => wire::parse_notification_result(&body).map(|r| r.launch_result).ok(),
            _ => None,
        };

        // Opened on a virtual display (0) or on the phone's own screen (1); "already opened" (4) is
        // not a success. On the phone's screen the phone-screen window shows it.
        let opened = matches!(launch, Some(0 | 1));
        info!(launch_result = launch, is_timed_out, opened, "notification launch finished");
        let _ = self.events.send(ScreenMirroringEvent::NotificationLaunched { key: key.to_string(), opened });
        if launch == Some(1) {
            self.open_phone_screen_as(None, Some(tx.clone()))?;
        }
        Ok(())
    }

    /// A V1 `launchNotification`, which the phone answers with nothing to wait for.
    fn launch_notification_v1(
        &self,
        channel: &MessageChannel,
        key: &str,
        action_index: i32,
    ) -> Result<(), MirroringError> {
        channel.send(scope::LAUNCH_NOTIFICATION, &wire::launch_notification_v1(key, action_index))?;
        info!("launchNotification sent; the phone sends no result");
        let _ = self.events.send(ScreenMirroringEvent::NotificationLaunched { key: key.to_string(), opened: true });
        Ok(())
    }

    /// `launchCustomIntent` on the permission-independent route; without `MultipleApps` the
    /// `launchPhoneScreenApp` form, which only the orchestrator handles (the phone's `earlyLaunch`
    /// channel drops it).
    pub(in super::super) async fn request_keyboard_settings(self: Arc<Self>) -> Result<(), MirroringError> {
        let v = self.check_launchable()?;
        if v.apps_feature_type() == AppsFeatureType::MultipleApps {
            let channel = self.permission_independent_channel().await?;
            let body = wire::launch_custom_intent(wire::KEYBOARD_SETTINGS_INTENT);
            return Ok(channel.send(scope::LAUNCH_CUSTOM_INTENT, &body)?);
        }

        let running = self.state.lock_or_panic().setup.clone();
        if let Some(setup) = running {
            setup.await.map_err(|f| MirroringError::Status(f.code()))?;
        }

        let channel = self
            .state
            .lock_or_panic()
            .orchestrator
            .clone()
            .ok_or(MirroringError::NotNow("the phone screen is not connected"))?;

        let launch_type = if self.presenter.input_source_is_input_method() { 3 } else { 2 };
        let body = wire::launch_phone_screen_app(PHONE_SCREEN_PACKAGE, &new_guid_lower(), launch_type);
        Ok(channel.send(scope::LAUNCH_PHONE_SCREEN_APP, &body)?)
    }
}

/// Sends a `requestSessionResult` on `channel`.
fn send_request_session_result(channel: &MessageChannel, body: &[u8]) {
    if let Err(e) = channel.send(scope::REQUEST_SESSION_RESULT, body) {
        warn!(error = %e, "requestSessionResult not sent");
    }
}

/// Logs the phone's `accessibilitySettings`.
fn log_accessibility_settings(body: &Body) {
    info!(
        is_service_enabled = ?body.opt_bool("accessibilityServiceEnabled"),
        talkback = ?body.opt_bool("accessibilityTalkbackEnabled"),
        negative = ?body.opt_bool("negativeFilterEnabled"),
        "accessibilitySettings received"
    );
}
