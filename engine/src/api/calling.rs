//! Call history and call control for the UI ([`crate::features::calling`]).
//!
//! The calls view reads the selected phone's call history with the contact each number resolves to,
//! its access state (the prompt that stands in for the history), the sync state and the missed-call
//! badge; it switches "use calls on this Mac", asks the phone for its call log, and runs the
//! empty-history refresh. Call control's state, the phone's current calls and the operations on them
//! (dial, answer, decline, end, swap) go through the same object; every call's audio stays on the
//! phone.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::FeatureError;
use crate::features::calling::control::{
    CallRole, CallStatus, CallView, ControlError, ControlState, ControlStatus, DialFailure,
};
use crate::features::calling::wire::CallType;
use crate::features::calling::{
    CallControl, Calling, CallingEvent, CallingStatus, CallsAccess as FeatureAccess, HistoryEntry,
    PermissionPromptResult,
};
use crate::types::DcgClient;

use super::bluetooth::BluetoothError;
use super::{FeatureApiError, RequestOutcome, core_of, features_of, selected_phone};

/// What the calls view shows in place of, or as, the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallsAccess {
    /// No calls view for this phone: not Android, or without the `Calls` capability.
    Unsupported,
    /// "Use calls on this Mac" is off.
    PcConsentRequired,
    /// The phone shares no call history; the view asks to update Link to Windows.
    CallLogsUnsupported,
    /// The phone must allow Link to Windows to post notifications first; nothing is sent.
    PostNotificationRequired,
    /// The phone has not allowed its call log.
    DevicePermissionRequired,
    /// The history is available.
    Granted,
}

impl From<FeatureAccess> for CallsAccess {
    fn from(a: FeatureAccess) -> Self {
        match a {
            FeatureAccess::Unsupported => Self::Unsupported,
            FeatureAccess::PcConsentRequired => Self::PcConsentRequired,
            FeatureAccess::CallLogsUnsupported => Self::CallLogsUnsupported,
            FeatureAccess::PostNotificationRequired => Self::PostNotificationRequired,
            FeatureAccess::DevicePermissionRequired => Self::DevicePermissionRequired,
            FeatureAccess::Granted => Self::Granted,
        }
    }
}

/// Call history's state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallingState {
    pub access: CallsAccess,
    /// "Use calls on this Mac" (default off).
    pub pc_consent: bool,
    pub up_to_date: bool,
    /// When the call history last finished syncing, Unix ms.
    pub last_updated_ms: Option<i64>,
    /// The Mac is asking the phone for its call history.
    pub refreshing: bool,
    /// Missed calls not yet seen in the calls view: the Calls section's badge.
    pub unseen_missed: u32,
}

impl From<CallingStatus> for CallingState {
    fn from(s: CallingStatus) -> Self {
        Self {
            access: s.access.into(),
            pc_consent: s.pc_consent,
            up_to_date: s.up_to_date,
            last_updated_ms: s.last_updated_ms,
            refreshing: s.refreshing,
            unseen_missed: s.unseen_missed,
        }
    }
}

/// What kind of call a history entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallKind {
    Incoming,
    Outgoing,
    Missed,
    Voicemail,
    Rejected,
    Blocked,
    /// Answered on another device.
    AnsweredExternally,
    /// The phone sent no type, or one this Mac does not know.
    Unknown,
}

impl From<CallType> for CallKind {
    fn from(t: CallType) -> Self {
        match t {
            CallType::Incoming => Self::Incoming,
            CallType::Outgoing => Self::Outgoing,
            CallType::Missed => Self::Missed,
            CallType::Voicemail => Self::Voicemail,
            CallType::Rejected => Self::Rejected,
            CallType::Blocked => Self::Blocked,
            CallType::AnsweredExternally => Self::AnsweredExternally,
            CallType::Unknown | CallType::Other(_) => Self::Unknown,
        }
    }
}

/// One call of the history.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallHistoryItem {
    pub call_id: i64,
    /// The other party's number as the phone stores it; empty when the phone has no number for the
    /// call (shown as a private number).
    pub number: String,
    /// The name of the contact the number resolves to; `None` when none has one.
    pub name: Option<String>,
    /// The contact the number resolves to.
    pub contact_id: Option<i64>,
    /// The contact whose photo shows for the number.
    pub photo_contact_id: Option<i64>,
    pub kind: CallKind,
    /// When the call started, Unix ms.
    pub start_ms: i64,
    pub duration_secs: i64,
    /// The phone's read flag.
    pub is_read: bool,
    /// The SIM subscription the call used; empty when it has none.
    pub phone_account_id: String,
}

impl From<HistoryEntry> for CallHistoryItem {
    fn from(e: HistoryEntry) -> Self {
        let HistoryEntry { call, contact } = e;
        Self {
            call_id: call.id,
            kind: CallType::from_i32(call.call_type).into(),
            name: contact.as_ref().and_then(|c| c.display_name.clone()),
            contact_id: contact.as_ref().map(|c| c.contact_id),
            photo_contact_id: contact.and_then(|c| c.photo_contact_id),
            number: call.number,
            start_ms: call.date,
            duration_secs: call.duration,
            is_read: call.read,
            phone_account_id: call.phone_account_id,
        }
    }
}

/// The outcome of the phone-side call-log permission prompt.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CallsPermissionResult {
    pub outcome: RequestOutcome,
    /// The phone did not answer in time; the permission is unchanged.
    pub timed_out: bool,
    /// The call-history refresh that followed a granted permission.
    pub refresh: Option<RequestOutcome>,
}

impl From<PermissionPromptResult> for CallsPermissionResult {
    fn from(r: PermissionPromptResult) -> Self {
        Self { outcome: r.outcome.into(), timed_out: r.timed_out, refresh: r.refresh.map(Into::into) }
    }
}

/// A change to calling: re-read the history, [`CallingApi::state`], [`CallingApi::control_state`] or
/// [`CallingApi::active_calls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallingChange {
    /// The calls changed: re-read the history and the state (the badge).
    HistoryChanged,
    /// What `state` reports changed.
    StateChanged,
    /// What `control_state` reports changed.
    ControlChanged,
    /// The phone's current calls changed: re-read `active_calls`.
    ActiveCalls,
}

impl From<&CallingEvent> for CallingChange {
    fn from(e: &CallingEvent) -> Self {
        match e {
            CallingEvent::HistoryChanged { .. } => CallingChange::HistoryChanged,
            CallingEvent::StateChanged { .. } => CallingChange::StateChanged,
            CallingEvent::ControlChanged { .. } => CallingChange::ControlChanged,
            CallingEvent::ActiveCalls { .. } => CallingChange::ActiveCalls,
        }
    }
}

/// Call control's connection to the phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallControlStatus {
    /// "Make and answer calls on this Mac" or "Show call history on this Mac" is off, or the phone
    /// cannot have call control.
    Off,
    /// Getting ready.
    Starting,
    /// The Mac's Bluetooth is off.
    BluetoothOff,
    /// No device paired with the Mac is the phone: *Begin Pairing*.
    NotPaired,
    Connecting,
    /// The connection failed: *Try Again*.
    ConnectionFailed,
    /// The connection was lost: *Connect*.
    NotConnected,
    Ready,
}

impl From<ControlStatus> for CallControlStatus {
    fn from(s: ControlStatus) -> Self {
        match s {
            ControlStatus::Off => Self::Off,
            ControlStatus::Starting => Self::Starting,
            ControlStatus::BluetoothOff => Self::BluetoothOff,
            ControlStatus::NotPaired => Self::NotPaired,
            ControlStatus::Connecting => Self::Connecting,
            ControlStatus::ConnectionFailed => Self::ConnectionFailed,
            ControlStatus::NotConnected => Self::NotConnected,
            ControlStatus::Ready => Self::Ready,
        }
    }
}

/// Call control's state for the selected phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CallControlState {
    /// The phone can have call control (Android, with calling, features not disabled).
    pub offered: bool,
    /// "Show call history on this Mac", which call control needs too.
    pub pc_consent: bool,
    /// "Make and answer calls on this Mac" (default off).
    pub enabled: bool,
    pub status: CallControlStatus,
}

impl From<ControlState> for CallControlState {
    fn from(s: ControlState) -> Self {
        Self { offered: s.offered, pc_consent: s.pc_consent, enabled: s.enabled, status: s.status.into() }
    }
}

/// A current call's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ActiveCallStatus {
    /// Ringing.
    Incoming,
    /// Being dialed, or ringing at the other end.
    Dialing,
    Talking,
    Held,
}

impl From<CallStatus> for ActiveCallStatus {
    fn from(s: CallStatus) -> Self {
        match s {
            CallStatus::Incoming => Self::Incoming,
            CallStatus::Dialing => Self::Dialing,
            CallStatus::Talking => Self::Talking,
            CallStatus::Held => Self::Held,
        }
    }
}

/// Where a current call shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ActiveCallRole {
    /// Placed or answered on the phone: not in the call window.
    Untracked,
    /// The call window's main call.
    Primary,
    /// The call window's other call.
    Secondary,
}

impl From<CallRole> for ActiveCallRole {
    fn from(r: CallRole) -> Self {
        match r {
            CallRole::Untracked => Self::Untracked,
            CallRole::Primary => Self::Primary,
            CallRole::Secondary => Self::Secondary,
        }
    }
}

/// One of the phone's current calls.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActiveCall {
    pub id: u64,
    /// The other party's number; empty when the phone gave none.
    pub number: String,
    /// The name of the contact the number resolves to.
    pub name: Option<String>,
    pub contact_id: Option<i64>,
    pub photo_contact_id: Option<i64>,
    pub incoming: bool,
    pub status: ActiveCallStatus,
    /// Incoming while another call exists.
    pub waiting: bool,
    /// When the Mac first saw the call talking, Unix ms.
    pub talking_since_ms: Option<i64>,
    pub role: ActiveCallRole,
}

impl From<CallView> for ActiveCall {
    fn from(v: CallView) -> Self {
        let CallView { call, contact } = v;
        Self {
            id: call.id,
            name: contact.as_ref().and_then(|c| c.display_name.clone()),
            contact_id: contact.as_ref().map(|c| c.contact_id),
            photo_contact_id: contact.and_then(|c| c.photo_contact_id),
            number: call.number,
            incoming: call.incoming,
            status: call.status.into(),
            waiting: call.waiting,
            talking_since_ms: call.talking_since_ms,
            role: call.role.into(),
        }
    }
}

/// Why a dial failed, as the calls view words it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum DialFailureKind {
    /// The phone has no network ("Unable to read SIM card").
    NoNetwork,
    /// The phone is in airplane mode.
    AirplaneMode,
    /// "Call failed".
    Failed,
}

/// Why a call-control operation did not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CallControlError {
    #[error("{error}")]
    Feature { error: FeatureApiError },
    /// Call control is not connected to the phone.
    #[error("call control is not connected")]
    NotReady,
    /// The call ended meanwhile.
    #[error("the call is gone")]
    NoSuchCall,
    /// The call is not in a state the operation applies to.
    #[error("the call cannot do that now")]
    InvalidState,
    #[error("nothing to dial")]
    EmptyNumber,
    #[error("the phone did not place the call")]
    DialFailed { kind: DialFailureKind },
    #[error("the phone refused: {reason}")]
    Refused { reason: String },
    #[error("{error}")]
    Bluetooth { error: BluetoothError },
    #[error("internal: {message}")]
    Internal { message: String },
}

impl From<FeatureApiError> for CallControlError {
    fn from(error: FeatureApiError) -> Self {
        CallControlError::Feature { error }
    }
}

impl From<ControlError> for CallControlError {
    fn from(e: ControlError) -> Self {
        match e {
            ControlError::NotReady => Self::NotReady,
            ControlError::NoSuchCall => Self::NoSuchCall,
            ControlError::InvalidState => Self::InvalidState,
            ControlError::EmptyNumber => Self::EmptyNumber,
            ControlError::DialFailed(kind) => Self::DialFailed {
                kind: match kind {
                    DialFailure::NoNetwork => DialFailureKind::NoNetwork,
                    DialFailure::AirplaneMode => DialFailureKind::AirplaneMode,
                    DialFailure::Failed => DialFailureKind::Failed,
                },
            },
            ControlError::Refused(reason) => Self::Refused { reason },
            ControlError::Bluetooth(f) => Self::Bluetooth { error: f.into() },
            ControlError::Internal(message) => Self::Internal { message },
        }
    }
}

/// Call history of the selected phone.
#[derive(uniffi::Object)]
pub struct CallingApi {
    engine: Arc<Engine>,
}

impl CallingApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(&self, f: impl FnOnce(&Calling, &DcgClient) -> Result<T, FeatureError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.calling.clone();
        let phone = selected_phone(&core)?;
        Ok(f(&feature, &phone)?)
    }

    fn feature(&self) -> Result<Arc<Calling>, FeatureApiError> {
        let core = core_of(&self.engine)?;
        Ok(features_of(&core)?.calling.clone())
    }

    /// Runs `f` with call control on the engine's runtime.
    async fn control<T: Send + 'static, Fut>(
        &self,
        f: impl FnOnce(Arc<CallControl>) -> Fut,
    ) -> Result<T, CallControlError>
    where
        Fut: Future<Output = Result<T, ControlError>> + Send + 'static,
    {
        let core = core_of(&self.engine)?;
        let control = features_of(&core)?.calling.control().clone();

        match core.runtime_handle().spawn(f(control)).await {
            Ok(result) => Ok(result?),
            Err(e) => Err(CallControlError::Internal { message: e.to_string() }),
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl CallingApi {
    /// The selected phone's call-history access, its consent, how fresh the sync is, and the badge.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<CallingState, FeatureApiError> {
        self.run(|f, phone| Ok(f.status(phone)?.into()))
    }

    /// The calls, newest first, with the contact each number resolves to. None while the access is
    /// not `Granted`: the view shows its prompt instead.
    pub fn history(&self) -> Result<Vec<CallHistoryItem>, FeatureApiError> {
        self.run(|f, phone| Ok(f.history(phone)?.into_iter().map(Into::into).collect()))
    }

    /// The user saw the calls: the badge counts only missed calls newer than those shown.
    pub fn mark_seen(&self) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.mark_seen(phone))
    }

    /// "Use calls on this Mac". Switched on, the phone is asked for its call history once if none is
    /// stored and the phone allows it.
    pub fn set_pc_consent(&self, consent: bool) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.set_pc_consent(phone, consent))
    }

    /// Asks the phone for its call history once, while none is stored, the phone allows it and the
    /// consent is on; `None` when it was not asked.
    pub async fn refresh_if_empty(&self) -> Result<Option<RequestOutcome>, FeatureApiError> {
        Ok(self.feature()?.refresh_if_empty().await?.map(Into::into))
    }

    /// Asks the phone to allow its call log; a grant is followed by a call-history refresh.
    pub async fn request_permission(&self) -> Result<CallsPermissionResult, FeatureApiError> {
        Ok(self.feature()?.request_device_permission().await?.into())
    }

    /// Call control's switch and connection for the selected phone.
    pub fn control_state(&self) -> Result<CallControlState, FeatureApiError> {
        self.run(|f, phone| Ok(f.control().state(phone)?.into()))
    }

    /// "Make and answer calls on this Mac". Switched on, the Mac connects to the phone over Bluetooth
    /// (not waited for).
    pub fn set_control_enabled(&self, enabled: bool) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.control().set_enabled(phone, enabled))
    }

    /// The phone's current calls.
    pub fn active_calls(&self) -> Result<Vec<ActiveCall>, FeatureApiError> {
        self.run(|f, phone| Ok(f.control().calls(phone).into_iter().map(Into::into).collect()))
    }

    /// Connects to the phone over Bluetooth now (*Connect*, *Try Again*); returns the state reached.
    pub async fn connect_control(&self) -> Result<CallControlStatus, CallControlError> {
        self.control(|c| async move { Ok(c.connect().await) }).await.map(Into::into)
    }

    /// Pairs the Mac with the phone over Bluetooth (*Begin Pairing*), then connects.
    pub async fn pair_for_control(&self) -> Result<CallControlStatus, CallControlError> {
        self.control(|c| async move { c.pair().await }).await.map(Into::into)
    }

    /// Dials `number` on the phone; the call's audio stays on the phone.
    pub async fn dial(&self, number: String) -> Result<(), CallControlError> {
        self.control(|c| async move { c.dial(&number).await }).await
    }

    /// Answers incoming call `id` on the phone.
    pub async fn answer(&self, id: u64) -> Result<(), CallControlError> {
        self.control(|c| async move { c.answer(id).await }).await
    }

    /// Declines incoming call `id`.
    pub async fn decline(&self, id: u64) -> Result<(), CallControlError> {
        self.control(|c| async move { c.decline(id).await }).await
    }

    /// Ends call `id`.
    pub async fn end_call(&self, id: u64) -> Result<(), CallControlError> {
        self.control(|c| async move { c.end(id).await }).await
    }

    /// Resumes the call window's held call and holds the other.
    pub async fn swap(&self) -> Result<(), CallControlError> {
        self.control(|c| async move { c.swap().await }).await
    }
}
