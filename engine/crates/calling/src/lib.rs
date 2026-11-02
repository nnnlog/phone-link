//! Calling's call history, the Android path: the phone mirrors its call log onto the Mac.
//!
//! The phone pushes `callLogs` content on `/legacy/phonecontent`; the PC answers a metadata message
//! with the ids it wants, stores the item messages that follow, asks for a sync with `/calling` and
//! for the call-log permission with `/permission` 256, and contributes to the connect message. Sync
//! is one-way, phone → PC: the Mac never changes or deletes the phone's calls. Placing and answering
//! calls (over Bluetooth) is not part of this crate.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the message keys, parsing with the malformed-content rules, the metadata answer, the `/calling` request |
//! | [`store`] | the per-phone call table, the `call_logs` sequence pair, the consent, the one automatic refresh, the badge mark |
//! | [`gates`] | the gates over the phone record |
//! | `sync` | the `callLogs` processor: metadata, items, completion |
//! | `connect` | the contribution to the connect message |
//! | this module | the requests and their triggers, and what the calls view reads |
//!
//! The PC asks for the call log on three occasions only: once ever, when the history is empty
//! ([`Calling::refresh_if_empty`], which the calls view runs); when "use calls on this Mac" is
//! switched on and the history is empty ([`Calling::set_pc_consent`]); and after the phone allowed
//! its call log ([`Calling::request_device_permission`]). Each answered request is followed by a
//! contacts sync while contacts may be synced, so the history's numbers resolve to names.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod gates;
pub mod store;
pub mod wire;

mod connect;
mod sync;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use phonelink_core::config::features::CallingConfig;
use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use phonelink_app::consts::permission;
use phonelink_app::{AppEvent, AppLayer, AppOutcome, ConnectProvider, PhoneRecord, RequestOptions};
use phonelink_core::edge::Clock;
use phonelink_core::types::{DcgClient, TraceContext};
use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError};
use phonelink_features::{ContactDirectory, ContactMatch, FeatureEnv, FeatureError};

pub use store::CallRow;
pub use wire::CallType;

/// `ypTriggerId` of the empty-history refresh (also run when the consent is switched on) and of the
/// permission prompt's action (Phone Link's names).
const TRIGGER_EMPTY: &str = "NoLocalDataAutoRefresh";
const TRIGGER_PROMPT: &str = "ErrorActionInvoked";
/// `ypScenarioId` of `/calling` and of the call-log `/permission` request.
const SCENARIO_SYNC: &str = "SyncAllCallLogs";
const SCENARIO_PERMISSION: &str = "CallLogsDevicePermission";

/// What tells the UI to re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallingEvent {
    /// `phone`'s calls changed: re-read [`Calling::history`] and the badge.
    HistoryChanged { phone: DcgClient },
    /// What [`Calling::status`] reports changed: the gates, the consent, the permission, the sync
    /// state, a refresh starting or ending, the badge mark.
    StateChanged { phone: DcgClient },
}

/// What the calls view shows in place of, or as, the history, in the order it is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallsAccess {
    /// No calls view for this phone: not the selected phone, not Android, or without the `Calls`
    /// capability.
    Unsupported,
    /// "Use calls on this Mac" is off.
    PcConsentRequired,
    /// The phone has `Calls` but not `CallLogs`; the view asks to update Link to Windows.
    CallLogsUnsupported,
    /// The phone must allow Link to Windows to post notifications before it is asked for its call
    /// log; the view explains how, and nothing is sent.
    PostNotificationRequired,
    /// The phone has not allowed its call log.
    DevicePermissionRequired,
    Granted,
}

/// The access the view shows for `record` and the consent.
fn access_of(record: Option<&PhoneRecord>, pc_consent: bool) -> CallsAccess {
    let Some(r) = record.filter(|r| gates::calls_view_supported(r)) else { return CallsAccess::Unsupported };
    if !pc_consent {
        CallsAccess::PcConsentRequired
    } else if !gates::call_logs_capable(r) {
        CallsAccess::CallLogsUnsupported
    } else if gates::call_log_permission(r) {
        CallsAccess::Granted
    } else if gates::post_notification_needed(r) {
        CallsAccess::PostNotificationRequired
    } else {
        CallsAccess::DevicePermissionRequired
    }
}

/// Call history's state for one phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallingStatus {
    pub access: CallsAccess,
    pub pc_consent: bool,
    /// `current ≥ expected` of the `call_logs` sequence pair.
    pub up_to_date: bool,
    /// The last completed call-log transaction, Unix ms.
    pub last_updated_ms: Option<i64>,
    /// A `/calling` request is in flight.
    pub refreshing: bool,
    /// Missed calls that started after the badge mark; `0` unless the access is
    /// [`CallsAccess::Granted`].
    pub unseen_missed: u32,
}

/// One call of the history with the contact its number resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub call: CallRow,
    pub contact: Option<ContactMatch>,
}

/// The outcome of the call-log permission prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPromptResult {
    pub outcome: AppOutcome,
    /// The phone did not answer in time; the permission is unchanged.
    pub timed_out: bool,
    /// The refresh that followed a granted permission.
    pub refresh: Option<AppOutcome>,
}

/// State shared by the handle, the processor factory, the connect provider and spawned requests.
/// Holds the App layer weakly: the App layer's registries hold the factory and the provider.
pub(crate) struct Shared {
    config: CallingConfig,
    stores: PhoneStores,
    app: Weak<AppLayer>,
    clock: Arc<dyn Clock>,
    contacts: Option<Arc<dyn ContactDirectory>>,
    runtime: Handle,
    events: broadcast::Sender<CallingEvent>,
    refreshing: AtomicBool,
}

impl Shared {
    pub(crate) fn db(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, StoreError> {
        self.stores.get(phone)
    }

    pub(crate) fn now_ms(&self) -> i64 {
        phonelink_core::time::unix_ms(self.clock.now())
    }

    pub(crate) fn emit(&self, event: CallingEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn app(&self) -> Result<Arc<AppLayer>, FeatureError> {
        self.app.upgrade().ok_or_else(|| FeatureError::Programming("the App layer is gone".into()))
    }

    /// `phone`'s record when it is the selected phone.
    fn record_for(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        self.app.upgrade()?.phone_record().filter(|r| r.phone().is_same_client(phone))
    }

    /// The selected phone and its record.
    fn selected(&self, app: &AppLayer) -> Result<(DcgClient, PhoneRecord), FeatureError> {
        let record = app.phone_record().ok_or_else(|| FeatureError::Programming("no phone is selected".into()))?;
        Ok((record.phone(), record))
    }

    /// Sends `/calling` (one attempt): `NeedsPermission` clears the call-log permission. Then, while
    /// contacts may be synced, a contacts sync follows. Nothing is sent while the phone reports no
    /// contacts capabilities (`Canceled`).
    async fn refresh(&self, app: &AppLayer, phone: &DcgClient, trigger: &str) -> Result<AppOutcome, FeatureError> {
        let Some(record) = self.record_for(phone) else {
            return Err(FeatureError::Programming("the phone is no longer selected".into()));
        };
        if record.contacts_capabilities.unwrap_or(0) == 0 {
            debug!("/calling not sent; the phone reports no contacts capabilities");
            return Ok(AppOutcome::Canceled);
        }

        self.refreshing.store(true, Ordering::SeqCst);
        self.emit(CallingEvent::StateChanged { phone: phone.clone() });
        let options = RequestOptions::default().named(trigger, SCENARIO_SYNC);
        let sent = app.request(wire::history_request(TraceContext::new_root(Vec::new()), options)).await;
        self.refreshing.store(false, Ordering::SeqCst);
        self.emit(CallingEvent::StateChanged { phone: phone.clone() });

        let outcome = sent?.outcome;
        info!(outcome = ?outcome, "/calling request finished");
        if outcome == AppOutcome::NeedsPermission {
            app.update_record_for(phone, |r| r.set_permission(permission::CALL_LOGS, false));
        }

        if let Some(contacts) = &self.contacts
            && contacts.sync_allowed(phone)
        {
            contacts.request_sync(false);
        }
        Ok(outcome)
    }

    /// The empty-history rule: the phone shares its call history and allows it, the consent is on,
    /// the history is empty and the one automatic refresh was not claimed — then it is claimed and
    /// sent (claimed even when [`Shared::refresh`] then sends nothing).
    async fn refresh_if_empty(&self) -> Result<Option<AppOutcome>, FeatureError> {
        let app = self.app()?;
        let (phone, record) = self.selected(&app)?;
        if !gates::history_supported(&record) || !gates::call_log_permission(&record) {
            return Ok(None);
        }

        let db = self.db(&phone)?;
        if !store::state(&db)?.pc_consent || !store::is_empty(&db)? || !store::claim_auto_refresh(&db)? {
            return Ok(None);
        }
        Ok(Some(self.refresh(&app, &phone, TRIGGER_EMPTY).await?))
    }

    fn status(&self, phone: &DcgClient) -> Result<CallingStatus, FeatureError> {
        let db = self.db(phone)?;
        let state = store::state(&db)?;
        let sync = store::sync_state(&db)?;
        let access = access_of(self.record_for(phone).as_ref(), state.pc_consent);

        Ok(CallingStatus {
            access,
            pc_consent: state.pc_consent,
            up_to_date: sync.up_to_date(),
            last_updated_ms: sync.last_updated_ms,
            refreshing: self.refreshing.load(Ordering::SeqCst),
            unseen_missed: if access == CallsAccess::Granted { store::unseen_missed(&db)? } else { 0 },
        })
    }
}

/// The call-history feature handle.
pub struct Calling {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Calling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Calling").finish_non_exhaustive()
    }
}

impl Calling {
    /// Registers the `callLogs` processor factory and the `Calling` connect provider, and watches the
    /// App layer's events for the gates. `contacts` names the history's numbers and is synced after
    /// each refresh. Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: CallingConfig,
        contacts: Option<Arc<dyn ContactDirectory>>,
    ) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("calling must be installed inside a tokio runtime".into()))?;

        let shared = Arc::new(Shared {
            config,
            stores: PhoneStores::new(env.stores.clone(), store::FEATURE, store::MIGRATIONS),
            app: Arc::downgrade(&env.app),
            clock: env.edges.clock.clone(),
            contacts,
            runtime: runtime.clone(),
            events: broadcast::channel(64).0,
            refreshing: AtomicBool::new(false),
        });

        env.app
            .register_processor_factory(&[wire::CONTENT_TYPE], Arc::new(sync::Factory { shared: shared.clone() }))?;
        env.app.register_connect_contributor(
            ConnectProvider::Calling,
            Arc::new(connect::Contributor { shared: shared.clone() }),
        )?;
        spawn_record_watch(&runtime, &shared, &env.app);
        Ok(Arc::new(Self { shared }))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CallingEvent> {
        self.shared.events.subscribe()
    }

    pub fn config(&self) -> &CallingConfig {
        &self.shared.config
    }

    /// `phone`'s access, consent, sync state and badge.
    pub fn status(&self, phone: &DcgClient) -> Result<CallingStatus, FeatureError> {
        self.shared.status(phone)
    }

    /// `phone`'s calls, newest first, each with the contact its number resolves to; none while the
    /// view's prompt stands in for the history (the access is not [`CallsAccess::Granted`]).
    pub fn history(&self, phone: &DcgClient) -> Result<Vec<HistoryEntry>, FeatureError> {
        if self.shared.status(phone)?.access != CallsAccess::Granted {
            return Ok(Vec::new());
        }

        let rows = store::history(&*self.shared.db(phone)?)?;
        let lookup = |number: &str| {
            let contacts = self.shared.contacts.as_ref()?;
            if number.is_empty() {
                return None;
            }
            contacts.lookup(phone, number, None)
        };
        Ok(rows.into_iter().map(|call| HistoryEntry { contact: lookup(&call.number), call }).collect())
    }

    /// The user saw the history: the badge counts only missed calls newer than the newest stored.
    pub fn mark_seen(&self, phone: &DcgClient) -> Result<(), FeatureError> {
        if store::mark_seen(&*self.shared.db(phone)?)? {
            self.shared.emit(CallingEvent::StateChanged { phone: phone.clone() });
        }
        Ok(())
    }

    /// "Use calls on this Mac". Switched on, the empty-history refresh runs (not waited for).
    pub fn set_pc_consent(&self, phone: &DcgClient, consent: bool) -> Result<(), FeatureError> {
        if !store::set_pc_consent(&*self.shared.db(phone)?, consent)? {
            return Ok(());
        }

        info!(enabled = consent, "call history on this Mac switched");
        self.shared.emit(CallingEvent::StateChanged { phone: phone.clone() });
        if consent {
            let shared = self.shared.clone();
            self.shared.runtime.spawn(async move {
                if let Err(e) = shared.refresh_if_empty().await {
                    warn!(error = %e, "call history not refreshed after the consent");
                }
            });
        }
        Ok(())
    }

    /// The empty-history rule of the calls view: sends `/calling` once when the selected phone's
    /// history is empty, the phone allows its call log and the consent is on. `None`: not sent.
    pub async fn refresh_if_empty(&self) -> Result<Option<AppOutcome>, FeatureError> {
        self.shared.refresh_if_empty().await
    }

    /// The "allow on your phone" action of the calls view: `/permission` with `permissionType` 256
    /// and `silent: false`, user-initiated, one attempt; accepted or success grants the call-log
    /// permission and a refresh follows.
    pub async fn request_device_permission(&self) -> Result<PermissionPromptResult, FeatureError> {
        let app = self.shared.app()?;
        let (phone, record) = self.shared.selected(&app)?;
        if !gates::history_supported(&record) {
            return Err(FeatureError::Programming("the selected phone has no call history for this Mac".into()));
        }

        let options = RequestOptions::user_initiated().named(TRIGGER_PROMPT, SCENARIO_PERMISSION);
        let response = app.request(app.permission_request(permission::CALL_LOGS, false, options)).await?;
        info!(outcome = ?response.outcome, "call log /permission request finished");

        let granted = matches!(response.outcome, AppOutcome::Success | AppOutcome::Accepted);
        let refresh = if granted {
            app.update_record_for(&phone, |r| r.set_permission(permission::CALL_LOGS, true));
            Some(self.shared.refresh(&app, &phone, TRIGGER_PROMPT).await?)
        } else {
            None
        };

        Ok(PermissionPromptResult {
            timed_out: response.outcome == AppOutcome::TimedOut,
            outcome: response.outcome,
            refresh,
        })
    }
}

/// Emits [`CallingEvent::StateChanged`] when the selected phone's gates or permissions change the
/// access (they live in the App layer's phone record).
fn spawn_record_watch(runtime: &Handle, shared: &Arc<Shared>, app: &Arc<AppLayer>) {
    let mut events = app.subscribe_events();
    let shared = Arc::downgrade(shared);
    let app = Arc::downgrade(app);

    runtime.spawn(async move {
        let mut last: Option<(DcgClient, CallsAccess)> = None;
        loop {
            match events.recv().await {
                Ok(AppEvent::PermissionsChanged { .. } | AppEvent::PhoneRecordChanged { .. })
                | Err(broadcast::error::RecvError::Lagged(_)) => {}
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }

            let (Some(shared), Some(app)) = (shared.upgrade(), app.upgrade()) else { break };
            let Some(record) = app.phone_record() else { continue };
            // The consent is the store's; its changes are announced where it is switched.
            let now = (record.phone(), access_of(Some(&record), true));
            if last.as_ref() != Some(&now) {
                shared.emit(CallingEvent::StateChanged { phone: now.0.clone() });
                last = Some(now);
            }
        }
    });
}
