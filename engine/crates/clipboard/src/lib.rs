//! Cross-device clipboard.
//!
//! A Cross Device Experience Host feature: platform-typed messages only, `/Context/Publish` tag 9
//! announcements in both directions and `/DeviceResourceManager` `GET /clipboard` content pulls,
//! status queries and feature-state messages. No App message and no connect contribution.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the `clipboard.v1` schema (write rules), the announcement message, the content answer (item encoding), the worked example |
//! | [`store`] | the inputs (the global switch, `clipboardEnabled`, `clipboardFeatureStatusUpToDate`), the feature option / the `CLIPBOARD` capability |
//! | [`pasteboard`] | the pasteboard edge, type mapping and markers (differs from Phone Link on Windows), the kind priority of announcing, the read limit |
//! | [`image`] | image re-encoding, the image cases of announcing and response handling (the codec edge) |
//! | [`html`] | the plain text derived from a received `TEXT_HTML` |
//! | [`sync`] | feature-state sync, startup and shutdown (the managers) |
//! | [`inbound`] | phone → PC: the change announcement (inbound tag 9), the content pull, response handling |
//! | [`outbound`] | PC → phone: the change announcement (outbound tag 9): change detection, debounce, echo suppression, announcing |
//! | [`receiver`] | PC → phone: serving `/clipboard` (answer rows 1–10, the content answer) |
//!
//! Gating: the global switch is a persisted app setting (default on); the per-phone "enabled" flag
//! is [`PhoneSwitches`] (shared with device status); the phone's `CLIPBOARD` capability and
//! `clientVersion` come from the device list ([`PeerDirectory`]; a missing list is the empty one);
//! "available" is the phone's platform session ([`PartnerSessionControl::is_connected`]). An
//! Android phone is a device-list entry whose `osName` is `Android` (case-insensitive).
//!
//! Failures are isolated: inbound data never panics, a request that fails the strict proto3 decode
//! is answered `UNRECOGNIZED_PAYLOAD`, a request without `_requestId` is logged and dropped, and
//! any other failure abandons only the clipboard operation in hand. Nothing is retried. After
//! [`Clipboard::shutdown`] nothing is sent, answered or written.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod html;
pub mod image;
pub mod inbound;
pub mod outbound;
pub mod pasteboard;
pub mod receiver;
pub mod store;
pub mod sync;
pub mod wire;

use phonelink_core::config::features::ClipboardConfig;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use phonelink_core::edge::Clock;
use phonelink_core::seams::PartnerSessionControl;
use phonelink_core::types::{DcgClient, DeviceInfoItem};
use phonelink_features::{FeatureEnv, FeatureError, PeerDirectory, PhoneSwitches, StoreError};
use phonelink_transport::auth::PeerDevice;
use phonelink_transport::platform::{DeviceFilter, Drm, DrmSendOutcome, DrmSendRequest, Msaep, Subscription};

pub use image::ClipboardImageCodec;
#[cfg(feature = "test-support")]
pub use image::ClipboardImageSize;
pub use pasteboard::{ClipKind, ClipboardPasteboard};
#[cfg(feature = "test-support")]
pub use pasteboard::{ClipboardPasteboardData, ClipboardPasteboardWrite};
#[cfg(feature = "test-support")]
pub use store::GLOBAL_SWITCH_DEFAULT;
pub use store::{CAPABILITY, global_switch_persisted, with_clipboard_capability};

use outbound::WatchState;
use phonelink_core::sync::MutexExt;
use store::ClipboardStore;
use sync::Command;

fn validate_config(config: &ClipboardConfig) -> Result<(), FeatureError> {
    if config.poll_interval.is_zero() {
        return Err(FeatureError::Config("clipboard poll_interval must be positive".into()));
    }
    if config.image_byte_limit == 0 {
        return Err(FeatureError::Config("clipboard image_byte_limit must be positive".into()));
    }
    if config.request_timeout.is_zero() {
        return Err(FeatureError::Config("clipboard request_timeout must be positive".into()));
    }
    Ok(())
}

/// What changed, so the UI (and the engine) re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardEvent {
    /// The global switch changed. The engine advertises `CLIPBOARD` iff `enabled`
    /// ([`with_clipboard_capability`] → `AuthService::set_capabilities`, then the client-info refresh).
    GlobalSwitchChanged { enabled: bool },
    /// Something [`Clipboard::phone`] reports for the phone changed (its toggle, capability,
    /// availability, enabled flag, up-to-date flag, or the phone joined or left the account).
    PhoneChanged { phone: DcgClient },
    /// The phone's clip was written to the pasteboard.
    Received { phone: DcgClient, kind: ClipKind },
    /// The PC announced its clipboard to the connected phones.
    Announced { kind: ClipKind },
}

/// What the settings UI shows for one Android phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardPhone {
    pub phone: DcgClient,
    /// The device-list `name`.
    pub name: Option<String>,
    /// The per-phone toggle (`clipboardEnabled`).
    pub toggle: bool,
    /// The toggle applies: the phone advertises `CLIPBOARD` and the global switch is on (Windows
    /// offers the per-phone setting only then).
    pub applicable: bool,
    /// The phone advertises `CLIPBOARD`.
    pub capable: bool,
    /// The per-phone "enabled" flag.
    pub enabled: bool,
    /// The phone's platform session is connected.
    pub available: bool,
    /// enabled ∧ available ∧ capable ∧ toggle.
    pub supported: bool,
    pub feature_status_up_to_date: bool,
}

/// The phone's device-list `capabilities`; a missing list is the empty list, so the two never
/// differ.
pub(crate) fn listed_capabilities(info: &DeviceInfoItem) -> &[String] {
    info.capabilities.as_deref().unwrap_or_default()
}

/// The phone's capability list contains exactly `CLIPBOARD`.
pub(crate) fn has_capability(capabilities: &[String]) -> bool {
    capabilities.iter().any(|c| c == CAPABILITY)
}

/// The installed clipboard feature.
pub struct Clipboard {
    me: Weak<Clipboard>,
    config: ClipboardConfig,
    peers: Arc<dyn PeerDirectory>,
    sessions: Arc<dyn PartnerSessionControl>,
    switches: Arc<PhoneSwitches>,
    clock: Arc<dyn Clock>,
    msaep: Arc<Msaep>,
    drm: Arc<Drm>,
    pasteboard: Arc<dyn ClipboardPasteboard>,
    codec: Arc<dyn ClipboardImageCodec>,
    store: ClipboardStore,
    events: broadcast::Sender<ClipboardEvent>,
    commands: mpsc::UnboundedSender<Command>,
    /// The tag-9 subscription; dropping it unsubscribes.
    subscription: Mutex<Option<Subscription>>,
    /// The change count last seen and the app's own last write (for echo suppression), under one
    /// lock with every write.
    watch: Mutex<WatchState>,
    /// The last announced content hash (memory only; only PC announcements update it).
    last_hash: Mutex<Option<String>>,
    /// Serializes the read, hash and dedupe of announcements.
    announcing: tokio::sync::Mutex<()>,
    /// Set once [`Clipboard::install`] completed. Every PC request waits for it, so those the parts
    /// start before the `/clipboard` receiver is registered — the managers' startup feature states,
    /// the pull of an announcement delivered meanwhile — are never sent by an install that fails.
    installed: watch::Sender<bool>,
    shutdown: CancellationToken,
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl Clipboard {
    /// Installs the feature in the startup order: one feature-state manager per
    /// Android phone (each sends its feature state), the tag-9 subscription, the pasteboard watcher,
    /// the `/clipboard` receiver. Call once, inside the tokio runtime, after `PlatformRoutes::install`
    /// with the account enrolled; `msaep` and `drm` are its handles.
    ///
    /// Registering the receiver, the last step, is the only one that can fail. The install stays
    /// atomic: every PC request waits until the install completed, so a failure stops what the
    /// earlier steps started (as [`Clipboard::shutdown`]) before anything is sent; an announcement
    /// of the PC needs a pasteboard change and the debounce, so none can go out meanwhile either.
    pub fn install(
        env: &FeatureEnv,
        config: ClipboardConfig,
        msaep: Arc<Msaep>,
        drm: Arc<Drm>,
        pasteboard: Arc<dyn ClipboardPasteboard>,
        codec: Arc<dyn ClipboardImageCodec>,
    ) -> Result<Arc<Clipboard>, FeatureError> {
        validate_config(&config)?;
        if tokio::runtime::Handle::try_current().is_err() {
            return Err(FeatureError::Programming("the clipboard must be installed inside a tokio runtime".into()));
        }
        if !env.peers.is_enrolled() {
            return Err(FeatureError::Programming("the clipboard starts once the account is enrolled".into()));
        }

        let store = ClipboardStore::open(&env.stores)?;
        let (commands, command_rx) = mpsc::unbounded_channel();

        // Subscribed before any state is read, so no change is missed.
        let triggers = sync::Triggers {
            commands: command_rx,
            peers: env.peers.subscribe(),
            sessions: env.sessions.subscribe(),
            switches: env.switches.subscribe(),
        };
        let clipboard = Arc::new_cyclic(|me| Clipboard {
            me: me.clone(),
            config,
            peers: env.peers.clone(),
            sessions: env.sessions.clone(),
            switches: env.switches.clone(),
            clock: env.edges.clock.clone(),
            msaep,
            drm,
            pasteboard,
            codec,
            store,
            events: broadcast::channel(64).0,
            commands,
            subscription: Mutex::new(None),
            watch: Mutex::new(WatchState::default()),
            last_hash: Mutex::new(None),
            announcing: tokio::sync::Mutex::new(()),
            installed: watch::Sender::new(false),
            shutdown: CancellationToken::new(),
        });

        let managers = clipboard.start_managers();
        clipboard.subscribe_announcements();
        outbound::spawn_watcher(&clipboard);

        if let Err(e) = receiver::register(&clipboard) {
            clipboard.shutdown();
            return Err(e);
        }

        clipboard.installed.send_replace(true);
        sync::spawn_loop(&clipboard, managers, triggers);
        info!("feature installed");
        Ok(clipboard)
    }

    /// Stops the four parts and sends nothing (account removal, shutdown): no more feature states,
    /// no tag-9 subscription, no pasteboard watching, and the `/clipboard` receiver answers nothing.
    /// Requests in flight are abandoned — a content pull or feature-state request still waiting for
    /// its session start, its send or its answer is cancelled — and nothing is sent, answered or
    /// written once this returns, whatever triggers are still queued.
    ///
    /// Idempotent and non-blocking (no `await`, no I/O): call it from any thread or tokio task.
    pub fn shutdown(&self) {
        let first = !self.shutdown.is_cancelled();
        self.shutdown.cancel();
        let subscription = self.subscription.lock_or_panic().take();
        drop(subscription);

        if first {
            info!("feature stopped");
        }
    }

    /// Changes in the clipboard's settings, phones and transfers.
    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEvent> {
        self.events.subscribe()
    }

    /// The global switch.
    pub fn global_switch(&self) -> Result<bool, FeatureError> {
        Ok(self.store.global_switch()?)
    }

    /// Sets the global switch. A change is persisted, reported (the engine re-advertises
    /// `CLIPBOARD`), and re-runs the startup sequence: every phone's feature state again.
    pub fn set_global_switch(&self, enabled: bool) -> Result<(), FeatureError> {
        if self.store.set_global_switch(enabled)? {
            info!(enabled, "global switch changed");
            self.emit(ClipboardEvent::GlobalSwitchChanged { enabled });
            self.command(Command::GlobalSwitchChanged);
        }
        Ok(())
    }

    /// `metadata.capabilities` for the current global switch: `base` (the engine's
    /// `config.auth.capabilities`) with `CLIPBOARD` iff the switch is on.
    pub fn advertised_capabilities(&self, base: &[String]) -> Result<Vec<String>, FeatureError> {
        Ok(with_clipboard_capability(base, self.global_switch()?))
    }

    /// The per-phone toggle (`clipboardEnabled`, default off).
    pub fn phone_toggle(&self, phone: &DcgClient) -> Result<bool, FeatureError> {
        Ok(self.store.toggle(phone)?)
    }

    /// Sets the per-phone toggle; a change is persisted, reported, and sends the phone's feature
    /// state with a forced session start.
    pub fn set_phone_toggle(&self, phone: &DcgClient, enabled: bool) -> Result<(), FeatureError> {
        if self.store.set_toggle(phone, enabled)? {
            info!(%phone, enabled, "phone toggle changed");
            self.emit(ClipboardEvent::PhoneChanged { phone: phone.clone() });
            self.command(Command::ToggleChanged(phone.clone()));
        }
        Ok(())
    }

    /// The Android phones of the account, as the settings UI shows them.
    pub fn phones(&self) -> Result<Vec<ClipboardPhone>, FeatureError> {
        self.peers.peers().iter().filter(|p| p.info.is_android()).map(|p| self.view(p)).collect()
    }

    /// One phone of the account (`None` when it is not an Android phone of the account).
    pub fn phone(&self, phone: &DcgClient) -> Result<Option<ClipboardPhone>, FeatureError> {
        match self.peers.peer(phone) {
            Some(peer) if peer.info.is_android() => Ok(Some(self.view(&peer)?)),
            _ => Ok(None),
        }
    }

    fn view(&self, peer: &PeerDevice) -> Result<ClipboardPhone, FeatureError> {
        let global = self.store.global_switch()?;
        let toggle = self.store.toggle(&peer.client)?;
        let capable = has_capability(listed_capabilities(&peer.info));
        let enabled = self.switches.is_enabled(&peer.client)?;
        let available = self.sessions.is_connected(&peer.client);

        Ok(ClipboardPhone {
            phone: peer.client.clone(),
            name: peer.info.name.clone(),
            toggle,
            applicable: capable && global,
            capable,
            enabled,
            available,
            supported: enabled && available && capable && toggle,
            feature_status_up_to_date: self.store.up_to_date(&peer.client)?,
        })
    }

    /// *Phone supported* = phone enabled ∧ available ∧ `CLIPBOARD` ∧ toggle on.
    fn phone_supported(&self, peer: &PeerDevice) -> Result<bool, StoreError> {
        Ok(self.switches.is_enabled(&peer.client)?
            && self.sessions.is_connected(&peer.client)
            && has_capability(listed_capabilities(&peer.info))
            && self.store.toggle(&peer.client)?)
    }

    /// *Any phone supported*, from the current inputs.
    fn any_phone_supported(&self) -> Result<bool, StoreError> {
        for peer in self.peers.peers().iter().filter(|p| p.info.is_android()) {
            if self.phone_supported(peer)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The gate of the phone's announcements and pulls: global switch on, phone enabled, `CLIPBOARD`,
    /// toggle on.
    fn phone_gates_open(&self, peer: &PeerDevice) -> Result<bool, StoreError> {
        Ok(self.store.global_switch()?
            && self.switches.is_enabled(&peer.client)?
            && has_capability(listed_capabilities(&peer.info))
            && self.store.toggle(&peer.client)?)
    }

    /// Subscribes tag 9 (no expiry, any publisher), replacing an earlier subscription: the old one is
    /// dropped first, so one observer at most ever sees a publication.
    fn subscribe_announcements(&self) {
        let mut slot = self.subscription.lock_or_panic();
        drop(slot.take());
        if self.is_stopped() {
            return;
        }

        let observer = Arc::new(inbound::AnnouncementObserver(self.me.clone()));
        *slot = Some(self.msaep.subscribe(wire::TAG, DeviceFilter::Any, observer));
    }

    fn command(&self, command: Command) {
        if self.commands.send(command).is_err() && !self.is_stopped() {
            error!("feature-state loop is gone; the change is not sent to the phones");
        }
    }

    fn emit(&self, event: ClipboardEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn is_stopped(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    /// The cancellation of a PC request: none of its own, so only [`Clipboard::shutdown`] (or
    /// dropping the feature) abandons it.
    fn request_cancel(&self) -> CancellationToken {
        self.shutdown.child_token()
    }

    /// Sends a PC request once the install completed; `None` when the request is cancelled first
    /// (the feature stopped, or its install failed).
    async fn send_request(&self, request: DrmSendRequest) -> Option<DrmSendOutcome> {
        let cancel = request.cancel.clone();
        let mut installed = self.installed.subscribe();
        let ready = tokio::select! {
            biased;
            _ = cancel.cancelled() => false,
            done = installed.wait_for(|done| *done) => done.is_ok(),
        };

        if !ready {
            return None;
        }
        Some(self.drm.send(request).await)
    }
}

/// Runs an OS-edge call off the async runtime (the edge traits block); `None` when it panicked.
async fn blocking<T: Send + 'static>(what: &'static str, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(value) => Some(value),
        Err(e) => {
            warn!(what, error = %e, "OS-edge call panicked; the clipboard operation is abandoned");
            None
        }
    }
}
