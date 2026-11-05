//! The windows after launch: closing and releasing them, and the UI's actions on one.

use super::{ASSET_PHONE, ASSET_TABLET, Inner, NearbySignal, Out, PhoneArgs, SetupStage, State, Transaction, send_on};
use crate::drag::WindowDrag;
use crate::errors::code;
use crate::input::{InputPipe, NavButton};
use crate::machine::{Details, PhoneCommand, PhoneState, SessionCommand, WindowState, phone_next};
use crate::session::TrueTouchBypass;
use crate::store::Settings;
use crate::wire::{Body, FocusableState, scope, screen_layout, workflow};
use crate::{MirroringError, ScreenMirroringEvent, store, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

impl Inner {
    pub(in super::super) fn close_session(&self, id: &str) -> Result<(), MirroringError> {
        let last = {
            let mut st = self.state.lock_or_panic();
            let mut s = st.sessions.remove(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            s.stop.cancel();
            s.unbind();
            if s.is_phone_screen() {
                st.phone_screen_active = false;
            }
            st.served.retain(|_, (_, sid)| sid != id);
            st.sessions.is_empty()
        };

        info!(session = id, "window closed");
        self.presenter.reset_video(id.to_string());
        let _ = self.events.send(ScreenMirroringEvent::SessionClosed { session_id: id.to_string() });
        if last {
            self.release_all(code::WINDOWS_CLOSED);
        }
        Ok(())
    }

    /// The last window closed: every workflow stopped; the connection closes when idle.
    pub(super) fn release_all(&self, code: i32) {
        let mut out = Out::default();
        let cancel_code = {
            let mut st = self.state.lock_or_panic();
            Self::cancel_setup(&mut st, code);
            self.drop_channels(&mut st);
            self.apply_phone_quiet(&mut st, PhoneCommand::OnConnectionClosed, &mut out);
            st.setup_cancel_code
        };

        self.reset_signals(cancel_code);
        for w in [workflow::REMOTE, workflow::AUDIO, workflow::PERMISSION, workflow::NEARBY, workflow::EARLY_LAUNCH] {
            self.links.stop(w);
        }
        info!(code, "mirroring released");
        self.flush(out);
    }

    /// A release cancels the running set-up, which fails and stops with:
    /// - the release's code in the nearby check once its channel is open, and while waiting for the
    ///   consent or the audio permissions;
    /// - 80 in the nearby check while its workflow is still starting, when no reason is known yet;
    /// - 268 while connecting or opening `remote`.
    ///
    /// Phone Link on Windows releases only the workflows whose channel is open, so a `remote` start in
    /// progress goes on there.
    pub(super) fn cancel_setup(st: &mut State, code: i32) {
        st.setup_cancel_code = match st.setup_stage {
            SetupStage::Nearby if st.nearby_channel.is_some() => code,
            SetupStage::Nearby => code::DISCONNECTED_UNKNOWN,
            SetupStage::Permission | SetupStage::Audio => code,
            SetupStage::Start | SetupStage::Remote => code::CONNECTION_CANCELED,
        };
        st.setup_cancel.cancel();
        st.setup_epoch += 1;
        st.setup = None;
        st.setup_requester = None;
        st.remote_started = false;
    }

    /// Ends a waiting nearby check with `code` (the set-up's cancel code, so the check and the
    /// set-up's cancellation report the same code) and drops the pending audio answers.
    pub(super) fn reset_signals(&self, code: i32) {
        // `send_replace`: `send` keeps the old value while no check is waiting on the signal.
        self.nearby_signal.send_replace(NearbySignal { failed: Some(code), ..Default::default() });
        self.audio_signal.send_replace((None, None));
    }

    /// A phone command without fan-out (a release reports its own error to the windows).
    pub(super) fn apply_phone_quiet(&self, st: &mut State, command: PhoneCommand, out: &mut Out) {
        if let Some(next) = phone_next(st.phone, command) {
            st.phone = next;
            st.phone_epoch += 1;
            st.nearby = None;
            out.events.push(ScreenMirroringEvent::Changed);
        }
    }

    pub(super) fn drop_channels(&self, st: &mut State) {
        st.orchestrator = None;
        st.early = None;
        st.permission = None;
        st.nearby_channel = None;
        st.audio = None;
        st.audio_output = None;
        st.audio_stop.cancel();
        st.permission_status = None;
        st.frame_length = None;
        st.remote_started = false;
        st.audio_answers.clear();
        st.audio_cancel.cancel();
        st.audio_cancel = CancellationToken::new();
        for c in st.held_audio.drain(..) {
            c.close();
        }
        st.grouping.clear();
        st.transactions.clear();
        st.notification_keys.clear();
    }

    /// The connection ended or a workflow the set-up opened ended: every window shows `code`.
    pub(super) fn released(&self, code: i32) {
        let mut out = Out::default();
        let cancel_code = {
            let mut st = self.state.lock_or_panic();
            Self::cancel_setup(&mut st, code);
            self.drop_channels(&mut st);
            for s in st.sessions.values_mut() {
                s.unbind();
            }
            self.apply_phone_quiet(&mut st, PhoneCommand::OnConnectionClosed, &mut out);
            self.fanout(&mut st, PhoneState::Error, PhoneArgs::error(code), &mut out);
            st.setup_cancel_code
        };

        self.reset_signals(cancel_code);
        for w in [workflow::REMOTE, workflow::AUDIO, workflow::PERMISSION, workflow::NEARBY, workflow::EARLY_LAUNCH] {
            self.links.stop(w);
        }
        self.flush(out);
    }

    pub(in super::super) fn window_ready(&self, id: &str) -> Result<(), MirroringError> {
        let notify = {
            let mut st = self.state.lock_or_panic();
            let s = st.sessions.get_mut(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            s.window_ready = true;
            s.window_ready_notify.clone()
        };
        notify.notify_waiters();
        Ok(())
    }

    pub(in super::super) fn focus_changed(
        &self,
        id: &str,
        state: FocusableState,
        caps_lock: bool,
    ) -> Result<(), MirroringError> {
        let mut out = Out::default();
        let (input, message) = {
            let mut st = self.state.lock_or_panic();
            let s = st.sessions.get_mut(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            s.focus = state;
            let active = (s.is_phone_screen())
                .then(|| state == FocusableState::InFocus && s.machine.state == WindowState::Connected);
            let r = (s.input.clone(), s.bound.as_ref().map(|b| b.message.clone()));
            if let Some(active) = active {
                Self::phone_screen_active(&mut st, active, &mut out);
            }
            r
        };

        let left_released = {
            let mut p = input.lock_or_panic();
            let focused = state == FocusableState::InFocus;
            let left = !focused && p.focused() && p.left_pressed();
            p.focus(focused, caps_lock);
            left
        };

        send_on(&message, scope::APP_FOCUSABLE_STATE, &wire::app_focusable_state(state, &new_guid_lower()));
        self.flush(out);
        if left_released {
            self.drag(id)?.focus_lost(true);
        }
        Ok(())
    }

    pub(in super::super) fn drag(&self, id: &str) -> Result<Arc<WindowDrag>, MirroringError> {
        let st = self.state.lock_or_panic();
        let s = st.sessions.get(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
        Ok(s.drag.clone().expect("the runtime gives every window its drag"))
    }

    pub(in super::super) fn with_input(&self, id: &str, f: impl FnOnce(&mut InputPipe)) -> Result<(), MirroringError> {
        let input = {
            let st = self.state.lock_or_panic();
            st.sessions.get(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?.input.clone()
        };
        f(&mut input.lock_or_panic());
        Ok(())
    }

    pub(in super::super) fn nav_button(&self, id: &str, button: NavButton) -> Result<(), MirroringError> {
        let (input, connected) = {
            let st = self.state.lock_or_panic();
            let s = st.sessions.get(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            (s.input.clone(), s.machine.state == WindowState::Connected)
        };
        if !connected {
            return Err(MirroringError::NotNow("the window is not connected"));
        }
        input.lock_or_panic().nav_button(button);
        Ok(())
    }

    /// A suspended window asks whether its app can resume; when it can, the app launches again. The
    /// answer has no time limit; the orchestrator closing ends the wait as not resumable (Phone Link
    /// on Windows keeps waiting).
    pub(in super::super) async fn resume(self: Arc<Self>, id: &str) -> Result<bool, MirroringError> {
        if !self.state.lock_or_panic().sessions.contains_key(id) {
            return Err(MirroringError::UnknownSession(id.to_string()));
        }

        let (tx, rx) = self.begin_transaction(Transaction::AppResumable);
        if !self.send_orchestrator(scope::APP_RESUMABLE, &wire::app_resumable(&tx)) {
            self.state.lock_or_panic().transactions.remove(&tx);
            return Ok(false);
        }

        let resumable = match rx.await {
            Ok(body) => Body::parse(&body).ok().and_then(|b| b.opt_bool("isAppResumable")).unwrap_or(false),
            Err(_) => false,
        };

        info!(session = id, resumable, "appResumableResult received");
        if resumable {
            self.session_command(id, SessionCommand::RelaunchApp, None);
            self.relaunch(id, false)?;
        }
        Ok(resumable)
    }

    /// The window asks for its app again (Reopen, Launch again, or past the touch warning with
    /// `bypass`): a bypass is taken once per warning, or by a suspended window.
    pub(super) fn relaunch(&self, id: &str, bypass: bool) -> Result<(), MirroringError> {
        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            let s = st.sessions.get_mut(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            if bypass {
                if s.machine.state == WindowState::Suspended {
                    s.true_touch = TrueTouchBypass::Unavailable;
                } else if s.true_touch == TrueTouchBypass::Available {
                    s.true_touch = TrueTouchBypass::Claimed;
                } else {
                    return Err(MirroringError::NotNow("no touch confirmation is waiting"));
                }
            } else {
                s.true_touch = TrueTouchBypass::Unavailable;
            }

            s.pc_initiated = true;
            Self::apply_session_preserving(&mut st, id, SessionCommand::RelaunchApp, &mut out);
        }

        self.flush(out);
        self.ensure_ready_for_launch();
        self.spawn_launch(id.to_string(), bypass);
        Ok(())
    }

    pub(in super::super) fn reopen(&self, id: &str) -> Result<(), MirroringError> {
        self.relaunch(id, false)
    }

    /// Retry / Connect: every window whose state the relaunch moves (and one showing the phone's
    /// lock) runs the set-up and its launch again.
    pub(in super::super) fn retry(&self) -> Result<(), MirroringError> {
        self.relaunch_all_if_needed(true);
        Ok(())
    }

    pub(in super::super) fn continue_true_touch(&self, id: &str) -> Result<(), MirroringError> {
        self.relaunch(id, true)
    }

    pub(in super::super) fn dismiss_error(&self, id: &str) -> Result<(), MirroringError> {
        if !self.state.lock_or_panic().sessions.contains_key(id) {
            return Err(MirroringError::UnknownSession(id.to_string()));
        }
        self.session_command(id, SessionCommand::DismissError, None);
        Ok(())
    }

    /// Wakes the phone's screen. Before the `remote` workflow opens (the nearby check, for one) there
    /// is no orchestrator to send it on, and nothing is sent.
    pub(in super::super) fn wake(&self) -> Result<(), MirroringError> {
        self.send_orchestrator(scope::WAKE_SCREEN, &wire::empty_orchestrator());
        Ok(())
    }

    pub(in super::super) fn resend_permission(&self) -> Result<(), MirroringError> {
        let (c, accepted) = {
            let st = self.state.lock_or_panic();
            (st.permission.clone(), st.permission_status.as_deref() == Some("accept"))
        };
        if accepted {
            return Err(MirroringError::NotNow("the phone already accepted"));
        }

        let c = c.ok_or(MirroringError::NotNow("the permission workflow is not open"))?;
        Ok(c.send(scope::REQUEST_PERMISSION, &wire::request_permission())?)
    }

    pub(in super::super) fn answer_move_to_virtual_display(
        &self,
        id: &str,
        accept: bool,
        do_not_ask: bool,
    ) -> Result<(), MirroringError> {
        let (pending, message) = {
            let mut st = self.state.lock_or_panic();
            let s = st.sessions.get_mut(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            (s.move_to_virtual_display.take(), s.bound.as_ref().map(|b| b.message.clone()))
        };

        let Some((tx, task_transfer)) = pending else {
            return Err(MirroringError::NotNow("the phone asked nothing"));
        };

        if do_not_ask {
            let mut s = self.settings();
            if task_transfer {
                s.do_not_ask_task_transfer = true;
            } else {
                s.do_not_ask_restart = true;
            }
            self.set_settings(s)?;
        }

        send_on(&message, scope::REQUEST_MOVE_TO_VD_RESULT, &wire::request_move_to_vd_result(accept, &tx));
        if accept {
            self.session_command(id, SessionCommand::AppLaunchSuccess, None);
        }
        let _ = self.events.send(ScreenMirroringEvent::SessionChanged { session_id: id.to_string() });
        Ok(())
    }

    pub(in super::super) fn set_settings(&self, new: Settings) -> Result<(), MirroringError> {
        let old = self.settings();
        let db = self.store.get(&self.phone()?)?;
        store::save_settings(&db, &new)?;

        if old.black_screen != new.black_screen {
            self.send_orchestrator(scope::BLACK_SCREEN, &wire::black_screen(new.black_screen));
        }
        if old.always_expanded != new.always_expanded {
            self.send_orchestrator(scope::ALWAYS_EXPANDED, &wire::always_expanded(new.always_expanded));
        }
        if old.audio_streaming != new.audio_streaming {
            self.send_streaming_allowed(self.presenter.has_audio_output());
        }
        if old.allow_apps && !new.allow_apps {
            self.close_everything(code::DEVICE_SWITCHED);
        }

        let _ = self.events.send(ScreenMirroringEvent::Changed);
        Ok(())
    }

    pub(in super::super) fn selected_phone_changed(&self) {
        info!("selected phone changed; mirroring closes");
        self.close_everything(code::DEVICE_SWITCHED);
    }

    /// The phone's device connection became disabled or unavailable: the connection closes and the
    /// windows stay open, showing 81. (A plain disconnect of the device connection changes nothing.)
    pub(in super::super) fn device_connection_lost(&self) {
        info!("device connection lost; mirroring is released");
        let any = !self.state.lock_or_panic().sessions.is_empty();
        if any {
            self.released(code::LOST_CONNECTION);
        }
        self.links.close(code::LOST_CONNECTION);
    }

    /// Closes the connection and every window.
    pub(super) fn close_everything(&self, code: i32) {
        let ids: Vec<String> = self.state.lock_or_panic().sessions.keys().cloned().collect();
        for id in &ids {
            let _ = self.close_session(id);
        }
        if ids.is_empty() {
            self.release_all(code);
        }
        self.links.close(code);
    }

    /// The window's volume change: stored (the level only when not muted, or when it is 0) and sent
    /// to the phone. Without an open `audio` workflow nothing is stored or sent, as on Windows.
    pub(in super::super) fn set_volume(&self, package: &str, volume: i32, muted: bool) -> Result<(), MirroringError> {
        let volume = volume.clamp(0, 100);
        let Some(c) = self.state.lock_or_panic().audio.clone() else {
            debug!("volume change dropped; the audio workflow is not open");
            return Ok(());
        };

        self.save_package(package, |d| {
            if !muted || volume == 0 {
                d.volume = volume;
            }
            d.muted = muted;
        })?;

        send_on(&Some(c), scope::AUDIO_SET_VOLUME, &wire::audio_set_volume(package, if muted { 0 } else { volume }));
        self.audio_changed(package);
        Ok(())
    }

    /// The user asks for screen layout `layout` in a connected app window: Default becomes Expanded
    /// with "always expanded", else the phone's kind (a tablet Expanded, a phone Compact). It is
    /// confirmed first unless "do not ask" is on.
    pub(in super::super) fn switch_layout(&self, id: &str, layout: i32) -> Result<(), MirroringError> {
        if !self.values().is_ok_and(|v| v.adaptive_layout()) {
            return Err(MirroringError::NotSupported("the phone has no adaptive layout"));
        }

        let mut layout = layout;
        if layout == screen_layout::DEFAULT {
            let category = self.env.app.phone_record().map_or(0, |r| r.asset_category);
            if self.settings().always_expanded || category == ASSET_TABLET {
                layout = screen_layout::EXPANDED;
            } else if category == ASSET_PHONE {
                layout = screen_layout::COMPACT;
            }
        }

        {
            let mut st = self.state.lock_or_panic();
            let s = st.sessions.get_mut(id).ok_or_else(|| MirroringError::UnknownSession(id.to_string()))?;
            if s.is_phone_screen() {
                return Err(MirroringError::NotSupported("the phone screen has no layouts"));
            }
            if s.machine.state != WindowState::Connected {
                return Err(MirroringError::NotNow("the window is not connected"));
            }
            s.target_layout = Some(layout);
        }

        if self.settings().do_not_ask_switch_layout {
            self.process_layout_switch(id);
        } else {
            self.session_command(id, SessionCommand::SwitchScreenLayout, Some(Details::LayoutSwitch));
        }
        Ok(())
    }

    /// The user confirmed the layout switch (and "do not ask again" when `do_not_ask`).
    pub(in super::super) fn confirm_layout_switch(&self, id: &str, do_not_ask: bool) -> Result<(), MirroringError> {
        if self.state.lock_or_panic().sessions.get(id).is_none_or(|s| s.target_layout.is_none()) {
            return Err(MirroringError::NotNow("no layout switch is waiting"));
        }
        if do_not_ask {
            let mut s = self.settings();
            s.do_not_ask_switch_layout = true;
            self.set_settings(s)?;
        }

        self.process_layout_switch(id);
        Ok(())
    }

    pub(super) fn process_layout_switch(&self, id: &str) {
        let (layout, message) = {
            let mut st = self.state.lock_or_panic();
            let Some(s) = st.sessions.get_mut(id) else { return };
            (s.target_layout.take(), s.bound.as_ref().map(|b| b.message.clone()))
        };

        let Some(layout) = layout else { return };
        let body = wire::request_screen_layout(layout, &new_guid_lower(), None);
        if send_on(&message, scope::REQUEST_SCREEN_LAYOUT, &body) {
            info!(session = id, layout, "requestScreenLayout sent");
        }
        self.session_command(id, SessionCommand::AppLaunchSuccess, None);
    }

    /// The Mac's output devices changed: the phone is told again only when an output appeared where
    /// there was none, or the last one went (and only while the `audio` workflow is open).
    pub(in super::super) fn audio_output_changed(&self) {
        let now = self.presenter.has_audio_output();
        let flipped = {
            let mut st = self.state.lock_or_panic();
            match st.audio_output {
                Some(before) if before != now => {
                    st.audio_output = Some(now);
                    true
                }
                _ => false,
            }
        };
        if flipped {
            self.send_streaming_allowed(now);
        }
    }

    /// `pcStreamingAllowed`: an output device exists (`output`) and the audio-streaming setting is on.
    pub(super) fn send_streaming_allowed(&self, output: bool) {
        let allowed = output && self.settings().audio_streaming;
        let c = self.state.lock_or_panic().audio.clone();
        send_on(&c, scope::AUDIO_PC_STREAMING_ALLOWED, &wire::audio_pc_streaming_allowed(allowed));
    }

    pub(in super::super) fn screen_lock_changed(&self, locked: bool) {
        // The close's line carries the code (`DEVICE_LOCKED`) that tells the lock.
        if locked && self.links.is_connected() {
            self.links.close(code::DEVICE_LOCKED);
        }
    }
}
