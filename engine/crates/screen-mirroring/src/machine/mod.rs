//! The phone state (one per connection) and the window state (one per session): their states,
//! commands, transition tables, and the window state's rules on top of its table.

use tracing::debug;

mod transitions;
pub(crate) use transitions::{phone_transition, session_transition};

/// The phone's state for mirroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneState {
    Disconnected,
    Connecting,
    Connected,
    Error,
    NearbyConnectionNeeded,
    ScreenScrapePermissionNeeded,
    FoldedCannotDraw,
    SecureLocked,
    Locked,
    Unlocked,
    ConnectionUnstable,
    ScreenOff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneCommand {
    EnsureConnection,
    NearbyConnectionCompleted,
    NearbyConnectionNeeded,
    OnConnected,
    OnConnectionClosed,
    OnConnectionError,
    OnConnectionRecovered,
    OnConnectionUnstable,
    OnFoldedCannotDraw,
    OnLocked,
    OnScreenOff,
    OnSecureLocked,
    OnUnlocked,
    RetryConnection,
    ScreenScrapePermissionNeeded,
    StateMachineTimeout,
}

/// A window's state: what it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowState {
    Loading,
    Connected,
    NearbyConnectionNeeded,
    ScreenScrapePermissionNeeded,
    Error,
    DismissibleError,
    Restart,
    FoldedCannotDraw,
    Locked,
    Suspended,
    Closed,
    TrueTouchWarning,
    DeviceOverheat,
    ConnectionUnstable,
    ScreenOff,
    PowerSavingMode,
    AudioStreamingPermissionNeeded,
    AudioStreamingPermissionAlwaysDenied,
    SecureContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionCommand {
    AppLaunchError,
    AppLaunchSuccess,
    AudioStreamingPermissionAlwaysDenied,
    AudioStreamingPermissionFlowComplete,
    AudioStreamingPermissionNeeded,
    DeviceOverheat,
    DismissError,
    MoveToVirtualDisplay,
    NearbyConnectionNeeded,
    PeerConnectionClosed,
    PeerConnectionClosedSuspended,
    PeerConnectionVideoReady,
    PhoneScreenAppLaunchError,
    PhoneStateConnected,
    PhoneStateConnecting,
    PhoneStateConnectionUnstable,
    PhoneStateError,
    PhoneStateFoldedCannotDraw,
    PhoneStateLocked,
    PhoneStateScreenOff,
    PhoneStateUnlocked,
    PowerSavingMode,
    RelaunchAll,
    RelaunchApp,
    ScreenScrapePermissionNeeded,
    SecureContent,
    StateMachineTimeout,
    SwitchScreenLayout,
    TrueTouchWarning,
}

/// What a window state shows besides the state itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Details {
    #[default]
    None,
    /// An error and its code.
    Error { code: i32 },
    /// The phone closed the app (reason text, when any).
    Closed { reason: Option<String> },
    /// The phone is locked or its screen is off.
    Locked { secure: bool, virtual_device_streaming: bool },
    /// The phone asks to move the app to the Mac.
    Restart { task_transfer: bool },
    /// The user asked for another screen layout: the app restarts in it once confirmed.
    LayoutSwitch,
}

/// One window's state, with the rules applied after the table lookup.
#[derive(Debug, Clone)]
pub struct SessionMachine {
    pub state: WindowState,
    pub details: Details,
    launched: bool,
    video_ready: bool,
    suspended: bool,
    closed: bool,
    open_in_main_display: bool,
    locked: bool,
    screen_off: bool,
    folded: bool,
    requires_true_touch: bool,
    /// The audio-permission state left for `ConnectionUnstable`, restored when it recovers.
    saved_audio_state: Option<WindowState>,
    last_locked: Details,
    last_restart: Details,
    last_screen_off: Details,
    /// "Do not show the touch-confirmation screen".
    pub do_not_show_true_touch: bool,
}

impl Default for SessionMachine {
    fn default() -> Self {
        Self::new(false)
    }
}

impl SessionMachine {
    pub fn new(do_not_show_true_touch: bool) -> Self {
        Self {
            state: WindowState::Loading,
            details: Details::None,
            launched: false,
            video_ready: false,
            suspended: false,
            closed: false,
            open_in_main_display: false,
            locked: false,
            screen_off: false,
            folded: false,
            requires_true_touch: false,
            saved_audio_state: None,
            last_locked: Details::None,
            last_restart: Details::None,
            last_screen_off: Details::None,
            do_not_show_true_touch,
        }
    }

    pub fn launched(&self) -> bool {
        self.launched
    }

    pub fn video_ready(&self) -> bool {
        self.video_ready
    }

    /// Applies `command`; `true` when the state or its details changed. A command the table does
    /// not allow from the current state changes nothing.
    pub fn apply(&mut self, command: SessionCommand, details: Option<Details>) -> bool {
        self.apply_inner(command, details, false)
    }

    /// Applies `command` keeping the current details when the state shows none of its own (the
    /// relaunch commands the launch paths give).
    pub fn apply_preserving(&mut self, command: SessionCommand) -> bool {
        self.apply_inner(command, None, true)
    }

    fn apply_inner(&mut self, command: SessionCommand, details: Option<Details>, preserve: bool) -> bool {
        let from = self.state;
        let Some(next) = session_transition(from, command) else {
            debug!(from_state = ?from, ?command, "window state transition not allowed; ignored");
            return false;
        };

        self.remember_details(command, details.as_ref());
        self.update_flags(command, from, next);
        let next = self.resolve_next(from, next);
        let details = self.details_for(from, next, details, preserve);

        let changed = next != self.state || details != self.details;
        self.state = next;
        self.details = details;
        changed
    }

    /// Keeps the last details of each kind, for a state shown later without its own.
    fn remember_details(&mut self, command: SessionCommand, details: Option<&Details>) {
        use SessionCommand as C;
        match details {
            Some(d @ (Details::Restart { .. } | Details::LayoutSwitch)) => self.last_restart = d.clone(),
            Some(d @ Details::Locked { .. }) if command == C::PhoneStateLocked => self.last_locked = d.clone(),
            Some(d @ Details::Locked { .. }) if command == C::PhoneStateScreenOff => self.last_screen_off = d.clone(),
            _ => {}
        }
    }

    /// Updates the flags `command` sets or clears, given the table's `next` state.
    fn update_flags(&mut self, command: SessionCommand, from: WindowState, next: WindowState) {
        use SessionCommand as C;
        use WindowState as S;
        match command {
            C::AppLaunchSuccess => {
                self.launched = true;
                self.suspended = false;
                self.closed = false;
                self.open_in_main_display = false;
            }
            C::AppLaunchError => {
                self.launched = false;
                self.suspended = false;
                self.closed = false;
            }
            C::PeerConnectionVideoReady => self.video_ready = true,
            C::PhoneStateError | C::PeerConnectionClosed | C::DeviceOverheat | C::PowerSavingMode => {
                self.closed = true;
                self.launched = false;
                self.video_ready = false;
                self.open_in_main_display = false;
            }
            C::PeerConnectionClosedSuspended => {
                self.suspended = true;
                self.launched = false;
                self.video_ready = false;
                self.open_in_main_display = false;
            }
            C::PhoneStateConnecting if next != S::Locked => self.locked = false,
            C::PhoneStateLocked => {
                self.locked = true;
                self.screen_off = false;
                self.folded = false;
            }
            C::PhoneStateScreenOff => self.screen_off = true,
            C::PhoneStateUnlocked => {
                self.locked = false;
                self.screen_off = false;
                self.folded = false;
            }
            C::PhoneStateFoldedCannotDraw => self.folded = true,
            C::TrueTouchWarning => self.requires_true_touch = !self.do_not_show_true_touch,
            C::MoveToVirtualDisplay => self.open_in_main_display = true,
            C::RelaunchApp | C::RelaunchAll if next == S::Loading && from != S::Loading => {
                self.launched = false;
                self.closed = false;
                self.suspended = false;
                self.video_ready = false;
                self.open_in_main_display = false;
            }
            _ => {}
        }
    }

    /// The state the window moves to: the table's `next` with the flags' rules applied.
    fn resolve_next(&mut self, from: WindowState, mut next: WindowState) -> WindowState {
        use WindowState as S;
        if next == S::TrueTouchWarning && self.do_not_show_true_touch {
            next = S::Loading;
        }
        if next == S::Connected {
            next = self.resolve_connected(from);
        }
        if next == S::Restart && self.locked {
            next = S::Locked;
        }
        if next == S::TrueTouchWarning {
            if self.locked {
                next = S::Locked;
            } else if self.folded {
                next = S::FoldedCannotDraw;
            } else if self.screen_off {
                next = S::ScreenOff;
            }
        }

        // Leaving the warning (or staying in it) for anything but Locked ends the need for it.
        if from == S::TrueTouchWarning && next != S::Locked {
            self.requires_true_touch = false;
        }
        if from == S::Locked && next == S::Loading && self.requires_true_touch {
            next = S::TrueTouchWarning;
        }
        if next == S::ConnectionUnstable
            && matches!(from, S::AudioStreamingPermissionNeeded | S::AudioStreamingPermissionAlwaysDenied)
        {
            self.saved_audio_state = Some(from);
        }

        next
    }

    /// What the window shows instead of `Connected` while a flag holds it back.
    fn resolve_connected(&mut self, from: WindowState) -> WindowState {
        use WindowState as S;
        if self.suspended {
            S::Suspended
        } else if self.closed {
            S::Closed
        } else if !self.video_ready || !self.launched {
            S::Loading
        } else if self.folded {
            S::FoldedCannotDraw
        } else if self.locked {
            S::Locked
        } else if self.screen_off {
            S::ScreenOff
        } else if self.requires_true_touch && from != S::TrueTouchWarning && from != S::Restart {
            S::TrueTouchWarning
        } else if self.open_in_main_display && from != S::Restart {
            S::Restart
        } else if from == S::ConnectionUnstable && self.saved_audio_state.is_some() {
            self.saved_audio_state.take().expect("checked")
        } else {
            S::Connected
        }
    }

    /// The details the `next` state shows, from those given and those kept.
    fn details_for(&self, from: WindowState, next: WindowState, details: Option<Details>, preserve: bool) -> Details {
        use WindowState as S;
        if next == S::Restart {
            self.last_restart.clone()
        } else if next == S::Locked && !matches!(details, Some(Details::Locked { .. })) {
            self.last_locked.clone()
        } else if next == S::ScreenOff && details.is_none() {
            self.last_screen_off.clone()
        } else if preserve
            || (matches!(next, S::Error | S::DismissibleError)
                && !matches!(details, Some(Details::Error { .. }))
                && from == next)
        {
            self.details.clone()
        } else {
            details.unwrap_or_default()
        }
    }
}

/// The phone state's machine: rejects what its table does not allow.
pub fn phone_next(from: PhoneState, command: PhoneCommand) -> Option<PhoneState> {
    let next = phone_transition(from, command);
    if next.is_none() {
        debug!(from_state = ?from, ?command, "phone state transition not allowed; ignored");
    }
    next
}
