//! Device status: the two device-status contracts, Phone Link's and Cross Device's, one status per phone
//! built from both models, the DND and ringer controls, and the low-battery notification.
//!
//! | Module | Implements | |---|---| | [`cdeh`] | the CDEH contract: status pull, status push, the PC status
//! for the phone; sender preconditions | | [`phone_link`] | the Phone Link contract: sync request, status
//! content, commands; the contribution to the connect message; the `wifiState` reset | | [`model`] | the CDEH
//! decoding of `DeviceStatusResponseMessage` into `DeviceStatus`; the status model and what is shown when
//! data is missing | | [`battery`] | the low-battery notification | | [`ring`] | ring my phone: the ring
//! control, its request and what it shows | | [`edge`] | the PC status for the phone from its macOS sources:
//! `IOKit`, `NWPathMonitor`, `CoreWLAN` |
//!
//! This module holds the feature handle: the CDEH model in memory, the controls' pending states and
//! confirmation timers, the ring control's countdown, the pull triggers, and the UI-facing operations. The
//! Phone Link model is the phone record's [`StatusKeys`](phonelink_app::StatusKeys), which every writer
//! updates in arrival order.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

uniffi::setup_scaffolding!();

pub mod battery;
pub mod cdeh;
pub mod edge;
pub mod model;
pub mod phone_link;
pub mod ring;

use phonelink_core::config::features::DeviceStatusConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use phonelink_app::phone_record::{StatusChanges, load_record};
use phonelink_app::{ConnectProvider, PhoneRecord};
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores};
use phonelink_features::shared::notifier::{NotificationResponder, NotificationResponse};
use phonelink_features::shared::ongoing::OngoingFailure;
use phonelink_features::{FeatureEnv, FeatureError};
use phonelink_transport::platform::{DeviceFilter, Drm, Msaep, Subscription};

pub use edge::PcStatusSource;

mod cdeh_sync;
mod controls;
mod listeners;
mod low_battery;
mod ring_phone;
#[cfg(feature = "test-support")]
pub use edge::{PcBattery, PcInternetPath};
pub use model::{
    BatteryItem, CdehStatus, CellularItem, DndPending, DndShown, RadioState, RingerPending, RingerShown, StatusView,
    WifiItem,
};
#[cfg(feature = "test-support")]
pub use model::{DndControl, RingerControl};

pub use ring::{RingControl, RingRequest, RingShown};

use model::{Overlay, Received, ViewInputs, build_view, decode_received, select};
use phone_link::command;
use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;

/// The feature's name: its store and its notification ids (`device_status/…`).
pub const FEATURE: &str = "device_status";

/// A prompt a control shows instead of sending; [`DeviceStatus::accept_prompt`] runs its action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Prompt {
    /// The DND control needs the phone's notifications-sync permission; the action requests it.
    DndNotificationsPermission,
    /// The phone's Android 15 DND rule for Link to Windows is disabled; the action sends command `9`.
    DndRuleDisabled,
    /// The ringer control needs the notifications-sync permission; the action requests it.
    RingerNotificationsPermission,
    /// A DND turn-off was not confirmed in time; the action sends command `9`.
    DndTurnOffFailed,
    /// The user asked to stop a ringing phone; the action stops it.
    StopRinging,
}

/// What activating a control did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlOutcome {
    /// `deviceStatusCommandType` / `deviceStatusCommandValue` were sent; the control shows its
    /// pending state until a change of the Phone Link value or its timer.
    Sent { command_type: i32, value: i32 },
    /// A prompt is shown instead of sending.
    Prompt(Prompt),
    /// The ring control sent its request (ring my phone).
    RingSent(RingRequest),
    /// The control does not exist or is disabled (including while a change is pending): nothing sent.
    NotAvailable,
}

/// What the UI hears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStatusEvent {
    /// What [`DeviceStatus::status`] shows for `phone` may have changed.
    Changed { phone: DcgClient },
    /// The engine raises a prompt by itself (the failure prompt of a reverted DND turn-off).
    PromptRequested { phone: DcgClient, prompt: Prompt },
    /// A prompt raised earlier no longer applies (the DND status became off).
    PromptWithdrawn { phone: DcgClient, prompt: Prompt },
    /// The user activated the low-battery notification (Windows opens its notification settings).
    LowBatteryNotificationActivated { phone: DcgClient },
}

/// A phone's map key: DCG ids compare case-insensitively.
fn key(phone: &DcgClient) -> DcgClient {
    DcgClient::new(phone.id.to_ascii_lowercase(), phone.env)
}

/// A control's state besides the selected value.
#[derive(Debug, Clone)]
struct ControlState<P> {
    /// The command sent and not confirmed, with the id its timer checks.
    pending: Option<(u64, P)>,
    /// What the timer reverted the control to, and the selected value it is shown over.
    reverted: Option<(i32, Option<i32>)>,
    /// What this control compares a record change against: the inputs its activation saw, then
    /// those of each record change processed since. Each control keeps its own, so activating one
    /// control never hides from the other a change written before the activation but processed
    /// after it.
    seen: Option<Seen>,
}

impl<P> Default for ControlState<P> {
    fn default() -> Self {
        Self { pending: None, reverted: None, seen: None }
    }
}

impl<P: Copy> ControlState<P> {
    fn overlay(&self, selected: Option<i32>) -> Overlay<P> {
        if let Some((_, pending)) = self.pending {
            return Overlay::Pending(pending);
        }
        match self.reverted {
            Some((shown, base)) if base == selected => Overlay::Reverted(shown),
            _ => Overlay::None,
        }
    }

    /// The control is re-read from the model: a pending or reverted state ends, no revert.
    fn reset(&mut self) {
        self.pending = None;
        self.reverted = None;
    }

    /// A reverted value is shown until the selected value changes: once it differs from the value the
    /// revert was shown over, the reverted state ends for good (a later return to that value shows
    /// the value, not the revert). A pending state is not affected (only a Phone Link change, or a
    /// re-read, ends it).
    fn selected_now(&mut self, selected: Option<i32>) {
        if self.reverted.is_some_and(|(_, base)| base != selected) {
            self.reverted = None;
        }
    }

    /// The record already holds a change this control re-reads on (`rereads`) that it has not
    /// compared yet: written after its activation or the last record change it processed, its record
    /// event still queued.
    fn unprocessed_change(&self, record: &PhoneRecord, rereads: fn(&Seen, &Seen) -> bool) -> bool {
        self.seen.is_some_and(|before| rereads(&before, &Seen::of(record)))
    }
}

/// What a record change is compared against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    changes: StatusChanges,
    post_notification: bool,
    notifications_sync: bool,
    all_features_disabled: bool,
}

impl Seen {
    fn of(record: &PhoneRecord) -> Self {
        Self {
            changes: record.status_keys.changes,
            post_notification: record.permission_or(phonelink_app::consts::permission::POST_NOTIFICATIONS, true),
            notifications_sync: model::notifications_sync_granted(record),
            all_features_disabled: record.all_features_disabled,
        }
    }

    /// The DND control is re-read on a change of the status, the rule flag, either permission or the
    /// disabled state.
    fn dnd_reset(&self, now: &Seen) -> bool {
        self.changes.do_not_disturb_status != now.changes.do_not_disturb_status
            || self.changes.do_not_disturb_rule_enabled != now.changes.do_not_disturb_rule_enabled
            || self.post_notification != now.post_notification
            || self.notifications_sync != now.notifications_sync
            || self.all_features_disabled != now.all_features_disabled
    }

    /// … and the ringer control on a change of the mode or the disabled state.
    fn ringer_reset(&self, now: &Seen) -> bool {
        self.changes.ringer_mode != now.changes.ringer_mode || self.all_features_disabled != now.all_features_disabled
    }
}

#[derive(Debug, Default)]
struct PhoneState {
    /// The CDEH model; `None` until an `OK` status arrived.
    cdeh: Option<CdehStatus>,
    dnd: ControlState<DndPending>,
    ringer: ControlState<RingerPending>,
    /// The failure prompt of a reverted DND turn-off is outstanding.
    failure_prompt: bool,
    /// The ring control shows the phone ringing.
    ring: Option<Ringing>,
}

/// A ring the control counts down.
#[derive(Debug, Clone, Copy)]
struct Ringing {
    /// The id its countdown task checks.
    id: u64,
    ends_at: tokio::time::Instant,
    /// The user asked to stop once; the next stop sends it.
    confirm_stop: bool,
}

#[derive(Debug, Default)]
struct State {
    phones: HashMap<DcgClient, PhoneState>,
    next_id: u64,
    /// The selected phone and its selected battery level when the low-battery rule last ran.
    battery: Option<(DcgClient, Option<i32>)>,
}

impl State {
    fn phone(&mut self, phone: &DcgClient) -> &mut PhoneState {
        self.phones.entry(key(phone)).or_default()
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

/// The feature's shared state; tasks and registered callbacks hold it weakly.
pub(crate) struct Inner {
    env: FeatureEnv,
    config: DeviceStatusConfig,
    drm: Arc<Drm>,
    store: PhoneStores,
    state: Mutex<State>,
    /// Serializes low-battery evaluations (check, post, record).
    battery_lock: Mutex<()>,
    events: broadcast::Sender<DeviceStatusEvent>,
    runtime: Handle,
    shutdown: CancellationToken,
    subscription: Mutex<Option<Subscription>>,
    weak: Weak<Inner>,
    /// The stored records of phones that are not selected cannot be read (each status change reads
    /// one).
    stored_record_failing: OngoingFailure,
    /// The low-battery settings cannot be read (each battery level change reads them).
    low_battery_store_failing: OngoingFailure,
    /// The low-battery notification cannot be posted (each battery level change below the
    /// threshold tries again).
    low_battery_post_failing: OngoingFailure,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// The device-status feature.
pub struct DeviceStatus {
    inner: Arc<Inner>,
}

impl DeviceStatus {
    /// Installs the feature (the first step of the sequence): the tag-8 subscriber and the
    /// `/remotedevicestatus` receiver, the `deviceStatus` processor, the `SyncDeviceStatus` connect
    /// provider and the Wi-Fi-state request observer, the notification responder; then the
    /// listeners, the pull of every phone that is already available with `DEVICE_STATUS`, and the
    /// low-battery rule.
    /// `msaep` and `drm` are the platform routes' handles; `pc_status` answers `/remotedevicestatus`.
    /// Must run inside a tokio runtime.
    pub fn install(
        env: &FeatureEnv,
        config: DeviceStatusConfig,
        msaep: Arc<Msaep>,
        drm: Arc<Drm>,
        pc_status: Arc<dyn PcStatusSource>,
    ) -> Result<Arc<Self>, FeatureError> {
        validate_config(&config)?;
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("device status must be installed inside a tokio runtime".into()))?;

        let inner = Arc::new_cyclic(|weak| Inner {
            env: env.clone(),
            config,
            drm: drm.clone(),
            store: PhoneStores::new(env.stores.clone(), FEATURE, battery::MIGRATIONS),
            state: Mutex::new(State::default()),
            battery_lock: Mutex::new(()),
            events: broadcast::channel(64).0,
            runtime,
            shutdown: CancellationToken::new(),
            subscription: Mutex::new(None),
            weak: weak.clone(),
            stored_record_failing: OngoingFailure::default(),
            low_battery_store_failing: OngoingFailure::default(),
            low_battery_post_failing: OngoingFailure::default(),
        });

        let session_events = env.sessions.subscribe();
        let peer_events = env.peers.subscribe();
        let app_events = env.app.subscribe_events();

        // CDEH.
        let subscription = msaep.subscribe(
            cdeh::STATUS_TAG,
            DeviceFilter::Any,
            Arc::new(cdeh::StatusPushObserver(Arc::downgrade(&inner))),
        );
        *inner.subscription.lock_or_panic() = Some(subscription);
        drm.register_receiver(cdeh::REMOTE_STATUS_PATH, Arc::new(cdeh::RemoteStatusReceiver::new(pc_status)))
            .map_err(RegistrationError::from)?;

        // Phone Link.
        env.app.register_processor_factory(&[phone_link::CONTENT_TYPE], Arc::new(phone_link::DeviceStatusFactory))?;
        env.app.register_connect_contributor(
            ConnectProvider::SyncDeviceStatus,
            Arc::new(phone_link::SyncDeviceStatusContributor),
        )?;
        env.app.register_request_observer(Arc::new(phone_link::WifiStateReset::new(&env.app)));
        env.responses
            .register(FEATURE, Arc::new(Responder(Arc::downgrade(&inner))))
            .map_err(RegistrationError::from)?;

        inner.spawn_listeners(session_events, peer_events, app_events);
        inner.start_pulls();
        inner.check_battery();
        Ok(Arc::new(DeviceStatus { inner }))
    }

    /// Events for the UI.
    pub fn subscribe(&self) -> broadcast::Receiver<DeviceStatusEvent> {
        self.inner.events.subscribe()
    }

    /// `phone`'s status as the menu-bar item shows it.
    pub fn status(&self, phone: &DcgClient) -> StatusView {
        self.inner.view(phone)
    }

    /// The CDEH model of `phone` (`None` until an `OK` status arrived).
    pub fn cdeh_status(&self, phone: &DcgClient) -> Option<CdehStatus> {
        self.inner.state.lock_or_panic().phones.get(&key(phone)).and_then(|p| p.cdeh.clone())
    }

    /// The user activates the DND control: a prompt when the notifications-sync permission is missing
    /// or (with the Android 15 DND rule support) the DND rule is disabled; otherwise command `2` —
    /// value `2` when the shown DND is `1`, else `1` — and the pending state with its confirmation
    /// timer.
    pub fn toggle_do_not_disturb(&self, phone: &DcgClient) -> ControlOutcome {
        self.inner.toggle_do_not_disturb(phone)
    }

    /// The user activates the ringer control: a prompt when the notifications-sync permission is
    /// missing; otherwise command `3` with the next mode (`2 → 1 → 0 → 2`) and the pending state.
    pub fn cycle_ringer_mode(&self, phone: &DcgClient) -> ControlOutcome {
        self.inner.cycle_ringer_mode(phone)
    }

    /// The user accepts `prompt`. A DND teaching tip acts on the tip that applies now: the permission
    /// request, or command `9`.
    pub fn accept_prompt(&self, phone: &DcgClient, prompt: Prompt) {
        self.inner.accept_prompt(phone, prompt);
    }

    /// The user rings the phone from a ring control that shows off: the control counts down
    /// ([`DeviceStatusConfig::ring_my_phone_countdown`]) and the ON request is sent; its outcome does
    /// not change the control.
    pub fn ring_my_phone(&self, phone: &DcgClient) -> ControlOutcome {
        self.inner.ring_my_phone(phone)
    }

    /// The user stops a ringing phone: the first stop of a ring returns
    /// [`Prompt::StopRinging`] (the confirmation); the next one shows off and sends the OFF request,
    /// and a failed OFF brings the ringing state back with a new countdown.
    pub fn stop_ringing(&self, phone: &DcgClient) -> ControlOutcome {
        self.inner.stop_ringing(phone)
    }

    /// The low-battery notification setting of `phone` (default on).
    pub fn low_battery_notifications_enabled(&self, phone: &DcgClient) -> Result<bool, FeatureError> {
        Ok(battery::load(&*self.inner.db(phone)?)?.enabled)
    }

    pub fn set_low_battery_notifications_enabled(&self, phone: &DcgClient, enabled: bool) -> Result<(), FeatureError> {
        let _guard = self.inner.battery_lock.lock_or_panic();
        let db = self.inner.db(phone)?;
        let settings = battery::load(&db)?;
        battery::save(&db, battery::LowBatterySettings { enabled, ..settings })?;
        info!(%phone, enabled, "low-battery notifications switched");
        Ok(())
    }
}

impl Inner {
    fn emit(&self, event: DeviceStatusEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn changed(&self, phone: &DcgClient) {
        self.emit(DeviceStatusEvent::Changed { phone: phone.clone() });
    }

    fn db(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, FeatureError> {
        Ok(self.store.get(&key(phone))?)
    }

    /// The selected phone's live record when `phone` is selected; otherwise its stored one.
    fn record_for(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        if self.env.app.selected_phone().is_some_and(|s| s.is_same_client(phone)) {
            return self.env.app.phone_record();
        }

        match load_record(&*self.env.edges.store, phone) {
            Ok(record) => {
                self.stored_record_failing.clear();
                record
            }
            Err(e) => {
                if self.stored_record_failing.starts() {
                    error!(%phone, error = %e, "stored phone record not read; the phone's Phone Link status is shown unknown");
                }
                None
            }
        }
    }

    /// The selected phone's record when `phone` is the selected phone (the controls are Phone Link's).
    fn selected_record(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        self.env.app.phone_record().filter(|r| r.phone().is_same_client(phone))
    }

    fn view(&self, phone: &DcgClient) -> StatusView {
        let record = self.record_for(phone);
        let connected = self.env.sessions.is_connected(phone);
        let mut state = self.state.lock_or_panic();
        let ps = state.phone(phone);
        let default_keys = phonelink_app::StatusKeys::default();
        let sel = select(record.as_ref().map_or(&default_keys, |r| &r.status_keys), ps.cdeh.as_ref());

        let mut view = build_view(&ViewInputs {
            phone,
            record: record.as_ref(),
            cdeh: ps.cdeh.as_ref(),
            connected,
            dnd: ps.dnd.overlay(sel.do_not_disturb_status),
            ringer: ps.ringer.overlay(sel.ringer_mode),
        });

        let selected = self.env.app.selected_phone().is_some_and(|s| s.is_same_client(phone));
        let ring = ps.ring;
        drop(state);
        view.ring_my_phone =
            record.as_ref().filter(|_| selected).and_then(|r| self.ring_control(phone, r, view.live, ring));
        view
    }

    /// The ring control of the selected phone's `record`, `None` when it does not exist.
    fn ring_control(
        &self,
        phone: &DcgClient,
        record: &PhoneRecord,
        live: bool,
        ring: Option<Ringing>,
    ) -> Option<RingControl> {
        let in_account = self.env.peers.is_enrolled() && self.env.peers.peer(phone).is_some();
        if !ring::exists(record, self.config.ring_my_phone_enabled, in_account) {
            return None;
        }

        let shown = match ring {
            _ if !live => RingShown::Unknown,
            None => RingShown::Off,
            Some(r) => RingShown::Ringing {
                remaining_seconds: ring::remaining_seconds(
                    r.ends_at.saturating_duration_since(tokio::time::Instant::now()),
                ),
                confirm_stop: r.confirm_stop,
            },
        };

        let ringing = matches!(shown, RingShown::Ringing { .. });
        Some(RingControl { shown, enabled: live, can_stop: ringing && self.config.stop_ring_my_phone_enabled })
    }

    /// The selected phone's ring control now, with its live record.
    fn current_ring_control(&self, phone: &DcgClient) -> Option<RingControl> {
        let record = self.selected_record(phone)?;
        let live = self.env.sessions.is_connected(phone) && !record.all_features_disabled;
        let ring = self.state.lock_or_panic().phone(phone).ring;
        self.ring_control(phone, &record, live, ring)
    }
}

/// Routes responses to `device_status/…` notifications.
struct Responder(Weak<Inner>);

impl NotificationResponder for Responder {
    fn on_response(&self, id: &str, user_info: &HashMap<String, String>, response: NotificationResponse) {
        let Some(inner) = self.0.upgrade() else { return };
        match (battery::notification_phone(user_info), response) {
            (Some(phone), NotificationResponse::Activated | NotificationResponse::Action { .. }) => {
                inner.emit(DeviceStatusEvent::LowBatteryNotificationActivated { phone });
            }
            (Some(_), _) => {}
            (None, _) => debug!(id, "response to an unknown device-status notification; dropped"),
        }
    }
}

/// Fails for an option value the engine does not build.
fn validate_config(config: &DeviceStatusConfig) -> Result<(), FeatureError> {
    if !config.android15_do_not_disturb {
        return Err(FeatureError::Config(
            "android15_do_not_disturb off is not supported: the engine builds only on".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
