//! The audio workflow: its permission, channel and controls.

use super::{Inner, Scoped, merged, send_on};
use crate::edge::AudioStreamFormat;
use crate::link::StartedWorkflow;
use crate::wire::{Body, PHONE_SCREEN_PACKAGE, property, scope, workflow};
use crate::{AudioButton, ScreenMirroringEvent, wire};
use phonelink_core::sync::MutexExt;
use phonelink_nano::channel::audio::{AudioEvent, AudioTarget};
use phonelink_nano::channel::{ChannelType, RawChannel};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

impl Inner {
    /// The `audio` workflow, once per `remote` start while it is not running. A start that fails is
    /// only logged: the next `remote` start tries again.
    pub(super) async fn open_audio(self: Arc<Self>) {
        let started = match self.links.start(workflow::AUDIO, wire::AUDIO_INBOUND, false, true).await {
            Ok(s) => s,
            Err(e) => return warn!(error = %e, "audio workflow did not open; the next remote start tries again"),
        };

        let output = self.presenter.has_audio_output();
        let stop = {
            let mut st = self.state.lock_or_panic();
            st.audio = Some(started.channel.clone());
            st.audio_output = Some(output);
            st.audio_stop = CancellationToken::new();
            st.audio_stop.clone()
        };

        self.send_streaming_allowed(output);
        let ids: Vec<String> = self.state.lock_or_panic().sessions.keys().cloned().collect();
        for id in ids {
            self.apply_initial_volume(&id);
        }

        let StartedWorkflow { inbound, mut others, channel, .. } = started;
        let mut inbound = merged(inbound, channel.closed());
        let me = self.clone();
        self.spawn(async move {
            while let Some(c) = others.next().await {
                me.audio_channel(c, stop.clone());
            }
        });

        while let Some(m) = inbound.recv().await {
            self.audio_message(&m);
        }

        let mut st = self.state.lock_or_panic();
        if st.audio.as_ref().is_some_and(|a| Arc::ptr_eq(a, &channel)) {
            st.audio = None;
        }
    }

    /// One message of the `audio` workflow, taken at whatever target its scope receives.
    pub(super) fn audio_message(self: &Arc<Self>, scoped: &Scoped) {
        let m = &scoped.m;
        let body = match Body::parse(&m.body) {
            Ok(b) => b,
            Err(e) => return warn!(error = %e, route = %scoped.scope, "malformed audio message; dropped"),
        };
        let package = body.opt_str("packageName");
        match scoped.scope.as_str() {
            scope::AUDIO_PHONE_CONFIG => self.on_audio_phone_config(&body),
            scope::AUDIO_STREAM_READY => self.on_audio_stream_ready(package),
            scope::AUDIO_VOLUME_CHANGED => self.on_audio_volume_changed(package, &body),
            scope::AUDIO_SILENCE_STATE_CHANGED => self.on_audio_silence_state_changed(package, &body),
            scope::AUDIO_CAPTURE_ALLOWED => self.on_audio_capture_allowed(package, &body),
            other => warn!(raw_target = other, "audio message without a handler; dropped"),
        }
    }

    /// The phone's audio configuration: its frame length lets the held audio channels open.
    fn on_audio_phone_config(self: &Arc<Self>, body: &Body) {
        // Audio channels wait for a frame length: without one (or with 0) they keep waiting.
        let Some(length) = body.opt_int("frameLengthMs").filter(|l| *l > 0).map(|l| l as u32) else {
            return warn!("audio configuration without a frame length; audio channels keep waiting");
        };

        let held = {
            let mut st = self.state.lock_or_panic();
            st.frame_length = Some(length);
            std::mem::take(&mut st.held_audio)
        };

        info!(frame_length_ms = length, "audio phoneConfig received");
        let stop = self.state.lock_or_panic().audio_stop.clone();
        for c in held {
            self.audio_channel(c, stop.clone());
        }
    }

    /// An app's audio stream is ready: its stored volume (0 when muted) goes to the phone.
    fn on_audio_stream_ready(&self, package: Option<String>) {
        if let Some(p) = package.filter(|p| p != PHONE_SCREEN_PACKAGE) {
            let d = self.package(&p);
            let c = self.state.lock_or_panic().audio.clone();
            send_on(&c, scope::AUDIO_SET_VOLUME, &wire::audio_set_volume(&p, if d.muted { 0 } else { d.volume }));
        }
    }

    /// The phone changed an app's volume.
    fn on_audio_volume_changed(&self, package: Option<String>, body: &Body) {
        // 0 is muted, and the stored level becomes 0 too; muted shows the volume button.
        if let (Some(p), Some(v)) = (package, body.opt_int("volume")) {
            let v = v.clamp(0, 100);
            if v == 0 {
                self.set_audio_button(&p, AudioButton::Volume);
            }

            if let Err(e) = self.save_package(&p, |d| {
                d.muted = v == 0;
                d.volume = v;
            }) {
                error!(error = %e, "app volume not saved; the app opens at its earlier volume");
            }

            self.audio_changed(&p);
        }
    }

    /// An app went silent or made sound.
    fn on_audio_silence_state_changed(&self, package: Option<String>, body: &Body) {
        // Sound, or a muted app, shows the volume button; silence hides it.
        if let (Some(p), Some(silent)) = (package, body.opt_bool("isSilent")) {
            let button = if !silent || self.package(&p).muted { AudioButton::Volume } else { AudioButton::None };
            self.set_audio_button(&p, button);
            self.state.lock_or_panic().package_audio.entry(p.clone()).or_default().silent = silent;
            self.audio_changed(&p);
        }
    }

    /// Whether an app's audio may be captured.
    fn on_audio_capture_allowed(&self, package: Option<String>, body: &Body) {
        // Per window, for the windows of the app open now (a later one starts allowed).
        if let (Some(p), Some(allowed)) = (package, body.opt_bool("isAudioCaptureAllowed")) {
            for s in self.state.lock_or_panic().sessions.values_mut().filter(|s| s.package == p) {
                s.audio_capture_allowed = allowed;
            }
            self.audio_changed(&p);
        }
    }

    /// Every window of `package` is given audio button `button`.
    pub(super) fn set_audio_button(&self, package: &str, button: AudioButton) {
        for s in self.state.lock_or_panic().sessions.values_mut().filter(|s| s.package == package) {
            s.audio_button = button;
        }
    }

    /// Window `id` is given audio button `button`.
    pub(super) fn set_window_audio_button(&self, id: &str, button: AudioButton) {
        if let Some(s) = self.state.lock_or_panic().sessions.get_mut(id) {
            s.audio_button = button;
        }
        let _ = self.events.send(ScreenMirroringEvent::SessionChanged { session_id: id.to_string() });
    }

    /// An app window's stored mute shows its volume button, once the window and the `audio`
    /// workflow are both there (the phone screen's is not applied).
    pub(super) fn apply_initial_volume(&self, id: &str) {
        let package = {
            let mut st = self.state.lock_or_panic();
            let audio_open = st.audio.is_some();
            let Some(s) = st.sessions.get_mut(id) else { return };
            if s.is_phone_screen() || s.initial_volume_set || !audio_open {
                return;
            }
            s.initial_volume_set = true;
            s.package.clone()
        };

        if self.package(&package).muted {
            self.set_window_audio_button(id, AudioButton::Volume);
        }
    }

    pub(super) fn audio_changed(&self, package: &str) {
        let ids: Vec<String> = self
            .state
            .lock_or_panic()
            .sessions
            .values()
            .filter(|s| s.package == package)
            .map(|s| s.id.clone())
            .collect();
        for id in ids {
            let _ = self.events.send(ScreenMirroringEvent::SessionChanged { session_id: id });
        }
    }

    /// One `AudioTarget` channel of the `audio` workflow: held until the phone's frame length is
    /// known, then opened and played until it closes or `stop` (the workflow ended).
    /// The `audio` workflow ended (the phone stopped it, or the connection closed): its audio
    /// channels close, the held ones are dropped and the frame length is forgotten. Nothing else is
    /// released; the workflow starts again with the next `remote` start.
    pub(super) fn audio_ended(&self) {
        let mut st = self.state.lock_or_panic();
        st.audio = None;
        st.audio_output = None;
        st.audio_stop.cancel();
        st.frame_length = None;
        for c in st.held_audio.drain(..) {
            c.close();
        }
    }

    pub(super) fn audio_channel(self: &Arc<Self>, c: RawChannel, stop: CancellationToken) {
        if c.announcement.channel_type != Some(ChannelType::AudioTarget) {
            warn!(raw_announcement = ?c.announcement, "audio workflow channel that is not audio; closed");
            return c.close();
        }
        let Some(package) = c.announcement.property(property::PACKAGE_NAME).map(str::to_string) else {
            warn!("audio channel without a packageName; closed");
            return c.close();
        };

        let length = {
            let mut st = self.state.lock_or_panic();
            match st.frame_length {
                Some(l) => l,
                None => {
                    st.held_audio.push(c);
                    return;
                }
            }
        };

        let me = self.clone();
        self.spawn(async move {
            let mut target = match AudioTarget::open(c).await {
                Ok(t) => t,
                Err(e) => return warn!(error = %e, package = %package, "audio channel did not open; the app plays no sound on the Mac"),
            };

            loop {
                let e = tokio::select! {
                    e = target.next_event() => e,
                    _ = stop.cancelled() => {
                        target.close();
                        break;
                    }
                };

                let Some(e) = e else { break };
                match e {
                    AudioEvent::Ready { format, .. } => {
                        info!(package = %package, "audio stream started");
                        let (bytes, float) = format.pcm.unwrap_or((0, 0));
                        me.presenter.audio_started(
                            package.clone(),
                            AudioStreamFormat {
                                channels: format.channels,
                                sample_rate: format.sample_rate,
                                codec: format.codec,
                                bytes_per_sample: bytes,
                                is_float: float != 0,
                                frame_length_ms: length,
                            },
                        );
                    }
                    AudioEvent::Frame(f) => me.presenter.audio_frame(package.clone(), f.data, f.timestamp),
                    AudioEvent::Closed => break,
                }
            }

            info!(package = %package, "audio stream ended");
            me.presenter.audio_ended(package);
        });
    }
}
