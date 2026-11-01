//! The virtual camera device on the Mac: registering, removing and its controls.

use super::{Action, CameraError, CameraState, ConnectedCameraEvent, Inner, device_id, key, placeholder_for};
use phonelink_core::types::DcgClient;
use phonelink_transport::auth::PeerDevice;
use std::fmt::Display;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

impl Inner {
    pub(super) fn register_if_needed(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        let data = self.data_or_default(phone);
        let should =
            self.camera.is_available() && self.with_phone(phone, |p| p.listening) && self.should_register(peer, &data);
        let registered = self.with_phone(phone, |p| p.registered);

        if registered && !should {
            self.unregister(phone);
        } else if !registered && should {
            self.register(peer);
        }
    }

    pub(super) fn register(&self, peer: &PeerDevice) {
        let phone = &peer.client;
        if self.with_phone(phone, |p| p.registered) {
            return;
        }
        if !self.config.multiple_cameras && !self.other_registered(phone).is_empty() {
            info!(%phone, "camera not published; another phone's camera is");
            return;
        }

        let name = peer.info.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Phone".to_owned());
        if let Err(e) = self.camera.publish(device_id(phone), name) {
            error!(%phone, error = %e, "camera not published; the setting offers a retry");
            self.with_phone(phone, |p| p.registration_failed = true);
            self.changed(phone);
            return;
        }

        info!(%phone, "camera published");

        let (tx, rx) = mpsc::channel(1);
        let stop = self.shutdown.child_token();
        self.with_phone(phone, |p| {
            p.registered = true;
            p.registration_failed = false;
            p.permission_requested = false;
            p.placeholder = None;
            p.signals = Some(tx);
            p.signals_stop = stop.clone();
        });

        self.spawn_client_signals(phone.clone(), rx, stop);
        self.set_state(phone, CameraState::Registered, CameraError::None);
        self.changed(phone);
    }

    pub(super) fn unregister(&self, phone: &DcgClient) {
        let (registered, state) = self.with_phone(phone, |p| (p.registered, p.state));
        if !registered && state == CameraState::Unregistered {
            return;
        }

        if !state.is_idle() {
            self.set_state(phone, CameraState::Registered, CameraError::None);
            if let Err(e) = self.send_action(phone, Action::Stop, None, None) {
                debug!(%phone, error = %e, "no stream channel; no Stop sent");
            }
        }

        if registered {
            self.camera.remove(device_id(phone));
            info!(%phone, "camera removed");
        }

        self.close_stream(phone);
        self.with_phone(phone, |p| {
            p.registered = false;
            p.signals = None;
            p.signals_stop.cancel();
        });

        self.set_state(phone, CameraState::Unregistered, CameraError::None);
        self.changed(phone);
    }

    /// The client start/stop consumer: a repeat of the last signal is ignored, and each handled one is
    /// followed by a pause.
    pub(super) fn spawn_client_signals(&self, phone: DcgClient, mut rx: mpsc::Receiver<bool>, stop: CancellationToken) {
        let weak = self.weak.clone();
        let pause = self.config.client_signal_pause;

        self.runtime.spawn(async move {
            let mut last = None;
            loop {
                let started = tokio::select! {
                    _ = stop.cancelled() => return,
                    s = rx.recv() => match s { Some(s) => s, None => return },
                };

                if last == Some(started) {
                    continue;
                }
                last = Some(started);

                let Some(inner) = weak.upgrade() else { return };
                info!(%phone, started, "camera client started or stopped");
                if started {
                    if let Some(peer) = inner.env.peers.peer(&phone) {
                        inner.spawn_open_stream(peer);
                    }
                } else {
                    inner.close_stream(&phone);
                }

                drop(inner);
                tokio::time::sleep(pause).await;
            }
        });
    }

    /// Moves to `state` with `error` and shows its still image.
    pub(super) fn set_state(&self, phone: &DcgClient, state: CameraState, error: CameraError) {
        self.transition(phone, state, error, None);
    }

    /// [`Inner::set_state`] for a failure of the PC's own: its line carries `cause`.
    pub(super) fn set_failed_state(&self, phone: &DcgClient, error: CameraError, cause: &dyn Display) {
        self.transition(phone, CameraState::Error, error, Some(cause));
    }

    fn transition(&self, phone: &DcgClient, state: CameraState, error: CameraError, cause: Option<&dyn Display>) {
        let auto_prompt = self.config.auto_prompt;
        let (changed, show, window_closed) = self.with_phone(phone, |p| {
            if p.state == state && p.error == error {
                return (false, None, false);
            }

            p.state = state;
            p.error = error;

            let window_closed = state.is_idle() && std::mem::replace(&mut p.control_window, false);
            let image = placeholder_for(state, error, auto_prompt);
            let show = match image {
                Some(i) if p.placeholder != Some(i) => {
                    p.placeholder = Some(i);
                    p.registered.then_some(i)
                }
                _ => None,
            };
            (true, show, window_closed)
        });

        log_transition(phone, state, error, cause, changed);
        if !changed {
            return;
        }

        if let Some(image) = show {
            self.camera.show_placeholder(device_id(phone), image);
        }
        if window_closed {
            self.emit(ConnectedCameraEvent::ControlWindow { phone: key(phone), shown: false });
        }
        self.changed(phone);
    }

    pub(super) fn set_control_window(&self, phone: &DcgClient, shown: bool) {
        if self.with_phone(phone, |p| std::mem::replace(&mut p.control_window, shown)) != shown {
            self.emit(ConnectedCameraEvent::ControlWindow { phone: key(phone), shown });
        }
    }
}

/// The one line of a state change: a failure of the PC's own (`cause`, logged even when the state
/// stays) at `warn`, an error state the phone reported or a permission needs at `info`, the rest at
/// `debug` (the stream's and the camera's lifecycle lines say the same).
fn log_transition(
    phone: &DcgClient,
    state: CameraState,
    error: CameraError,
    cause: Option<&dyn Display>,
    changed: bool,
) {
    match cause {
        Some(cause) => log_failure(phone, error, cause),
        None if !changed => {}
        None if matches!(state, CameraState::Error | CameraState::TerminalError) => {
            info!(%phone, ?state, error_kind = ?error, "camera in an error state");
        }
        None => debug!(%phone, ?state, error_kind = ?error, "camera state changed"),
    }
}

/// The line of a stream the PC's own `cause` ended in error `error`.
fn log_failure(phone: &DcgClient, error: CameraError, cause: &dyn Display) {
    warn!(%phone, error_kind = ?error, reason = %cause, "camera stream failed; the camera shows the error");
}
