//! Call control: placing, answering, declining and ending the phone's calls from the Mac over
//! Bluetooth Hands-Free (HFP), with every call's audio on the phone.
//!
//! The Mac takes the HF role itself over an RFCOMM channel to the phone's HFP Audio Gateway service;
//! the phone's side is Android's own Bluetooth stack. No voice link is opened: the user talks on the
//! phone, so the controls that act on the Mac's audio (mute, hold, the in-call keypad, moving the
//! audio) do not exist here.
//!
//! | Module | Implements |
//! |---|---|
//! | [`at`] | the AT commands and the parsing of the phone's lines |
//! | `link` | the channel, its reader, one outstanding command at a time |
//! | `calls` | the calls from the phone's call list, and the call window's two tracked calls |
//! | `number` | the dialed number's normalization |
//! | `testing` | a scripted phone and Audio Gateway for tests |
//! | this module | the gates, the connection state and its triggers, the operations |
//!
//! The connection is set up when the feature starts for the selected phone, when "Show call history
//! on this Mac" or "Make and answer calls on this Mac" is switched on, and when the user asks; after a
//! failure or a lost connection, also when the app comes to the foreground and when a connect to the
//! phone completes. Nothing retries on a timer.

pub mod at;
mod calls;
mod link;
mod number;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::{Notify, broadcast, mpsc};
use tracing::{info, warn};

use phonelink_app::consts::capability;
use phonelink_app::phone_record::status_capability;
use phonelink_app::{AppEvent, PhoneRecord};
use phonelink_bluetooth::{Bluetooth, BluetoothFailure, RfcommError};
use phonelink_core::sync::MutexExt;
use phonelink_core::types::DcgClient;
use phonelink_features::{ContactMatch, FeatureError};

use self::at::{ClccEntry, Final, Line, command};
use self::calls::{CallBook, FollowUp};
use self::link::{Link, LinkEvent, Response};
use super::{CallingEvent, Shared, gates, store};

pub use self::calls::{Call, CallRole, CallStatus};
pub use self::number::normalize_dialable;

/// The HFP Audio Gateway service.
pub const AG_SERVICE_UUID: &str = "0000111F-0000-1000-8000-00805F9B34FB";

/// The trace scenario of the pairing call control runs.
const PAIRING_SCENARIO: &str = "Calling";

/// The connection's state for the selected phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlStatus {
    /// A gate is off.
    Off,
    /// The setup runs before the channel opens.
    Starting,
    BluetoothOff,
    /// No device paired with the Mac is the phone.
    NotPaired,
    /// The channel opens and the service-level connection runs.
    Connecting,
    /// Opening the channel or the service-level connection failed.
    ConnectionFailed,
    /// The channel closed after it was ready.
    NotConnected,
    Ready,
}

impl ControlStatus {
    /// The states the foreground and a completed connect retry from.
    fn retried_on_events(self) -> bool {
        matches!(self, ControlStatus::ConnectionFailed | ControlStatus::NotConnected)
    }
}

/// Call control's state for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlState {
    /// The phone can have call control: Android, the `Calls` capability, features not disabled.
    pub offered: bool,
    /// "Show call history on this Mac".
    pub pc_consent: bool,
    /// "Make and answer calls on this Mac".
    pub enabled: bool,
    pub status: ControlStatus,
}

/// Why a dial failed, as the calls view words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialFailure {
    /// The phone's device status says it has no network.
    NoNetwork,
    AirplaneMode,
    Failed,
}

/// Why an operation did not run or the phone refused it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlError {
    #[error("call control is not connected")]
    NotReady,
    #[error("the call is gone")]
    NoSuchCall,
    #[error("the call cannot do that now")]
    InvalidState,
    #[error("nothing to dial")]
    EmptyNumber,
    #[error("the phone did not place the call")]
    DialFailed(DialFailure),
    #[error("the phone refused: {0}")]
    Refused(String),
    #[error("{0}")]
    Bluetooth(BluetoothFailure),
    #[error("{0}")]
    Internal(String),
}

impl From<FeatureError> for ControlError {
    fn from(e: FeatureError) -> Self {
        ControlError::Internal(e.to_string())
    }
}

/// One call with the contact its number resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallView {
    pub call: Call,
    pub contact: Option<ContactMatch>,
}

/// What runs a setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// The feature started, a gate changed, or the user asked: always runs.
    Always,
    /// The foreground or a completed connect: runs only after a failure or a lost connection.
    AfterFailure,
}

/// The connection to the AG and what it reported.
#[derive(Default)]
struct Inner {
    phone: Option<DcgClient>,
    status: Option<ControlStatus>,
    link: Option<Arc<Link>>,
    /// Counts connections, so a closed link's late events change nothing.
    generation: u64,
    /// The indicator names in `+CIND` order.
    indicators: Vec<String>,
    book: CallBook,
}

/// Call control's handle.
pub struct CallControl {
    shared: Arc<Shared>,
    bluetooth: Arc<Bluetooth>,
    inner: Mutex<Inner>,
    /// One setup at a time.
    setup: tokio::sync::Mutex<()>,
    /// One call-list read and its application at a time, so lists apply in the order they came.
    list: tokio::sync::Mutex<()>,
    /// Woken after every change of the calls.
    calls_changed: Notify,
}

impl std::fmt::Debug for CallControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallControl").finish_non_exhaustive()
    }
}

impl CallControl {
    pub(crate) fn install(shared: Arc<Shared>, bluetooth: Arc<Bluetooth>) -> Arc<Self> {
        let control = Arc::new(Self {
            shared,
            bluetooth,
            inner: Mutex::new(Inner::default()),
            setup: tokio::sync::Mutex::new(()),
            list: tokio::sync::Mutex::new(()),
            calls_changed: Notify::new(),
        });
        if let Ok(app) = control.shared.app() {
            spawn_event_watch(&control, app.subscribe_events());
        }
        control
    }

    // MARK: - State

    /// `phone`'s call-control state.
    pub fn state(&self, phone: &DcgClient) -> Result<ControlState, FeatureError> {
        let record = self.shared.record_for(phone);
        let switches = store::state(&*self.shared.db(phone)?)?;
        let offered = record.as_ref().is_some_and(offered);

        let inner = self.inner.lock_or_recover();
        let ours = inner.phone.as_ref().is_some_and(|p| p.is_same_client(phone));
        let status = match inner.status {
            Some(status) if ours => status,
            _ => ControlStatus::Off,
        };
        Ok(ControlState { offered, pc_consent: switches.pc_consent, enabled: switches.call_control, status })
    }

    /// `phone`'s calls, ordered by the phone's index, each with the contact its number resolves to.
    pub fn calls(&self, phone: &DcgClient) -> Vec<CallView> {
        let calls = {
            let inner = self.inner.lock_or_recover();
            if !inner.phone.as_ref().is_some_and(|p| p.is_same_client(phone)) {
                return Vec::new();
            }
            inner.book.calls()
        };

        let lookup = |number: &str| {
            let contacts = self.shared.contacts.as_ref()?;
            (!number.is_empty()).then(|| contacts.lookup(phone, number, None)).flatten()
        };
        calls.into_iter().map(|call| CallView { contact: lookup(&call.number), call }).collect()
    }

    /// "Make and answer calls on this Mac" for `phone`. Switched on, the connection is set up (not
    /// waited for); switched off, it is closed.
    pub fn set_enabled(self: &Arc<Self>, phone: &DcgClient, enabled: bool) -> Result<(), FeatureError> {
        if !store::set_call_control(&*self.shared.db(phone)?, enabled)? {
            return Ok(());
        }

        info!(is_enabled = enabled, "call control on this Mac switched");
        self.shared.emit(CallingEvent::ControlChanged { phone: phone.clone() });
        self.spawn_setup(Trigger::Always);
        Ok(())
    }

    /// A gate the controller does not watch itself changed (the consent): set up or close.
    pub(crate) fn gates_changed(self: &Arc<Self>) {
        self.spawn_setup(Trigger::Always);
    }

    /// The app came to the foreground: a failed or lost connection is set up again.
    pub fn foreground(self: &Arc<Self>) {
        self.spawn_setup(Trigger::AfterFailure);
    }

    /// The user's *Connect* / *Try Again*: runs the setup and waits for it.
    pub async fn connect(self: &Arc<Self>) -> ControlStatus {
        self.run_setup(Trigger::Always).await
    }

    /// The user's *Begin Pairing*: pairs the Mac with the phone over Bluetooth (the shared pairing),
    /// then runs the setup.
    pub async fn pair(self: &Arc<Self>) -> Result<ControlStatus, ControlError> {
        let (phone, _) = self.selected()?;
        self.bluetooth.pair(&phone, PAIRING_SCENARIO).await.map_err(ControlError::Bluetooth)?;
        Ok(self.run_setup(Trigger::Always).await)
    }

    // MARK: - Operations

    /// Dials `typed`, normalized; on success the call shows in the call window.
    pub async fn dial(&self, typed: &str) -> Result<(), ControlError> {
        let number = normalize_dialable(typed);
        if number.is_empty() {
            return Err(ControlError::EmptyNumber);
        }

        let link = self.ready_link()?;
        let ok = matches!(link.command(&command::dial(&number), None).await, Ok(Response { result: Final::Ok, .. }));
        if !ok {
            let failure = self.selected().map(|(_, r)| dial_failure(&r)).unwrap_or(DialFailure::Failed);
            info!(failure_kind = ?failure, "the phone did not place the call");
            return Err(ControlError::DialFailed(failure));
        }

        self.inner.lock_or_recover().book.expect_dialed();
        self.refresh_calls(&link).await;
        Ok(())
    }

    /// Answers incoming call `id`; it shows in the call window. A waiting call is answered by holding
    /// the other call.
    pub async fn answer(&self, id: u64) -> Result<(), ControlError> {
        let (link, call) = self.call(id)?;
        if call.status != CallStatus::Incoming {
            return Err(ControlError::InvalidState);
        }

        let answer = if call.waiting { command::HOLD_AND_ACCEPT_OTHER } else { command::ANSWER };
        self.send(&link, answer).await?;
        let follow = self.inner.lock_or_recover().book.track(id);
        self.shared_calls_changed();
        self.follow_up(&link, follow.into_iter().collect()).await;
        self.refresh_calls(&link).await;
        Ok(())
    }

    /// Declines incoming call `id`.
    pub async fn decline(&self, id: u64) -> Result<(), ControlError> {
        let (link, call) = self.call(id)?;
        if call.status != CallStatus::Incoming {
            return Err(ControlError::InvalidState);
        }

        let decline = if call.waiting { command::RELEASE_HELD_OR_REJECT_WAITING } else { command::HANG_UP };
        self.send(&link, decline).await?;
        self.refresh_calls(&link).await;
        Ok(())
    }

    /// Ends call `id`. A held call is resumed first and ended once it talks (a held call next to
    /// another cannot be hung up directly); an incoming one is declined.
    pub async fn end(&self, id: u64) -> Result<(), ControlError> {
        let (link, call) = self.call(id)?;
        match call.status {
            CallStatus::Incoming => return self.decline(id).await,
            CallStatus::Held => {
                self.send(&link, command::HOLD_AND_ACCEPT_OTHER).await?;
                self.refresh_calls(&link).await;
                if !self.wait_until(id, CallStatus::Talking, self.shared.config.control_end_held_wait).await {
                    return Err(ControlError::Refused("the held call did not resume".into()));
                }
            }
            CallStatus::Talking | CallStatus::Dialing => {}
        }

        self.send(&link, command::HANG_UP).await?;
        self.refresh_calls(&link).await;
        Ok(())
    }

    /// Swaps the call window's calls: the held secondary call is resumed and the primary held.
    pub async fn swap(&self) -> Result<(), ControlError> {
        let link = self.ready_link()?;
        let held = {
            let inner = self.inner.lock_or_recover();
            let secondary = inner.book.calls().into_iter().find(|c| c.role == CallRole::Secondary);
            secondary.is_some_and(|c| c.status == CallStatus::Held)
        };
        if !held {
            return Err(ControlError::InvalidState);
        }

        self.send(&link, command::HOLD_AND_ACCEPT_OTHER).await?;
        self.inner.lock_or_recover().book.swap_roles();
        self.shared_calls_changed();
        self.refresh_calls(&link).await;
        Ok(())
    }

    // MARK: - Setup

    fn spawn_setup(self: &Arc<Self>, trigger: Trigger) {
        let control = self.clone();
        self.shared.runtime.spawn(async move {
            control.run_setup(trigger).await;
        });
    }

    /// The setup sequence under its slot: the gates, the radio, the paired device, the channel, the
    /// service-level connection.
    async fn run_setup(self: &Arc<Self>, trigger: Trigger) -> ControlStatus {
        let _slot = self.setup.lock().await;

        let selected = self.selected().ok();
        let wanted = selected.as_ref().is_some_and(|(phone, record)| self.gates(phone, record));
        let (phone, current) = {
            let inner = self.inner.lock_or_recover();
            (inner.phone.clone(), inner.status)
        };
        let same_phone = match (&phone, &selected) {
            (Some(p), Some((s, _))) => p.is_same_client(s),
            _ => false,
        };

        if !wanted || !same_phone {
            self.close("a gate is off or another phone is selected");
        }
        let Some((phone, _)) = selected.filter(|_| wanted) else {
            return ControlStatus::Off;
        };

        let status = if same_phone { current } else { None };
        match (status, trigger) {
            (Some(ControlStatus::Ready), _) => return ControlStatus::Ready,
            (Some(s), Trigger::AfterFailure) if !s.retried_on_events() => return s,
            (None, Trigger::AfterFailure) => return ControlStatus::Off,
            _ => {}
        }

        let outcome = self.connect_to(&phone).await;
        info!(control_status = ?outcome, "call control setup finished");
        outcome
    }

    async fn connect_to(self: &Arc<Self>, phone: &DcgClient) -> ControlStatus {
        self.set_status(phone, ControlStatus::Starting);

        let address = match self.bluetooth.paired_phone(phone).await {
            Ok(Some(address)) => address,
            Ok(None) => return self.set_status(phone, ControlStatus::NotPaired),
            Err(BluetoothFailure::BluetoothOff) => return self.set_status(phone, ControlStatus::BluetoothOff),
            Err(e) => {
                warn!(error = %e, "the phone's Bluetooth device was not looked up");
                return self.set_status(phone, ControlStatus::ConnectionFailed);
            }
        };

        self.set_status(phone, ControlStatus::Connecting);
        let (link, events) = match self.open(&address).await {
            Ok(opened) => opened,
            Err(e) => {
                warn!(error = %e, "the hands-free channel did not open");
                return self.set_status(phone, ControlStatus::ConnectionFailed);
            }
        };

        let indicators = match service_level_connection(&link).await {
            Ok(indicators) => indicators,
            Err(e) => {
                warn!(error = %e, "the hands-free service-level connection failed");
                link.close();
                return self.set_status(phone, ControlStatus::ConnectionFailed);
            }
        };

        let generation = {
            let mut inner = self.inner.lock_or_recover();
            inner.generation += 1;
            inner.link = Some(link.clone());
            inner.indicators = indicators;
            inner.book.clear();
            inner.generation
        };
        spawn_link_events(self, phone.clone(), link.clone(), events, generation);
        self.set_status(phone, ControlStatus::Ready);
        self.refresh_calls(&link).await;
        ControlStatus::Ready
    }

    /// Opens the channel; when the Mac's records lack the service, has them read again and tries once
    /// more.
    async fn open(&self, address: &str) -> Result<(Arc<Link>, mpsc::UnboundedReceiver<LinkEvent>), RfcommError> {
        match self.open_once(address).await {
            Err(RfcommError::ServiceNotFound) => {
                self.bluetooth.refresh_services(address, AG_SERVICE_UUID).await;
                self.open_once(address).await
            }
            other => other,
        }
    }

    async fn open_once(&self, address: &str) -> Result<(Arc<Link>, mpsc::UnboundedReceiver<LinkEvent>), RfcommError> {
        let edge = self.bluetooth.edge();
        let config = self.shared.config.clone();
        let runtime = self.shared.runtime.clone();
        let address = address.to_string();

        let opened = tokio::task::spawn_blocking(move || {
            Link::open(
                edge,
                &address,
                AG_SERVICE_UUID,
                config.control_open_timeout,
                config.control_read_poll,
                config.control_command_timeout,
                &runtime,
            )
        })
        .await;
        opened.map_err(|e| RfcommError::Unexpected { message: e.to_string() })?
    }

    /// Closes the connection, if any; its calls end.
    fn close(&self, why: &'static str) {
        let (link, phone) = {
            let mut inner = self.inner.lock_or_recover();
            if inner.link.is_none() && matches!(inner.status, None | Some(ControlStatus::Off)) {
                return;
            }
            inner.generation += 1;
            inner.book.clear();
            inner.status = Some(ControlStatus::Off);
            (inner.link.take(), inner.phone.clone())
        };

        if let Some(link) = link {
            info!(reason = why, "hands-free connection closed");
            link.close();
        }
        if let Some(phone) = phone {
            self.shared.emit(CallingEvent::ControlChanged { phone: phone.clone() });
            self.shared.emit(CallingEvent::ActiveCalls { phone });
        }
        self.calls_changed.notify_waiters();
    }

    fn set_status(&self, phone: &DcgClient, status: ControlStatus) -> ControlStatus {
        {
            let mut inner = self.inner.lock_or_recover();
            inner.phone = Some(phone.clone());
            inner.status = Some(status);
        }
        self.shared.emit(CallingEvent::ControlChanged { phone: phone.clone() });
        status
    }

    // MARK: - The link's events

    /// The link closed: its calls are lost, and the state is *Not connected*.
    fn link_closed(&self, phone: &DcgClient, generation: u64) {
        {
            let mut inner = self.inner.lock_or_recover();
            if inner.generation != generation {
                return;
            }
            inner.link = None;
            inner.book.clear();
        }

        self.set_status(phone, ControlStatus::NotConnected);
        self.shared_calls_changed();
    }

    /// An unsolicited line: `true` when the call list should be read again.
    fn unsolicited(&self, generation: u64, line: Line) -> bool {
        let mut inner = self.inner.lock_or_recover();
        if inner.generation != generation {
            return false;
        }

        match line {
            Line::Ciev { index, .. } => {
                let name = index.checked_sub(1).and_then(|i| inner.indicators.get(i));
                name.is_some_and(|n| matches!(n.as_str(), "call" | "callsetup" | "callheld"))
            }
            Line::Ring => true,
            Line::Clip(number) | Line::Ccwa(number) => {
                inner.book.set_caller(number);
                true
            }
            _ => false,
        }
    }

    /// Reads the call list and applies it, then runs what the tracked calls' rules ask for.
    async fn refresh_calls(&self, link: &Arc<Link>) {
        let follow = {
            let _list = self.list.lock().await;
            let entries = match link.command(command::CLCC, Some("+CLCC")).await {
                Ok(Response { result: Final::Ok, info }) => clcc_entries(info),
                Ok(r) => {
                    warn!(result = ?r.result, "the phone refused its call list");
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "the phone's call list was not read");
                    return;
                }
            };

            let now = self.shared.now_ms();
            let mut inner = self.inner.lock_or_recover();
            if !inner.link.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
                return;
            }
            inner.book.apply(&entries, now)
        };

        self.shared_calls_changed();
        self.follow_up(link, follow).await;
    }

    /// Ends a call the third tracked call pushed out, or resumes the one left held.
    async fn follow_up(&self, link: &Arc<Link>, follow: Vec<FollowUp>) {
        for step in follow {
            let result = match step {
                FollowUp::End(id) => {
                    let status = self.inner.lock_or_recover().book.get(id).map(|c| c.status);
                    match status {
                        Some(CallStatus::Held) => self.send(link, command::RELEASE_HELD_OR_REJECT_WAITING).await,
                        Some(_) => self.send(link, command::HANG_UP).await,
                        None => Ok(()),
                    }
                }
                FollowUp::Resume(_) => self.send(link, command::HOLD_AND_ACCEPT_OTHER).await,
            };
            if let Err(e) = result {
                warn!(error = %e, "a tracked call's follow-up failed");
            }
        }
    }

    // MARK: - Helpers

    fn selected(&self) -> Result<(DcgClient, PhoneRecord), ControlError> {
        let app = self.shared.app()?;
        let record = app.phone_record().ok_or(ControlError::NotReady)?;
        Ok((record.phone(), record))
    }

    /// The gates over the record and the switches.
    fn gates(&self, phone: &DcgClient, record: &PhoneRecord) -> bool {
        if !offered(record) {
            return false;
        }
        match self.shared.db(phone).and_then(|db| store::state(&db)) {
            Ok(s) => s.pc_consent && s.call_control,
            Err(e) => {
                warn!(error = %e, "call control's switches were not read");
                false
            }
        }
    }

    fn ready_link(&self) -> Result<Arc<Link>, ControlError> {
        let inner = self.inner.lock_or_recover();
        match (&inner.link, inner.status) {
            (Some(link), Some(ControlStatus::Ready)) if !link.is_closed() => Ok(link.clone()),
            _ => Err(ControlError::NotReady),
        }
    }

    fn call(&self, id: u64) -> Result<(Arc<Link>, Call), ControlError> {
        let link = self.ready_link()?;
        let call = self.inner.lock_or_recover().book.get(id).cloned().ok_or(ControlError::NoSuchCall)?;
        Ok((link, call))
    }

    /// Sends `command`; anything but `OK` is a refusal.
    async fn send(&self, link: &Link, command: &str) -> Result<(), ControlError> {
        match link.command(command, None).await {
            Ok(Response { result: Final::Ok, .. }) => Ok(()),
            Ok(r) => Err(ControlError::Refused(format!("{:?}", r.result))),
            Err(e) => Err(ControlError::Refused(e.to_string())),
        }
    }

    /// Waits up to `limit` for call `id` to reach `status`; `false` when it did not or is gone.
    async fn wait_until(&self, id: u64, status: CallStatus, limit: Duration) -> bool {
        let reached = || self.inner.lock_or_recover().book.get(id).map(|c| c.status == status);
        tokio::time::timeout(limit, async {
            loop {
                let changed = self.calls_changed.notified();
                match reached() {
                    Some(true) => return true,
                    None => return false,
                    Some(false) => changed.await,
                }
            }
        })
        .await
        .unwrap_or(false)
    }

    fn shared_calls_changed(&self) {
        let phone = self.inner.lock_or_recover().phone.clone();
        if let Some(phone) = phone {
            self.shared.emit(CallingEvent::ActiveCalls { phone });
        }
        self.calls_changed.notify_waiters();
    }
}

/// The phone can have call control: Android, the `Calls` capability, features not disabled.
fn offered(record: &PhoneRecord) -> bool {
    gates::is_android(record) && record.has_capability(capability::CALLS) && !record.all_features_disabled
}

/// The dial-failure wording, from the phone's device status: no network (`networkSignalStrength`
/// `-1`), else airplane mode when the phone reports it, else a failure.
fn dial_failure(record: &PhoneRecord) -> DialFailure {
    let status = &record.status_keys;
    if status.network_signal_strength == Some(-1) {
        DialFailure::NoNetwork
    } else if status.airplane_mode == Some(true) && record.status_capabilities & status_capability::AIRPLANE_MODE != 0 {
        DialFailure::AirplaneMode
    } else {
        DialFailure::Failed
    }
}

fn clcc_entries(info: Vec<Line>) -> Vec<ClccEntry> {
    info.into_iter()
        .filter_map(|l| match l {
            Line::Clcc(e) => Some(e),
            _ => None,
        })
        .collect()
}

/// Why the service-level connection failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum SlcError {
    #[error("{0}")]
    Link(#[from] link::LinkError),
    #[error("the phone answered {step} with {result:?}")]
    Refused { step: &'static str, result: Final },
    #[error("the phone's answer to {0} is missing its information line")]
    Missing(&'static str),
}

/// The service-level connection; returns the indicator names in order.
async fn service_level_connection(link: &Link) -> Result<Vec<String>, SlcError> {
    let ag_features = match ok(link, &command::brsf(), Some("+BRSF"), "AT+BRSF").await?.as_slice() {
        [Line::Brsf(features), ..] => *features,
        _ => return Err(SlcError::Missing("AT+BRSF")),
    };
    let indicators = match ok(link, command::CIND_TEST, Some("+CIND"), "AT+CIND=?").await?.as_slice() {
        [Line::CindNames(names), ..] => names.clone(),
        _ => return Err(SlcError::Missing("AT+CIND=?")),
    };
    ok(link, command::CIND_READ, Some("+CIND"), "AT+CIND?").await?;
    ok(link, command::CMER, None, "AT+CMER").await?;

    let three_way = ag_features & at::AG_THREE_WAY != 0;
    if three_way {
        ok(link, command::CHLD_TEST, Some("+CHLD"), "AT+CHLD=?").await?;
    }
    ok(link, command::CLIP, None, "AT+CLIP").await?;
    if three_way {
        ok(link, command::CCWA, None, "AT+CCWA").await?;
    }

    info!(feature_state = ag_features, has_three_way = three_way, "hands-free service-level connection established");
    Ok(indicators)
}

async fn ok(
    link: &Link,
    command: &str,
    prefix: Option<&'static str>,
    step: &'static str,
) -> Result<Vec<Line>, SlcError> {
    let response = link.command(command, prefix).await?;
    match response.result {
        Final::Ok => Ok(response.info),
        result => Err(SlcError::Refused { step, result }),
    }
}

/// Handles one link's events until it closes: re-reads the call list when the phone's lines ask for
/// it, at most one read for the lines that came meanwhile.
fn spawn_link_events(
    control: &Arc<CallControl>,
    phone: DcgClient,
    link: Arc<Link>,
    mut events: mpsc::UnboundedReceiver<LinkEvent>,
    generation: u64,
) {
    let control = Arc::downgrade(control);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            let Some(c) = control.upgrade() else { break };
            let mut read = false;
            let mut closed = false;
            for event in std::iter::once(event).chain(std::iter::from_fn(|| events.try_recv().ok())) {
                match event {
                    LinkEvent::Unsolicited(line) => read |= c.unsolicited(generation, line),
                    LinkEvent::Closed => closed = true,
                }
            }

            if closed {
                c.link_closed(&phone, generation);
                break;
            }
            if read {
                c.refresh_calls(&link).await;
            }
        }
    });
}

/// Watches the App layer for what the controller reacts to: the selected phone or its gates changing
/// (setup or close), and a completed connect to the phone (setup after a failure).
fn spawn_event_watch(control: &Arc<CallControl>, mut events: broadcast::Receiver<AppEvent>) {
    let control: Weak<CallControl> = Arc::downgrade(control);
    tokio::spawn(async move {
        let mut last: Option<(DcgClient, bool)> = None;
        loop {
            let trigger = match events.recv().await {
                Ok(AppEvent::ConnectCompleted { .. }) => Some(Trigger::AfterFailure),
                Ok(
                    AppEvent::PhoneRecordChanged { .. }
                    | AppEvent::PermissionsChanged { .. }
                    | AppEvent::AllFeaturesDisabled { .. },
                )
                | Err(broadcast::error::RecvError::Lagged(_)) => None,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let Some(c) = control.upgrade() else { break };
            if let Some(trigger) = trigger {
                c.spawn_setup(trigger);
                continue;
            }

            // A record change sets up or closes only when the selected phone or its offer changed.
            let Ok((phone, record)) = c.selected() else { continue };
            let now = (phone, offered(&record));
            if last.as_ref() != Some(&now) {
                last = Some(now);
                c.spawn_setup(Trigger::Always);
            }
        }
    });
}

#[cfg(test)]
mod tests;
