//! Media controls for the UI ([`crate::features::audio`]): the selected phone's media sessions, the
//! play/pause, next and previous buttons, the session picker and the audio-controls setting.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::audio::{Audio, AudioError, AudioEvent, MediaControls, MediaSession};

use super::{FeatureApiError, core_of, features_of};

impl From<AudioError> for FeatureApiError {
    fn from(e: AudioError) -> Self {
        match e {
            AudioError::NoPhone => FeatureApiError::NoPhone,
            AudioError::AppGone => FeatureApiError::NotStarted,
            AudioError::UnknownSession => FeatureApiError::UnknownItem { what: "media session".into() },
            AudioError::Store(e) => e.into(),
            AudioError::Notifications(e) => e.into(),
        }
    }
}

/// One media session on the phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaSessionView {
    /// Identifies the session in `select_session`.
    pub id: i64,
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// The album art as the phone sent it (normally JPEG); `None` → a placeholder.
    pub album_art: Option<Vec<u8>>,
    /// The media app's icon as the phone sent it (normally JPEG).
    pub app_icon: Option<Vec<u8>>,
    /// The session plays (the play/pause button pauses).
    pub playing: bool,
}

impl From<&MediaSession> for MediaSessionView {
    fn from(s: &MediaSession) -> Self {
        Self {
            id: s.row_id,
            app_name: s.app_name.clone(),
            title: s.title.clone(),
            artist: s.artist.clone(),
            album_art: s.album_art.clone(),
            app_icon: s.app_icon.clone(),
            playing: s.is_playing(),
        }
    }
}

/// The selected phone's media controls.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MediaControlsState {
    /// The phone supports media controls: the setting is offered.
    pub supported: bool,
    /// The audio-controls setting (default on).
    pub setting: bool,
    /// The phone grants notification access; the controls need it.
    pub notifications_permission: bool,
    /// The controls are shown: they are on, the notifications have synced, and a session exists.
    pub shown: bool,
    /// The sessions, in the order they appeared; a picker is offered with two or more.
    pub sessions: Vec<MediaSessionView>,
    /// The session the buttons act on.
    pub selected_id: Option<i64>,
    /// A command was sent and has not shown its effect yet.
    pub commanding: bool,
}

impl From<&MediaControls> for MediaControlsState {
    fn from(c: &MediaControls) -> Self {
        Self {
            supported: c.supported,
            setting: c.setting,
            notifications_permission: c.notifications_permission,
            shown: c.shown,
            sessions: c.sessions.iter().map(Into::into).collect(),
            selected_id: c.selected,
            commanding: c.commanding,
        }
    }
}

/// A change to the media controls: re-read [`AudioApi::controls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum AudioChange {
    /// What `controls` gives changed.
    Changed,
}

impl From<&AudioEvent> for AudioChange {
    fn from(e: &AudioEvent) -> Self {
        match e {
            AudioEvent::Changed { .. } => AudioChange::Changed,
        }
    }
}

/// The media controls of the selected phone.
#[derive(uniffi::Object)]
pub struct AudioApi {
    engine: Arc<Engine>,
}

impl AudioApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(&self, f: impl FnOnce(&Audio) -> Result<T, AudioError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.audio.clone();
        Ok(f(&feature)?)
    }
}

#[uniffi::export]
impl AudioApi {
    /// The selected phone's media sessions, the session the buttons act on, and the setting.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn controls(&self) -> Result<MediaControlsState, FeatureApiError> {
        self.run(|f| f.controls()).map(|c| (&c).into())
    }

    /// The play/pause button of the selected session; nothing without one.
    pub fn play_pause(&self) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.play_pause();
            Ok(())
        })
    }

    /// The next button of the selected session; nothing without one.
    pub fn next(&self) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.next();
            Ok(())
        })
    }

    /// The previous button of the selected session; nothing without one.
    pub fn previous(&self) -> Result<(), FeatureApiError> {
        self.run(|f| {
            f.previous();
            Ok(())
        })
    }

    /// The user picked the session `id`; when the previous one was playing and this one is not, it
    /// is told to play.
    pub fn select_session(&self, id: i64) -> Result<(), FeatureApiError> {
        self.run(|f| f.select(id))
    }

    /// The selected phone's audio-controls setting.
    pub fn set_setting(&self, on: bool) -> Result<(), FeatureApiError> {
        self.run(|f| f.set_setting(on))
    }
}
