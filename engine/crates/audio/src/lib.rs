//! Media controls for what plays on the phone: the phone's media sessions, shown from its stored
//! media-style notifications, and the play, pause, next and previous commands sent back.
//!
//! | Module | Implements |
//! |---|---|
//! | [`sessions`] | the sessions made from the stored media-style notifications, and the selection |
//! | [`wire`] | the `/command/audio` request |
//! | [`store`] | the per-phone audio-controls setting |
//!
//! There is no audio message from the phone: a media app's notification arrives as an ordinary
//! `notifications` item with `notificationClass` `2` (the notifications feature stores it and keeps
//! it out of its list), and the phone publishes such items only after the `connect` message said
//! `AudioInfoSyncEnabled` — which the notifications feature sends from the phone's `AudioControl`
//! capability, whatever this feature's setting says.
//!
//! The controls are **active** while the phone is Android with `AudioControl`, the audio-controls
//! setting is on, the phone grants notification access and its features are not all disabled; they
//! are **shown** while active, once the notifications have synced, and while a session exists. A
//! command goes to the selected session only; its outcome is logged and changes nothing but the
//! busy indicator, which lasts until 20 s after the request finished or until a session change
//! leaves the selected session in a state other than *none*.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod sessions;
pub mod store;
pub mod wire;

use phonelink_core::config::features::AudioConfig;
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use phonelink_app::consts::capability as phone_capability;
use phonelink_app::{AppEvent, AppLayer, PhoneRecord};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::StoreError;
use phonelink_features::shared::ongoing::OngoingFailure;
use phonelink_features::{FeatureEnv, FeatureError};
use phonelink_notifications::item::NotificationItem;
use phonelink_notifications::{Notifications, NotificationsError, NotificationsEvent};

#[cfg(feature = "test-support")]
pub use sessions::STATE_PLAYING;
pub use sessions::{MediaSession, STATE_NONE};
pub use wire::AudioCommand;

use phonelink_core::sync::MutexExt;
use sessions::{MediaRow, Sessions};
use store::AudioStore;

/// The feature's name: its store.
pub const FEATURE: &str = "audio";

/// Why a UI operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("the App layer is gone")]
    AppGone,
    #[error("the media session is not shown")]
    UnknownSession,
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("notifications: {0}")]
    Notifications(#[from] NotificationsError),
}

/// What decides whether the controls are active, for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Gates {
    /// The phone is Android and reports `AudioControl`.
    supported: bool,
    /// The audio-controls setting (default on).
    setting: bool,
    /// The phone's notification access.
    notifications_permission: bool,
    all_features_disabled: bool,
    /// The notifications have synced once.
    synced: bool,
}

impl Gates {
    fn of(record: &PhoneRecord, setting: bool, synced: bool) -> Self {
        Self {
            supported: phonelink_notifications::is_android(record)
                && record.has_capability(phone_capability::AUDIO_CONTROL),
            setting,
            notifications_permission: phonelink_notifications::device_permission(record),
            all_features_disabled: record.all_features_disabled,
            synced,
        }
    }

    fn active(&self) -> bool {
        self.supported && self.setting && self.notifications_permission && !self.all_features_disabled
    }
}

/// The media controls of the selected phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaControls {
    pub phone: DcgClient,
    /// The phone is Android and reports `AudioControl`: the setting is offered.
    pub supported: bool,
    /// The audio-controls setting (default on).
    pub setting: bool,
    /// The phone grants notification access; without it the controls are off whatever the setting.
    pub notifications_permission: bool,
    /// Supported, setting on, notification access, features not all disabled.
    pub active: bool,
    /// Active, the notifications have synced once, and a session exists.
    pub shown: bool,
    /// The sessions in arrival order (empty while not active).
    pub sessions: Vec<MediaSession>,
    /// The row of the session the controls act on.
    pub selected: Option<i64>,
    /// A command was sent and the controls show they are busy.
    pub commanding: bool,
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    /// What [`Audio::controls`] reports for `phone` changed.
    Changed { phone: DcgClient },
}

/// The selected phone's sessions and busy state.
#[derive(Default)]
struct State {
    /// The phone the rest belongs to.
    phone: Option<DcgClient>,
    gates: Option<Gates>,
    sessions: Sessions,
    /// The busy indicator's generation while it shows.
    busy: Option<u64>,
    next_busy: u64,
    /// What the UI was last told.
    published: Option<MediaControls>,
}

impl State {
    fn snapshot(&self) -> Option<MediaControls> {
        let (phone, gates) = (self.phone.clone()?, self.gates?);
        let active = gates.active();
        Some(MediaControls {
            phone,
            supported: gates.supported,
            setting: gates.setting,
            notifications_permission: gates.notifications_permission,
            active,
            shown: active && gates.synced && !self.sessions.is_empty(),
            sessions: self.sessions.list(),
            selected: self.sessions.selected().map(|s| s.row_id),
            commanding: self.busy.is_some(),
        })
    }
}

struct Inner {
    config: AudioConfig,
    app: Weak<AppLayer>,
    notifications: Arc<Notifications>,
    store: AudioStore,
    runtime: Handle,
    state: Mutex<State>,
    events: broadcast::Sender<AudioEvent>,
    shutdown: CancellationToken,
    /// The refreshes cannot read the stores.
    unreadable: OngoingFailure,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl Inner {
    /// Tells the UI when what it shows changed.
    fn publish(&self, st: &mut State) {
        let snapshot = st.snapshot();
        if snapshot != st.published {
            st.published = snapshot.clone();
            if let Some(s) = snapshot {
                // No subscriber is not an error.
                let _ = self.events.send(AudioEvent::Changed { phone: s.phone });
            }
        }
    }

    /// Re-reads the gates and, while active, brings the sessions in line with the stored
    /// notifications; while not active the sessions are dropped. A store that cannot be read leaves
    /// the controls as they were; it is logged once until a refresh succeeds again, because every
    /// change of the phone record or the notifications refreshes.
    fn refresh(&self) {
        match self.try_refresh() {
            Ok(()) => self.unreadable.clear(),
            Err(e) => {
                if self.unreadable.starts() {
                    error!(error = %e, "media controls not refreshed; they show their last state");
                }
            }
        }
    }

    fn try_refresh(&self) -> Result<(), AudioError> {
        let Some(app) = self.app.upgrade() else { return Ok(()) };
        let Some(phone) = app.selected_phone() else { return Ok(()) };
        let Some(record) = app.phone_record().filter(|r| r.phone().is_same_client(&phone)) else { return Ok(()) };
        let gates = self.read_gates(&phone, &record)?;
        let rows = if gates.active() { Some(self.notifications.stored_rows(&phone)?) } else { None };

        let mut st = self.state.lock_or_panic();
        if !st.phone.as_ref().is_some_and(|p| p.is_same_client(&phone)) {
            st.sessions.clear();
            st.busy = None;
            st.phone = Some(phone);
        }

        st.gates = Some(gates);
        match rows {
            Some(rows) => {
                let rows = rows
                    .into_iter()
                    .filter_map(|r| {
                        // An unparsable row is reported by the notification list; it makes no session.
                        let item = NotificationItem::parse(&r.json).ok()?;
                        Some(MediaRow { row_id: r.id, json: r.json, item })
                    })
                    .collect();
                if st.sessions.apply(rows)
                    && st.busy.is_some()
                    && st.sessions.selected().is_some_and(|s| s.playback_state != STATE_NONE)
                {
                    st.busy = None;
                }
            }
            None => {
                st.sessions.clear();
                st.busy = None;
            }
        }

        self.publish(&mut st);
        Ok(())
    }

    fn read_gates(&self, phone: &DcgClient, record: &PhoneRecord) -> Result<Gates, AudioError> {
        let setting = self.store.setting(phone)?;
        let synced = self.notifications.synced_once(phone)?;
        Ok(Gates::of(record, setting, synced))
    }

    /// Sends `command` to `session` and shows the controls busy; the outcome is logged.
    fn send(self: &Arc<Self>, st: &mut State, command: AudioCommand, session: &MediaSession, trigger: &'static str) {
        let app = match self.app.upgrade() {
            Some(app) => app,
            None => return,
        };

        let generation = st.next_busy;
        st.next_busy += 1;
        st.busy = Some(generation);
        self.publish(st);

        let request = wire::command_request(command, &session.key, trigger);
        let weak = Arc::downgrade(self);
        let busy = self.config.command_busy;
        let shutdown = self.shutdown.clone();
        self.runtime.spawn(async move {
            match app.request(request).await {
                Ok(response) => info!(?command, outcome = ?response.outcome, "media command finished"),
                Err(e) => warn!(?command, error = %e, "media command not sent"),
            }
            drop(app);

            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(busy) => {}
            }

            let Some(inner) = weak.upgrade() else { return };
            let mut st = inner.state.lock_or_panic();
            if st.busy == Some(generation) {
                st.busy = None;
                inner.publish(&mut st);
            }
        });
    }

    /// Re-reads on every change of the phone record, its permissions or its features flag, and on
    /// every change of the stored notifications or their status; lost events re-read too.
    fn spawn_watcher(self: &Arc<Self>, app: &AppLayer) {
        let mut app_events = app.subscribe_events();
        let mut notification_events = self.notifications.subscribe();
        let weak = Arc::downgrade(self);
        let shutdown = self.shutdown.clone();
        self.runtime.spawn(async move {
            loop {
                let refresh = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    event = app_events.recv() => match event {
                        Ok(AppEvent::PermissionsChanged { .. }
                            | AppEvent::AllFeaturesDisabled { .. }
                            | AppEvent::PhoneRecordChanged { .. }) => true,
                        Ok(_) => false,
                        Err(RecvError::Lagged(n)) => {
                            warn!(skipped = n, "media controls missed App events; re-reading");
                            true
                        }
                        Err(RecvError::Closed) => break,
                    },
                    event = notification_events.recv() => match event {
                        Ok(NotificationsEvent::ListChanged { .. } | NotificationsEvent::StatusChanged { .. }) => true,
                        Ok(_) => false,
                        Err(RecvError::Lagged(n)) => {
                            warn!(skipped = n, "media controls missed notification events; re-reading");
                            true
                        }
                        Err(RecvError::Closed) => break,
                    },
                };

                if refresh {
                    let Some(inner) = weak.upgrade() else { break };
                    inner.refresh();
                }
            }
        });
    }
}

/// The installed feature: what the UI reads and does.
pub struct Audio {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Audio").finish_non_exhaustive()
    }
}

impl Audio {
    /// Installs the feature on `env`'s App layer, reading the sessions from `notifications`' store.
    /// Call it once, inside the tokio runtime, after notifications.
    pub fn install(
        env: &FeatureEnv,
        config: AudioConfig,
        notifications: Arc<Notifications>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("audio must be installed inside a tokio runtime".into()))?;
        if config.command_busy.is_zero() {
            return Err(FeatureError::Config("the media command busy time must be positive".into()));
        }

        let inner = Arc::new(Inner {
            config,
            app: Arc::downgrade(&env.app),
            notifications,
            store: AudioStore::open(&env.stores),
            runtime,
            state: Mutex::new(State::default()),
            events: broadcast::channel(64).0,
            shutdown: CancellationToken::new(),
            unreadable: OngoingFailure::default(),
        });

        inner.spawn_watcher(&env.app);
        inner.refresh();
        Ok(Arc::new(Self { inner }))
    }

    /// Events telling the UI to re-read [`Audio::controls`].
    pub fn subscribe(&self) -> broadcast::Receiver<AudioEvent> {
        self.inner.events.subscribe()
    }

    /// The media controls of the selected phone.
    pub fn controls(&self) -> Result<MediaControls, AudioError> {
        let app = self.inner.app.upgrade().ok_or(AudioError::AppGone)?;
        let phone = app.selected_phone().ok_or(AudioError::NoPhone)?;
        let current = |inner: &Inner| {
            let st = inner.state.lock_or_panic();
            st.snapshot().filter(|s| s.phone.is_same_client(&phone))
        };

        if let Some(s) = current(&self.inner) {
            return Ok(s);
        }

        self.inner.refresh();
        current(&self.inner).ok_or(AudioError::NoPhone)
    }

    /// The play/pause button: *Pause* when the selected session plays, else *Play*. Nothing is sent
    /// without a selected session.
    pub fn play_pause(&self) {
        self.press(|s| if s.is_playing() { AudioCommand::Pause } else { AudioCommand::Play });
    }

    /// The next button: *Next* to the selected session.
    pub fn next(&self) {
        self.press(|_| AudioCommand::Next);
    }

    /// The previous button: *Previous* to the selected session.
    pub fn previous(&self) {
        self.press(|_| AudioCommand::Previous);
    }

    fn press(&self, command: impl FnOnce(&MediaSession) -> AudioCommand) {
        let mut st = self.inner.state.lock_or_panic();
        if !st.gates.is_some_and(|g| g.active()) {
            return;
        }

        let Some(session) = st.sessions.selected().cloned() else { return };
        let command = command(&session);
        self.inner.send(&mut st, command, &session, command.button_trigger());
    }

    /// The user picked the session of `row_id`. When the previous selection was playing and the
    /// picked one is not, *Play* is sent to the picked one.
    pub fn select(&self, row_id: i64) -> Result<(), AudioError> {
        let mut st = self.inner.state.lock_or_panic();
        if st.sessions.find(row_id).is_none() {
            return Err(AudioError::UnknownSession);
        }

        let play = st.sessions.pick(row_id);
        self.inner.publish(&mut st);

        if let Some(session) = play {
            self.inner.send(&mut st, AudioCommand::Play, &session, wire::TRIGGER_SOURCE_SWITCH);
        }
        Ok(())
    }

    /// Sets the selected phone's audio-controls setting; the controls follow it.
    pub fn set_setting(&self, on: bool) -> Result<(), AudioError> {
        let app = self.inner.app.upgrade().ok_or(AudioError::AppGone)?;
        let phone = app.selected_phone().ok_or(AudioError::NoPhone)?;
        if self.inner.store.set_setting(&phone, on)? {
            self.inner.refresh();
        }
        Ok(())
    }
}
