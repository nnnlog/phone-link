//! Media playback: what plays on the phone (its media sessions), the controls for it and the session
//! they act on, the phone's media-controls setting, and — while the user lets them — the desktop's
//! media controls and media keys (an MPRIS player) for the selected session.
//!
//! `query` names: `media`. `perform` names: `media.*`.
//!
//! The QML hears `areaEvent("media", "systemControls", {"on": bool})` when the user changes whether
//! the desktop's media controls show the phone's media, which the app's settings keep; `FilesHost`
//! reports the kept choice with `media.systemControls` whenever the engine starts or the media changes.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use phonelink_engine::api::FeatureApiError;
use phonelink_engine::api::audio::{AudioChange, MediaControlsState, MediaSessionView};
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::{debug, warn};

use super::EngineBridge;
use super::features::{Failure, Revision};
use crate::images;
use crate::l10n::translate;
use crate::mpris::{Command, NowPlaying, Player};

/// What the playback part keeps between calls.
#[derive(Default)]
pub(super) struct State {
    /// The user lets the desktop's media controls and media keys control the phone's media (a choice
    /// the app's settings keep; off until `FilesHost` reports it).
    system_controls: bool,
    /// The MPRIS player, while it shows the phone's media.
    player: Option<Player>,
}

/// A media session as the controls show it: the app's name ("Media" when the phone sent none), the
/// title (the app's name when the phone sent none), and the artist when there is one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Session {
    id: i64,
    app: String,
    title: String,
    artist: Option<String>,
    playing: bool,
}

impl Session {
    fn of(view: &MediaSessionView) -> Self {
        let non_empty = |text: &Option<String>| text.clone().filter(|text| !text.is_empty());
        let app = non_empty(&view.app_name).unwrap_or_else(|| translate("Media").to_owned());
        Self {
            id: view.id,
            title: non_empty(&view.title).unwrap_or_else(|| app.clone()),
            artist: non_empty(&view.artist),
            app,
            playing: view.playing,
        }
    }

    /// The line under the title: the artist and the app.
    fn subtitle(&self) -> String {
        match &self.artist {
            Some(artist) => format!("{artist} · {}", self.app),
            None => self.app.clone(),
        }
    }
}

/// The session the buttons act on.
fn selected(controls: &MediaControlsState) -> Option<&MediaSessionView> {
    let id = controls.selected_id?;
    controls.sessions.iter().find(|session| session.id == id)
}

/// The controls show: they are on, the notifications have synced, and the selected session exists.
fn shown(controls: &MediaControlsState) -> bool {
    controls.shown && selected(controls).is_some()
}

/// The setting applies: the phone supports media controls and grants notification access.
fn setting_applies(controls: &MediaControlsState) -> bool {
    controls.supported && controls.notifications_permission
}

/// The line under the setting's switch.
fn setting_detail(controls: Option<&MediaControlsState>) -> &'static str {
    match controls {
        None => "Control what plays on your phone from this computer.",
        Some(c) if !c.supported => "This phone doesn't support media controls.",
        Some(c) if !c.notifications_permission => {
            "This needs notification access on your phone. Request it in Settings › Notifications."
        }
        Some(_) => "Play, pause and skip what plays on your phone from the system tray and the Phone Link window.",
    }
}

/// The controls, `None` before the engine runs or without a phone; another failure is logged.
fn controls(engine: &Arc<Engine>) -> Option<MediaControlsState> {
    match engine.clone().audio().controls() {
        Ok(controls) => Some(controls),
        Err(FeatureApiError::NotStarted | FeatureApiError::NoPhone) => None,
        Err(e) => {
            warn!(error = %e, "the media controls were not read");
            None
        }
    }
}

/// The media app's icon as a glyph to tint: the phone sends its notification icon — a shape Android
/// draws through its alpha alone — as an image without alpha, light on black. Each pixel's brightest
/// channel becomes the alpha of a white pixel. `None` for bytes that are no image.
fn glyph_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image =
        image::load_from_memory(bytes).inspect_err(|e| warn!(error = %e, "a media app's icon was not decoded")).ok()?;
    let mut glyph = image.to_rgba8();
    for pixel in glyph.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let alpha = u16::from(r.max(g).max(b)) * u16::from(a) / 255;
        pixel.0 = [255, 255, 255, u8::try_from(alpha).unwrap_or(u8::MAX)];
    }
    let mut png = std::io::Cursor::new(Vec::new());
    glyph
        .write_to(&mut png, image::ImageFormat::Png)
        .inspect_err(|e| warn!(error = %e, "a media app's glyph was not encoded"))
        .ok()?;
    Some(png.into_inner())
}

fn app_glyph(bytes: &[u8]) -> Option<String> {
    images::cached("media-app-glyphs", &format!("{:016x}", images::fnv1a(bytes)), || glyph_png(bytes))
}

fn album_art(view: &MediaSessionView) -> Option<String> {
    view.album_art.as_deref().and_then(|bytes| images::by_content("album-art", bytes))
}

fn media(engine: &Arc<Engine>, state: &State) -> Value {
    let controls = controls(engine);
    let view = controls.as_ref().and_then(selected);
    let session = view.map(Session::of);
    let sessions: Vec<Value> = controls
        .iter()
        .flat_map(|c| c.sessions.iter())
        .map(|s| json!({ "id": s.id.to_string(), "app": Session::of(s).app }))
        .collect();
    json!({
        "shown": controls.as_ref().is_some_and(shown),
        "title": session.as_ref().map(|s| s.title.clone()),
        "artist": session.as_ref().and_then(|s| s.artist.clone()),
        "app": session.as_ref().map(|s| s.app.clone()),
        "subtitle": session.as_ref().map(Session::subtitle),
        "art": view.and_then(album_art),
        "appIcon": view.and_then(|v| v.app_icon.as_deref()).and_then(app_glyph),
        "playing": session.as_ref().is_some_and(|s| s.playing),
        "commanding": controls.as_ref().is_some_and(|c| c.commanding),
        "selected": session.as_ref().map(|s| s.id.to_string()),
        "offersPicker": sessions.len() > 1,
        "sessions": sessions,
        "settingApplies": controls.as_ref().is_some_and(setting_applies),
        "showsControls": controls.as_ref().is_some_and(|c| setting_applies(c) && c.setting),
        "settingDetail": translate(setting_detail(controls.as_ref())),
        "systemControls": state.system_controls,
    })
}

/// What the desktop's media controls show: the selected session while the controls show.
fn now_playing(engine: &Arc<Engine>) -> Option<NowPlaying> {
    let controls = controls(engine)?;
    if !shown(&controls) {
        return None;
    }
    let view = selected(&controls)?;
    let session = Session::of(view);
    Some(NowPlaying {
        session: session.id,
        title: session.title,
        artist: session.artist,
        app: session.app,
        art_url: album_art(view),
        playing: session.playing,
    })
}

/// Logs a media command that failed; one sent before the engine runs is dropped.
fn log_command(subject: &str, result: Result<(), FeatureApiError>) {
    match result {
        Ok(()) => {}
        Err(FeatureApiError::NotStarted) => {
            debug!(command = subject, "a media command was dropped: the engine has not started")
        }
        Err(e) => warn!(command = subject, error = %e, "a media command was not sent"),
    }
}

impl EngineBridge {
    pub(super) fn query_playback(&self, engine: &Arc<Engine>, name: &str, _arg: &str) -> Option<Value> {
        match name {
            "media" => Some(media(engine, &self.areas.playback)),
            _ => None,
        }
    }

    pub(super) fn perform_playback(mut self: Pin<&mut Self>, engine: Arc<Engine>, name: &str, arg: &str) -> bool {
        let flag = arg == "true";
        // Whether the views re-read: after every action of the user's, but not for `FilesHost`'s report
        // of an unchanged choice, which it sends whenever the media changes.
        let changed = match name {
            "media.playPause" => {
                log_command("media play/pause", engine.audio().play_pause());
                true
            }
            "media.next" => {
                log_command("media next track", engine.audio().next());
                true
            }
            "media.previous" => {
                log_command("media previous track", engine.audio().previous());
                true
            }
            "media.select" => {
                match arg.parse::<i64>() {
                    Ok(id) => log_command("media session choice", engine.audio().select_session(id)),
                    Err(e) => warn!(%arg, error = %e, "media.select takes a session id"),
                }
                true
            }
            "media.setting" => {
                match engine.audio().set_setting(flag) {
                    Ok(()) | Err(FeatureApiError::NotStarted) => {}
                    Err(e) => self.as_mut().show_failure(Failure::setting(&e)),
                }
                true
            }
            "media.systemControls" => {
                let changed = self.areas.playback.system_controls != flag;
                if changed {
                    self.as_mut().rust_mut().areas.playback.system_controls = flag;
                    self.as_mut().emit_area_event("media", "systemControls", &json!({ "on": flag }));
                }
                changed
            }
            _ => return false,
        };

        self.as_mut().sync_player();
        if changed {
            self.bump(Revision::Media);
        }
        true
    }

    /// The account or the phone went away: the desktop's media controls stop showing its media.
    pub(super) fn drop_media_player(self: Pin<&mut Self>) {
        self.rust_mut().areas.playback.player = None;
    }

    /// Shows the selected session in the desktop's media controls while the user lets them and the
    /// controls show, and removes the player otherwise.
    ///
    /// The player follows the bridge's engine: it is read anew every time, and without one the player
    /// goes.
    fn sync_player(mut self: Pin<&mut Self>) {
        let wanted =
            if self.areas.playback.system_controls { self.engine.clone().and_then(|e| now_playing(&e)) } else { None };
        let Some(item) = wanted else {
            self.as_mut().rust_mut().areas.playback.player = None;
            return;
        };

        if self.areas.playback.player.is_none() {
            let thread = self.qt_thread();
            let player = Player::start(self.runtime.handle().clone(), move |command| {
                let _ = thread.queue(move |bridge| bridge.media_command(command));
            });
            self.as_mut().rust_mut().areas.playback.player = Some(player);
        }
        if let Some(player) = &self.areas.playback.player {
            player.show(item);
        }
    }

    /// A request of the desktop's media controls.
    ///
    /// The engine has one play/pause button, so play and pause press it only when the session is not
    /// already doing what they ask, and stop pauses.
    fn media_command(mut self: Pin<&mut Self>, command: Command) {
        let Some(engine) = self.engine.clone() else {
            self.as_mut().rust_mut().areas.playback.player = None;
            return;
        };

        let audio = engine.clone().audio();
        match command {
            Command::Raise => {
                self.show_section(QString::default());
                return;
            }
            Command::PlayPause => log_command("media play/pause", audio.play_pause()),
            Command::Next => log_command("media next track", audio.next()),
            Command::Previous => log_command("media previous track", audio.previous()),
            Command::Play | Command::Pause | Command::Stop => {
                let playing = controls(&engine).and_then(|c| selected(&c).map(|s| s.playing));
                let toggles = match command {
                    Command::Play => playing == Some(false),
                    _ => playing == Some(true),
                };
                if toggles {
                    log_command("media play/pause", audio.play_pause());
                }
            }
        }

        self.as_mut().sync_player();
        self.bump(Revision::Media);
    }

    pub(super) fn audio_changed(mut self: Pin<&mut Self>, _change: AudioChange) {
        self.as_mut().sync_player();
        self.bump(Revision::Media);
    }
}

#[cfg(test)]
mod tests;
