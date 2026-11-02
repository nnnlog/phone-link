//! Engine → UI events.
//!
//! It covers [`EngineEvent`], the [`EventSink`] the UI implements, the web sign-in that raises one
//! and waits for the UI's answer, and [`EventBridge`], which turns the core's and the features'
//! events into them.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::warn;

use crate::api::FeatureKind;
use crate::api::pairing::PairingUpdate;
use crate::api::remote_control::RemoteControlChange;
use crate::edge::{WebSignIn, WebSignInError, WebSignInRequest};
use crate::engine::{CoreEvent, CoreEvents};
use crate::features::Features;
use crate::features::remote_control::RemoteControlEvent;
use crate::sync::MutexExt;

/// The in-flight web sign-ins awaiting the UI's answer, keyed by request id.
#[derive(Default)]
pub struct WebSignInPending {
    waiters: Mutex<HashMap<String, tokio::sync::oneshot::Sender<Result<String, WebSignInError>>>>,
}

impl WebSignInPending {
    /// An empty set, with no sign-in waiting.
    pub fn new() -> Self {
        Self::default()
    }

    fn register(&self, id: String, tx: tokio::sync::oneshot::Sender<Result<String, WebSignInError>>) {
        self.waiters.lock_or_panic().insert(id, tx);
    }

    /// The UI's answer for `request_id`: the posted JSON on success, or a declined/failed outcome.
    pub fn resolve(&self, request_id: &str, result: Result<String, WebSignInError>) {
        if let Some(tx) = self.waiters.lock_or_panic().remove(request_id) {
            let _ = tx.send(result);
        }
    }
}

/// The production web sign-in: raises [`EngineEvent::WebSignInRequested`] and awaits the UI's answer
/// through [`WebSignInPending`]. Dropping the waiter (the UI went away) counts as declined.
pub struct EventWebSignIn {
    events: Arc<dyn EventSink>,
    pending: Arc<WebSignInPending>,
}

impl EventWebSignIn {
    /// A web sign-in that raises its requests through `events` and waits for answers in `pending`.
    pub fn new(events: Arc<dyn EventSink>, pending: Arc<WebSignInPending>) -> Self {
        Self { events, pending }
    }
}

#[async_trait::async_trait]
impl WebSignIn for EventWebSignIn {
    async fn present(&self, request: WebSignInRequest) -> Result<String, WebSignInError> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.register(request_id.clone(), tx);
        self.events.on_event(EngineEvent::WebSignInRequested { request_id, request });
        rx.await.unwrap_or(Err(WebSignInError::Declined))
    }
}

/// Engine → UI events. The sink is called on the engine's threads; the UI hops to its own.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum EngineEvent {
    /// The Microsoft-account sign-in needs the user: the UI hosts `request` in a web view and answers with
    /// `Engine::complete_web_sign_in(request_id, …)` or `Engine::cancel_web_sign_in(request_id, …)`.
    WebSignInRequested {
        request_id: String,
        request: WebSignInRequest,
    },
    /// The platform session with the phone connected, or ended (`failure` says why).
    PhoneSession {
        connected: bool,
        failure: Option<String>,
    },
    /// The selected phone left the account's linked devices.
    PhoneUnlinked,
    /// The account was removed while the features ran; the next `Engine::start` builds a new core.
    AccountRemoved,
    /// The phone asked to bring the window forward (`phoneCommand` type 1) with this `ms-phone:` URI.
    BringWindowForward {
        uri: String,
    },
    /// The phone turned every feature off for this PC, or back on.
    AllFeaturesDisabled {
        disabled: bool,
    },
    /// The phone record changed (name, permissions, capabilities): re-read `PhoneApi::info`.
    PhoneChanged,
    /// A connect exchange finished; `immediate` for one the user or app activation started.
    ConnectCompleted {
        immediate: bool,
        outcome: crate::api::RequestOutcome,
    },
    Notifications {
        change: crate::api::notifications::NotificationsChange,
    },
    Clipboard {
        change: crate::api::clipboard::ClipboardChange,
    },
    DeviceStatus {
        change: crate::api::device_status::DeviceStatusChange,
    },
    Messaging {
        change: crate::api::messaging::MessagingChange,
    },
    Contacts {
        change: crate::api::contacts::ContactsChange,
    },
    /// Call history: the selected phone's calls or their state changed.
    Calling {
        change: crate::api::calling::CallingChange,
    },
    Photos {
        change: crate::api::photos::PhotosChange,
    },
    /// Remote control's state for the selected phone may have changed (also after lost events).
    RemoteControl {
        change: RemoteControlChange,
    },
    /// Wallpaper: the phone's wallpaper shown for the selected phone may have changed.
    Wallpaper {
        change: crate::api::wallpaper::WallpaperChange,
    },
    /// New-photo notifications: the selected phone's remote-snipping state may have changed.
    RemoteSnipping {
        change: crate::api::remote_snipping::RemoteSnippingChange,
    },
    /// File transfer: the selected phone's settings or transfers may have changed, or a received
    /// notification asks to open a file or show a folder.
    FileTransfer {
        change: crate::api::file_transfer::FileTransferChange,
    },
    /// Phone files: the selected phone's state changed, its domain appeared or went away, its tree
    /// changed, or deleted files went to its recycle bin.
    PhoneFiles {
        change: crate::api::phone_files::PhoneFilesChange,
    },
    /// The connected camera: the selected phone's camera state may have changed, or its control
    /// window shows or hides.
    ConnectedCamera {
        change: crate::api::connected_camera::ConnectedCameraChange,
    },
    /// Phone apps: the selected phone's apps, recent apps or their state changed.
    PhoneApps {
        change: crate::api::phone_apps::PhoneAppsChange,
    },
    /// Screen mirroring: the phone state, a window's state, or a window to open, close or raise.
    ScreenMirroring {
        change: crate::api::screen_mirroring::ScreenMirroringChange,
    },
    /// Media controls: the selected phone's media sessions or their state changed.
    Audio {
        change: crate::api::audio::AudioChange,
    },
    /// A pairing session's step changed.
    Pairing {
        update: PairingUpdate,
    },
    /// A feature's events were lost (its stream lagged): re-read everything it shows.
    Resync {
        feature: FeatureKind,
    },
}

/// The receiver of [`EngineEvent`]s, implemented by the UI.
#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync {
    /// Receives one event, on the engine thread that raised it; the UI hops to its own thread.
    fn on_event(&self, event: EngineEvent);
}

/// The core's events and the features' events as [`EngineEvent`]s, delivered to the UI's sink on the
/// thread that raised them.
pub struct EventBridge {
    sink: Arc<dyn EventSink>,
}

impl EventBridge {
    /// A bridge that delivers every core and feature event to `sink`.
    pub fn new(sink: Arc<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self { sink })
    }
}

impl CoreEvents for EventBridge {
    fn core_event(&self, event: CoreEvent) {
        self.sink.on_event(match event {
            CoreEvent::PhoneSession { connected, failure } => EngineEvent::PhoneSession { connected, failure },
            CoreEvent::PhoneUnlinked => EngineEvent::PhoneUnlinked,
            CoreEvent::AccountRemoved => EngineEvent::AccountRemoved,
            CoreEvent::BringWindowForward { uri } => EngineEvent::BringWindowForward { uri },
            CoreEvent::AllFeaturesDisabled { disabled } => EngineEvent::AllFeaturesDisabled { disabled },
            CoreEvent::PhoneChanged => EngineEvent::PhoneChanged,
            CoreEvent::ConnectCompleted { immediate, outcome } => {
                EngineEvent::ConnectCompleted { immediate, outcome: outcome.into() }
            }
            CoreEvent::Pairing { session, step } => {
                EngineEvent::Pairing { update: PairingUpdate { session, step: step.into() } }
            }
        });
    }

    /// Forwards every feature's events to the sink as typed [`EngineEvent`]s; a lagged stream
    /// (events lost) becomes [`EngineEvent::Resync`] so the UI re-reads that feature.
    fn features_installed(&self, features: &Features) {
        fn forward<E: Clone + Send + 'static>(
            mut rx: tokio::sync::broadcast::Receiver<E>,
            events: Arc<dyn EventSink>,
            feature: FeatureKind,
            map: impl Fn(&E) -> EngineEvent + Send + 'static,
        ) {
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(e) => events.on_event(map(&e)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            warn!(n, ?feature, "feature events lagged; the UI re-reads the feature");
                            events.on_event(EngineEvent::Resync { feature });
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        let events = &self.sink;
        forward(features.notifications.subscribe(), events.clone(), FeatureKind::Notifications, |e| {
            EngineEvent::Notifications { change: e.into() }
        });
        forward(features.clipboard.subscribe(), events.clone(), FeatureKind::Clipboard, |e| EngineEvent::Clipboard {
            change: e.into(),
        });
        forward(features.device_status.subscribe(), events.clone(), FeatureKind::DeviceStatus, |e| {
            EngineEvent::DeviceStatus { change: e.into() }
        });
        forward(features.messaging.subscribe(), events.clone(), FeatureKind::Messaging, |e| EngineEvent::Messaging {
            change: e.into(),
        });
        forward(features.contacts.subscribe(), events.clone(), FeatureKind::Contacts, |e| EngineEvent::Contacts {
            change: e.into(),
        });
        forward(features.calling.subscribe(), events.clone(), FeatureKind::Calling, |e| EngineEvent::Calling {
            change: e.into(),
        });
        forward(features.photos.subscribe(), events.clone(), FeatureKind::Photos, |e| EngineEvent::Photos {
            change: e.into(),
        });

        // Remote control's only change means "re-read", so lost events become one such change.
        let mut remote_control = features.remote_control.subscribe();
        let sink = events.clone();
        tokio::spawn(async move {
            loop {
                match remote_control.recv().await {
                    Ok(RemoteControlEvent::PhoneChanged { .. }) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "remote-control events lagged; the UI re-reads the feature");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
                sink.on_event(EngineEvent::RemoteControl { change: RemoteControlChange::PhoneChanged });
            }
        });

        forward(features.audio.subscribe(), events.clone(), FeatureKind::Audio, |e| EngineEvent::Audio {
            change: e.into(),
        });
        forward(features.wallpaper.subscribe(), events.clone(), FeatureKind::Wallpaper, |e| EngineEvent::Wallpaper {
            change: e.into(),
        });
        forward(features.remote_snipping.subscribe(), events.clone(), FeatureKind::RemoteSnipping, |e| {
            EngineEvent::RemoteSnipping { change: e.into() }
        });
        forward(features.file_transfer.subscribe(), events.clone(), FeatureKind::FileTransfer, |e| {
            EngineEvent::FileTransfer { change: e.into() }
        });
        forward(features.phone_files.subscribe(), events.clone(), FeatureKind::PhoneFiles, |e| {
            EngineEvent::PhoneFiles { change: e.into() }
        });
        forward(features.connected_camera.subscribe(), events.clone(), FeatureKind::ConnectedCamera, |e| {
            EngineEvent::ConnectedCamera { change: e.into() }
        });
        forward(features.phone_apps.subscribe(), events.clone(), FeatureKind::PhoneApps, |e| EngineEvent::PhoneApps {
            change: e.into(),
        });
        forward(features.screen_mirroring.subscribe(), events.clone(), FeatureKind::ScreenMirroring, |e| {
            EngineEvent::ScreenMirroring { change: e.into() }
        });
    }
}

/// An [`EventSink`] that records events, for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod testing {
    use std::sync::Mutex;

    use super::{EngineEvent, EventSink};
    use crate::sync::MutexExt;

    #[derive(Debug, Default)]
    pub struct CollectingEvents(pub Mutex<Vec<EngineEvent>>);

    impl CollectingEvents {
        pub fn take(&self) -> Vec<EngineEvent> {
            std::mem::take(&mut *self.0.lock_or_panic())
        }
    }

    impl EventSink for CollectingEvents {
        fn on_event(&self, event: EngineEvent) {
            self.0.lock_or_panic().push(event);
        }
    }
}
