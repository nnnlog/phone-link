//! The windows: creating them, checking what can launch, and launching apps and notifications.

use super::{Inner, Out, PendingPhoneScreen, PhoneArgs, SetupFailure, State, send_on};
use crate::drag::WindowDrag;
use crate::errors::setup_code;
use crate::gates::{AppsFeatureType, PhoneValues};
use crate::machine::{PhoneCommand, PhoneState, SessionCommand, WindowState};
use crate::session::{Session, TrueTouchBypass};
use crate::wire::{PHONE_SCREEN_PACKAGE, launch_flags, scope, screen_layout};
use crate::{MirroringError, ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::new_guid_lower;
use phonelink_notifications::outbound::NotificationsRequest;
use std::sync::Arc;
use tracing::{debug, info, warn};

impl Inner {
    pub(super) fn create_session(&self, st: &mut State, id: String, package: &str, pc_initiated: bool, out: &mut Out) {
        let order = st.next_order;
        st.next_order += 1;

        let settings = self.settings();
        let mut session =
            Session::new(id.clone(), package.to_string(), order, pc_initiated, settings.do_not_show_true_touch);
        session.drag = Some(WindowDrag::new(
            id.clone(),
            session.input.clone(),
            self.events.clone(),
            self.runtime.clone(),
            self.env.notifier.clone(),
            self.drag_staging.join(&id),
        ));

        if let Some(record) = self.env.app.phone_record().and_then(|r| r.device_resolution) {
            session.display.start_with_record_resolution(&record);
            if let Some(size) = session.display.device_resolution() {
                session.input.lock_or_panic().set_device_resolution(Some(size));
            }
        }

        st.sessions.insert(id.clone(), session);
        info!(session = %id, package, pc_initiated, "window opened");
        out.events.push(ScreenMirroringEvent::SessionOpened {
            session_id: id.clone(),
            package_name: package.to_string(),
            phone_initiated: !pc_initiated,
        });

        // A new window gets the last phone state the windows were given; after an error, the
        // disconnected state (it starts again).
        let (phone, args) = match &st.last_fanout {
            (PhoneState::Error, _) => (PhoneState::Disconnected, PhoneArgs::default()),
            (p, a) => (*p, a.clone()),
        };
        if let Some((c, d)) = self.fanout_command(&st.sessions[&id], phone, &args, settings.allow_apps) {
            if c == SessionCommand::PeerConnectionClosed {
                Self::peer_connection_closed(st, &id);
            }
            Self::apply_session(st, &id, c, d, out);
        }

        out.session_watch.push((id.clone(), 0));
    }

    pub(super) fn check_launchable(&self) -> Result<PhoneValues, MirroringError> {
        let v = self.values()?;
        if !v.remoting_supported() {
            return Err(MirroringError::NotSupported("remoting is not supported"));
        }
        if !v.phone_screen_apps() {
            return Err(MirroringError::NotSupported("the phone has no phone screen"));
        }
        Ok(v)
    }

    pub(super) fn find_package(st: &State, package: &str) -> Option<String> {
        st.sessions.values().filter(|s| s.package == package).min_by_key(|s| s.order).map(|s| s.id.clone())
    }

    pub(in super::super) fn open_phone_screen(
        &self,
        pending: Option<PendingPhoneScreen>,
    ) -> Result<String, MirroringError> {
        self.open_phone_screen_as(pending, None)
    }

    /// Opens the phone-screen window (with `id` as its session id when given) or, when it is open,
    /// raises it: an app to open goes into it, and the phone is asked for its consent (unless it
    /// gave it) and woken.
    pub(super) fn open_phone_screen_as(
        &self,
        pending: Option<PendingPhoneScreen>,
        id: Option<String>,
    ) -> Result<String, MirroringError> {
        self.check_launchable()?;
        // Every launch of the phone screen replaces the notification waiting for it.
        let (pending, notification) = match pending {
            Some(PendingPhoneScreen::Notification(k, i)) => (None, Some((k, i))),
            other => (other, None),
        };

        self.state.lock_or_panic().pending_notification = notification;
        self.ensure_ready_for_launch();

        let mut out = Out::default();
        let existing = Self::find_package(&self.state.lock_or_panic(), PHONE_SCREEN_PACKAGE);
        if let Some(id) = existing {
            if let Some(PendingPhoneScreen::App(p)) = pending {
                self.send_orchestrator(scope::LAUNCH_PHONE_SCREEN_APP, &wire::launch_phone_screen_app(&p, &id, 1));
            }
            let _ = self.events.send(ScreenMirroringEvent::BringToFront { session_id: id.clone() });
            self.wake_with_permission();
            return Ok(id);
        }

        let id = id.unwrap_or_else(new_guid_lower);
        {
            let mut st = self.state.lock_or_panic();
            self.create_session(&mut st, id.clone(), PHONE_SCREEN_PACKAGE, true, &mut out);
            let s = st.sessions.get_mut(&id).expect("created");
            if let Some(PendingPhoneScreen::App(p)) = pending {
                s.pending_app = Some(p);
            }
        }

        self.flush(out);
        self.spawn_launch(id.clone(), false);
        Ok(id)
    }

    /// The phone has a window that counts as live (connected, or showing the phone's lock).
    pub(super) fn has_valid_session(st: &State) -> bool {
        st.sessions
            .values()
            .any(|s| matches!(s.machine.state, WindowState::Connected | WindowState::Locked | WindowState::ScreenOff))
    }

    /// The screen-capture consent asked again (unless the phone gave it) and the phone's screen
    /// woken, each only on its open channel.
    pub(super) fn wake_with_permission(&self) {
        self.request_permission_if_needed();
        self.send_orchestrator(scope::WAKE_SCREEN, &wire::empty_orchestrator());
    }

    /// `requestPermission` on the open permission workflow unless the phone accepted.
    pub(super) fn request_permission_if_needed(&self) -> bool {
        let (channel, accepted) = {
            let st = self.state.lock_or_panic();
            (st.permission.clone(), st.permission_status.as_deref() == Some("accept"))
        };

        if accepted {
            debug!("requestPermission not sent; the phone already accepted screen capture");
            return false;
        }
        send_on(&channel, scope::REQUEST_PERMISSION, &wire::request_permission())
    }

    /// Before a launch: every window the relaunch moves starts again, and a phone state in error is
    /// reset.
    pub(super) fn ensure_ready_for_launch(&self) {
        self.relaunch_all_if_needed(false);
        if self.state.lock_or_panic().phone == PhoneState::Error {
            self.phone_command(PhoneCommand::RetryConnection, PhoneArgs::default());
        }
    }

    /// Gives every window the relaunch-all command; a window it moves (or, with `cleanup`, one
    /// showing the phone's lock) runs the set-up and its launch again. `cleanup` first closes all but
    /// the first window of each package.
    pub(super) fn relaunch_all_if_needed(&self, cleanup: bool) {
        if cleanup {
            let duplicates: Vec<String> = {
                let st = self.state.lock_or_panic();
                let mut v: Vec<&Session> = st.sessions.values().collect();
                v.sort_by_key(|s| s.order);
                let mut seen = std::collections::HashSet::new();
                v.into_iter().filter(|s| !seen.insert(s.package.clone())).map(|s| s.id.clone()).collect()
            };
            for id in duplicates {
                let _ = self.close_session(&id);
            }
        }

        let ids: Vec<(String, WindowState)> = {
            let st = self.state.lock_or_panic();
            let mut v: Vec<&Session> = st.sessions.values().collect();
            v.sort_by_key(|s| s.order);
            v.into_iter().map(|s| (s.id.clone(), s.machine.state)).collect()
        };

        for (id, before) in ids {
            let mut out = Out::default();
            let changed = {
                let mut st = self.state.lock_or_panic();
                let changed = Self::apply_session_preserving(&mut st, &id, SessionCommand::RelaunchAll, &mut out);
                let again = changed || (cleanup && before == WindowState::Locked);
                if again {
                    if let Some(s) = st.sessions.get_mut(&id) {
                        s.pc_initiated = true;
                    }
                }
                again
            };

            self.flush(out);
            if changed {
                self.spawn_launch(id, false);
            }
        }
    }

    pub(in super::super) fn launch_app(
        &self,
        package: &str,
        transfer_task_id: Option<i64>,
    ) -> Result<String, MirroringError> {
        let v = self.check_launchable()?;
        if package == PHONE_SCREEN_PACKAGE {
            return self.open_phone_screen(None);
        }

        // A recent task always opens in a window of its own; other apps without app windows open
        // inside the phone-screen window.
        if transfer_task_id.is_none() && v.apps_feature_type(self.separate_window()) != AppsFeatureType::MultipleApps {
            return self.open_phone_screen(Some(PendingPhoneScreen::App(package.to_string())));
        }

        self.ensure_ready_for_launch();

        // With no live window the phone is asked for its consent and woken before the launch (not
        // for a phone that runs the nearby check, which unlocks it).
        let wake = !v.nearby() && !Self::has_valid_session(&self.state.lock_or_panic());
        if wake {
            self.wake_with_permission();
        }

        let existing = {
            let st = self.state.lock_or_panic();
            Self::find_package(&st, package).map(|id| (id.clone(), st.sessions[&id].machine.state))
        };
        if let Some((id, state)) = existing {
            if matches!(
                state,
                WindowState::Loading
                    | WindowState::NearbyConnectionNeeded
                    | WindowState::ScreenScrapePermissionNeeded
                    | WindowState::AudioStreamingPermissionNeeded
            ) {
                debug!(session = %id, window_state = ?state, "launch of a window still launching; ignored");
                return Ok(id);
            }

            let _ = self.events.send(ScreenMirroringEvent::BringToFront { session_id: id.clone() });
            if state == WindowState::Suspended {
                // A suspended window opened again from the app list launches its app again.
                let mut out = Out::default();
                {
                    let mut st = self.state.lock_or_panic();
                    Self::apply_session_preserving(&mut st, &id, SessionCommand::RelaunchApp, &mut out);
                    if let Some(s) = st.sessions.get_mut(&id) {
                        s.pc_initiated = true;
                        s.true_touch = TrueTouchBypass::Unavailable;
                    }
                }

                self.flush(out);
                self.ensure_ready_for_launch();
                self.spawn_launch(id.clone(), false);
            }
            return Ok(id);
        }

        let id = new_guid_lower();
        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            self.create_session(&mut st, id.clone(), package, true, &mut out);
            let s = st.sessions.get_mut(&id).expect("created");
            s.transfer_task_id = transfer_task_id;
        }

        self.flush(out);
        self.spawn_launch(id.clone(), false);
        Ok(id)
    }

    /// Runs the set-up for session `id`, then (for a window the PC launches) its launch, past the
    /// phone's launch checks when `bypass`.
    pub(super) fn spawn_launch(&self, id: String, bypass: bool) {
        let me = self.me();
        self.spawn(async move { me.launch(&id, bypass).await });
    }

    pub(super) async fn launch(self: Arc<Self>, id: &str, bypass: bool) {
        let epoch = self.state.lock_or_panic().setup_epoch;
        let result = self.ensure_setup(id).await;

        if !self.state.lock_or_panic().sessions.contains_key(id) {
            debug!(session = id, "window closed during its set-up; no launch");
            return;
        }
        if let Err(failure) = result {
            return self.setup_failed(&failure);
        }
        if self.state.lock_or_panic().setup_epoch != epoch {
            debug!(session = id, "mirroring released during the set-up; no launch");
            return;
        }

        let Some((pc_initiated, pending_app)) = self.take_launch(id, bypass) else { return };
        if !pc_initiated {
            return;
        }

        self.send_launch_app(id, bypass);
        if let Some(p) = pending_app {
            self.send_orchestrator(scope::LAUNCH_PHONE_SCREEN_APP, &wire::launch_phone_screen_app(&p, id, 1));
            self.send_orchestrator(scope::WAKE_SCREEN, &wire::empty_orchestrator());
        }
    }

    /// Shows the failure of the set-up session `id` waited for (the set-up logged it, once for all
    /// its windows).
    fn setup_failed(&self, failure: &SetupFailure) {
        match failure {
            SetupFailure::Closed(code) => self.report_closed(*code),
            other => {
                let ice = self.values().is_ok_and(|v| v.ice_support());
                let code = setup_code(other.code(), ice);
                self.phone_command(PhoneCommand::OnConnectionError, PhoneArgs::error(code));
            }
        }
    }

    /// Readies session `id` for its launch after the set-up (its touch bypass ends unless `bypass`):
    /// whether the PC launches it and the app the phone screen opens then, `None` when the session is gone.
    fn take_launch(&self, id: &str, bypass: bool) -> Option<(bool, Option<String>)> {
        let mut st = self.state.lock_or_panic();
        let s = st.sessions.get_mut(id)?;
        if !bypass {
            s.true_touch = TrueTouchBypass::Unavailable;
        }
        Some((s.pc_initiated, if s.pc_initiated { s.pending_app.take() } else { None }))
    }

    /// The phone state closes and every window shows `code` (a set-up the Mac stopped).
    pub(super) fn report_closed(&self, code: i32) {
        let mut out = Out::default();
        {
            let mut st = self.state.lock_or_panic();
            self.apply_phone_quiet(&mut st, PhoneCommand::OnConnectionClosed, &mut out);
            self.fanout(&mut st, PhoneState::Error, PhoneArgs::error(code), &mut out);
        }
        self.flush(out);
    }

    /// `launchApp` for session `id`: the phone screen with flags 3; an app with the bypass flag when
    /// `bypass`, and as a task transfer (flag 4, its task id) while it continues a recent task.
    pub(super) fn send_launch_app(&self, id: &str, bypass: bool) {
        let Some((package, transfer)) =
            self.state.lock_or_panic().sessions.get(id).map(|s| (s.package.clone(), s.transfer_task_id))
        else {
            return;
        };

        let flags = if package == PHONE_SCREEN_PACKAGE {
            launch_flags::BYPASS_ANDROID_LAUNCH_CHECKS | launch_flags::IS_PHONE_SCREEN
        } else {
            let mut f = if bypass { launch_flags::BYPASS_ANDROID_LAUNCH_CHECKS } else { 0 };
            if transfer.is_some() {
                f |= launch_flags::IS_TASK_TRANSFER_LAUNCH;
            }
            f
        };

        let saved = if package == PHONE_SCREEN_PACKAGE { 0 } else { self.package(&package).screen_layout };
        let layout = self.layout_override(saved);
        let body = wire::launch_app(&package, id, layout, transfer, flags);

        if self.send_orchestrator(scope::LAUNCH_APP, &body) {
            info!(session = id, package = %package, flags, "launchApp sent");
        }
    }

    /// A layout as the settings make it: Default without adaptive layout, Expanded with "always
    /// expanded".
    pub(super) fn layout_override(&self, layout: i32) -> i32 {
        if !self.values().is_ok_and(|v| v.adaptive_layout()) {
            screen_layout::DEFAULT
        } else if self.settings().always_expanded {
            screen_layout::EXPANDED
        } else {
            layout
        }
    }

    pub(super) fn send_orchestrator(&self, target: &str, body: &[u8]) -> bool {
        let c = self.state.lock_or_panic().orchestrator.clone();
        send_on(&c, target, body)
    }

    /// The phone-screen window became active: the notification waiting for it opens on the phone,
    /// while the phone has the phone screen (else it keeps waiting).
    pub(super) fn open_pending_notification(&self, key: String, index: i32) {
        if !self.values().is_ok_and(|v| v.phone_screen_apps()) {
            let mut st = self.state.lock_or_panic();
            if st.pending_notification.is_none() {
                st.pending_notification = Some((key, index));
            }
            return;
        }

        let me = self.me();
        self.spawn(async move { me.notification_on_phone(&key, index).await });
    }

    /// `/notifications` operation 5 for `key` (the phone-screen window became active).
    pub(super) async fn notification_on_phone(self: Arc<Self>, key: &str, action_index: i32) {
        let request = NotificationsRequest::open_on_phone(key, action_index).app_request_user_traced();
        match self.env.app.request(request).await {
            Ok(r) => {
                let opened = r.outcome.is_success();
                info!(opened, "notification open on the phone answered");
                let _ = self.events.send(ScreenMirroringEvent::NotificationLaunched { key: key.to_string(), opened });
            }
            Err(e) => warn!(error = %e, "notification open on the phone not sent"),
        }
    }
}
