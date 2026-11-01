//! How the phone state and the windows change, and what each change sends.

use super::{Inner, Out, PhoneArgs, State};
use crate::ScreenMirroringEvent;
use crate::errors::code;
use crate::machine::{Details, PhoneCommand, PhoneState, SessionCommand, WindowState, phone_next};
use crate::session::Session;
use crate::wire::LockedState;
use phonelink_core::sync::MutexExt;
use tracing::info;

impl Inner {
    pub(super) fn flush(&self, out: Out) {
        for e in out.events {
            let _ = self.events.send(e);
        }
        for (id, epoch) in out.session_watch {
            self.watch_session(id, epoch);
        }
        if let Some(epoch) = out.phone_watch {
            self.watch_phone(epoch);
        }
        for id in out.changed {
            self.apply_initial_volume(&id);
        }
        if let Some((key, index)) = out.notification {
            self.open_pending_notification(key, index);
        }
    }

    pub(super) fn apply_session(
        st: &mut State,
        id: &str,
        command: SessionCommand,
        details: Option<Details>,
        out: &mut Out,
    ) {
        Self::apply_session_with(st, id, command, out, |m| m.apply(command, details));
    }

    /// A relaunch command that keeps the window's details (the launch paths' relaunch).
    pub(super) fn apply_session_preserving(st: &mut State, id: &str, command: SessionCommand, out: &mut Out) -> bool {
        Self::apply_session_with(st, id, command, out, |m| m.apply_preserving(command))
    }

    pub(super) fn apply_session_with(
        st: &mut State,
        id: &str,
        command: SessionCommand,
        out: &mut Out,
        f: impl FnOnce(&mut super::super::machine::SessionMachine) -> bool,
    ) -> bool {
        let Some(s) = st.sessions.get_mut(id) else { return false };
        let from = s.machine.state;
        if !f(&mut s.machine) {
            return false;
        }

        s.epoch += 1;
        let state = s.machine.state;
        let phone_screen = s.is_phone_screen();
        if let Some(d) = &s.drag {
            d.window_state(state);
        }

        info!(session = id, ?command, from_state = ?from, ?state, "window state changed");
        out.events.push(ScreenMirroringEvent::SessionChanged { session_id: id.to_string() });
        out.changed.push(id.to_string());
        if matches!(state, WindowState::Loading | WindowState::ConnectionUnstable) {
            out.session_watch.push((id.to_string(), s.epoch));
        }
        if phone_screen {
            Self::phone_screen_active(st, state == WindowState::Connected, out);
        }
        true
    }

    /// The phone-screen window's activity: it is active when it becomes connected, inactive in any
    /// other state, and on a focus change active when focused and connected. Becoming active opens
    /// the notification waiting for it.
    pub(super) fn phone_screen_active(st: &mut State, active: bool, out: &mut Out) {
        let became = active && !st.phone_screen_active;
        st.phone_screen_active = active;
        if became && let Some(n) = st.pending_notification.take() {
            out.notification = Some(n);
        }
    }

    /// The window command a phone state gives session `s`. The lock states reach a window only with
    /// the lock message's values, and an app window not while the phone keeps its display unlocked.
    pub(super) fn fanout_command(
        &self,
        s: &Session,
        phone: PhoneState,
        args: &PhoneArgs,
        allow_apps: bool,
    ) -> Option<(SessionCommand, Option<Details>)> {
        use SessionCommand as C;
        let applies = |l: &Option<LockedState>| l.is_some_and(|l| s.is_phone_screen() || !l.is_always_unlocked_display);
        let vd = args.lock.is_some_and(|l| l.is_virtual_device_streaming_supported);

        Some(match phone {
            PhoneState::Disconnected => {
                if allow_apps {
                    (C::RelaunchApp, None)
                } else {
                    (C::PeerConnectionClosed, Some(Details::Closed { reason: Some("PCPermission".into()) }))
                }
            }
            PhoneState::ConnectionUnstable => (C::PhoneStateConnectionUnstable, None),
            PhoneState::NearbyConnectionNeeded => (C::NearbyConnectionNeeded, None),
            PhoneState::ScreenScrapePermissionNeeded => (C::ScreenScrapePermissionNeeded, None),
            PhoneState::Connecting => (C::PhoneStateConnecting, None),
            PhoneState::Connected => (C::PhoneStateConnected, None),
            PhoneState::FoldedCannotDraw if applies(&args.lock) => (C::PhoneStateFoldedCannotDraw, None),
            PhoneState::Unlocked => (C::PhoneStateUnlocked, None),
            PhoneState::ScreenOff if applies(&args.lock) => {
                (C::PhoneStateScreenOff, Some(Details::Locked { secure: false, virtual_device_streaming: vd }))
            }
            PhoneState::Error => {
                let code = if self.all_features_disabled() {
                    code::ALL_FEATURES_DISABLED
                } else {
                    args.error.unwrap_or(code::GENERIC)
                };
                (C::PhoneStateError, Some(Details::Error { code }))
            }
            PhoneState::SecureLocked | PhoneState::Locked if applies(&args.lock) => (
                C::PhoneStateLocked,
                Some(Details::Locked { secure: phone == PhoneState::SecureLocked, virtual_device_streaming: vd }),
            ),
            _ => return None,
        })
    }

    pub(super) fn fanout(&self, st: &mut State, phone: PhoneState, args: PhoneArgs, out: &mut Out) {
        let allow_apps = self.settings().allow_apps;
        let ids: Vec<String> = st.sessions.keys().cloned().collect();
        for id in ids {
            let cmd = self.fanout_command(&st.sessions[&id], phone, &args, allow_apps);
            if let Some((c, d)) = cmd {
                if c == SessionCommand::PeerConnectionClosed {
                    Self::peer_connection_closed(st, &id);
                }
                Self::apply_session(st, &id, c, d, out);
            }
        }
        st.last_fanout = (phone, args);
    }

    /// A window's peer connection ends without the phone saying why ("allow apps" turned off): its
    /// channels close, it is no longer a task transfer, and every pending request of the feature is
    /// dropped — a waiting resume ends as not resumable and the audio questions go unanswered.
    pub(super) fn peer_connection_closed(st: &mut State, id: &str) {
        if let Some(s) = st.sessions.get_mut(id) {
            s.unbind();
            s.transfer_task_id = None;
        }
        st.transactions.clear();
    }

    /// Applies a phone command; when the table allows it and it moves the state, or carries values,
    /// the windows get the new state.
    pub(super) fn apply_phone(&self, st: &mut State, command: PhoneCommand, args: PhoneArgs, out: &mut Out) -> bool {
        let Some(next) = phone_next(st.phone, command) else { return false };
        if next == st.phone && !args.has_details() {
            return false;
        }

        let from = st.phone;
        st.phone = next;
        if next == PhoneState::Error {
            st.phone_error = args.error.unwrap_or(code::GENERIC);
        }
        if next != PhoneState::NearbyConnectionNeeded {
            st.nearby = None;
        }

        st.phone_epoch += 1;
        out.events.push(ScreenMirroringEvent::Changed);
        if matches!(next, PhoneState::Connecting | PhoneState::ConnectionUnstable) {
            out.phone_watch = Some(st.phone_epoch);
        }

        info!(?command, from_state = ?from, state = ?next, code = args.error, "phone state changed");
        self.fanout(st, next, args, out);
        true
    }

    pub(super) fn phone_command(&self, command: PhoneCommand, args: PhoneArgs) {
        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            self.apply_phone(&mut st, command, args, &mut out);
        }
        self.flush(out);
    }

    pub(super) fn session_command(&self, id: &str, command: SessionCommand, details: Option<Details>) {
        let mut out = Out::default();
        Self::apply_session(&mut self.state.lock_or_panic(), id, command, details, &mut out);
        self.flush(out);
    }

    pub(super) fn watch_session(&self, id: String, epoch: u64) {
        let me = self.weak.clone();
        let after = self.config.watchdog;
        self.spawn(async move {
            tokio::time::sleep(after).await;
            let Some(me) = me.upgrade() else { return };

            let mut out = Out::default();
            {
                let mut st = me.state.lock_or_panic();
                let fire = st.sessions.get(&id).is_some_and(|s| s.epoch == epoch);
                // The state change's line (command `StateMachineTimeout`) tells the timeout.
                if fire {
                    Self::apply_session(
                        &mut st,
                        &id,
                        SessionCommand::StateMachineTimeout,
                        Some(Details::Error { code: code::LAUNCH_TIMEOUT }),
                        &mut out,
                    );
                }
            }

            me.flush(out);
        });
    }

    pub(super) fn watch_phone(&self, epoch: u64) {
        let me = self.weak.clone();
        let after = self.config.watchdog;
        self.spawn(async move {
            tokio::time::sleep(after).await;
            let Some(me) = me.upgrade() else { return };

            let mut out = Out::default();
            {
                let mut st = me.state.lock_or_panic();
                // The state change's line (command `StateMachineTimeout`) tells the timeout.
                if st.phone_epoch == epoch {
                    me.apply_phone(
                        &mut st,
                        PhoneCommand::StateMachineTimeout,
                        PhoneArgs::error(code::GENERIC),
                        &mut out,
                    );
                }
            }

            me.flush(out);
        });
    }
}
