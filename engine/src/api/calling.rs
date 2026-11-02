//! Call history for the UI ([`crate::features::calling`]).
//!
//! The calls view reads the selected phone's call history with the contact each number resolves to,
//! its access state (the prompt that stands in for the history), the sync state and the missed-call
//! badge; it switches "use calls on this Mac", asks the phone for its call log, and runs the
//! empty-history refresh.

use std::sync::Arc;

use crate::api::engine::Engine;
use crate::features::FeatureError;
use crate::features::calling::wire::CallType;
use crate::features::calling::{
    Calling, CallingEvent, CallingStatus, CallsAccess as FeatureAccess, HistoryEntry, PermissionPromptResult,
};
use crate::types::DcgClient;

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

/// A change to call history: re-read the history, or re-read [`CallingApi::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CallingChange {
    /// The calls changed: re-read the history and the state (the badge).
    HistoryChanged,
    /// What `state` reports changed.
    StateChanged,
}

impl From<&CallingEvent> for CallingChange {
    fn from(e: &CallingEvent) -> Self {
        match e {
            CallingEvent::HistoryChanged { .. } => CallingChange::HistoryChanged,
            CallingEvent::StateChanged { .. } => CallingChange::StateChanged,
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
}
