//! Phone notifications mirrored on the Mac.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the message catalog, request keys, trace context names, settings identifiers |
//! | [`item`] | the notification item JSON (identity, display, actions, classification) |
//! | `inbound` | inbound notification batches, phone → PC (content message, applying a batch, reply, batch announcement) |
//! | [`outbound`], `triggers` | outbound notification requests (operations, triggers (a)–(d), the silent permission check), permission requests |
//! | `queue` | the persistent action queue |
//! | [`settings`], `preferences` | per-app notification settings (requests 25 / 26 / 27, responses, local-only settings) |
//! | `connect` | the contribution to the connect message |
//! | [`presentation`], `responses` | presentation on macOS and the responses to what it posts |
//! | [`store`] | preconditions and state: the per-phone notification store, queue, settings and values |
//!
//! [`install`] registers the `notifications` content processor, the `SyncNotifications` connect
//! provider and the `notifications` notification responder, and starts the feature's tasks: the
//! permission watcher (full syncs (b) and (c)) and one action-queue sender per phone. Everything the
//! UI does goes through [`Notifications`]; [`Notifications::subscribe`] says when to re-read.
//!
//! The device permission is the phone record's `NotificationsSync` bit (`0x40`, default off) and
//! the sensitive-notification permission its `NotificationsSensitive` bit (`0x80000000`, default
//! off); the shared App layer writes both from the connect response, the `permissions` push and
//! `phoneCommand` type 7, and this feature writes the first from `permissions.notifications` and
//! the outcome of its `/permission` requests.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

mod connect;
mod inbound;
pub mod item;
pub mod outbound;
mod preferences;
pub mod presentation;
mod queue;
mod responses;
pub mod settings;
pub mod store;
mod triggers;
pub mod wire;

use phonelink_core::config::features::NotificationsConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::UNIX_EPOCH;

use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use phonelink_app::consts::{capability as phone_capability, permission, platform};
use phonelink_app::{AppError, AppLayer, AppOutcome, ConnectProvider, PhoneRecord};
use phonelink_core::codec::valueset::Value;
use phonelink_core::edge::Clock;
use phonelink_core::types::DcgClient;
use phonelink_features::shared::db::{FeatureDb, PhoneStores, StoreError};
use phonelink_features::shared::notifier::SystemNotifier;
use phonelink_features::shared::ongoing::OngoingFailure;
use phonelink_features::{FeatureEnv, FeatureError, MessagingAlerts, PhoneAppDirectory};

pub use item::{ConversationLine, ItemAction};
pub use settings::AppFlags;

use phonelink_core::sync::MutexExt;
use phonelink_features::RegistrationError;
use presentation::{Dedupe, Throttle, ThrottleConfig};
use store::value as stored;

mod views;
pub use views::{
    AppSettingView, MuteDuration, NotificationView, NotificationsStatus, PreferencesChange, PreferencesResult,
    PreferencesSource,
};

/// The feature name: the store name and the notification-id prefix.
pub const FEATURE: &str = "notifications";

/// Where a click on a notification body takes the user (it sends nothing to the phone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTarget {
    /// The app's notification view.
    Notifications,
    /// `notificationClass` 1: the messaging view.
    Messaging,
    /// `notificationClass` 3: the calls view.
    Calls,
}

/// What changed, for the UI to re-read.
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationsEvent {
    /// The stored notifications changed (batch, dismiss, clear all, pin).
    ListChanged { phone: DcgClient },
    /// [`NotificationsStatus`] changed (permissions, capabilities, syncing, last updated, mutes).
    StatusChanged { phone: DcgClient },
    /// Per-app settings rows changed.
    SettingsChanged { phone: DcgClient },
    /// A queued action or reply reached its final status; `outcome` is `None` when it was dropped
    /// unsent after the queue expiry.
    ActionFinished { phone: DcgClient, key: String, reply: bool, outcome: Option<AppOutcome> },
    /// The user clicked a posted notification.
    OpenRequested { phone: DcgClient, key: String, target: OpenTarget },
}

/// Why a UI operation could not run.
#[derive(Debug, thiserror::Error)]
pub enum NotificationsError {
    #[error("no phone is selected")]
    NoPhone,
    #[error("the App layer is gone")]
    AppGone,
    #[error("the notification is not stored")]
    UnknownNotification,
    #[error("the notification has no such action")]
    UnknownAction,
    #[error("not available: {0}")]
    NotAvailable(&'static str),
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("App layer: {0}")]
    App(#[from] AppError),
}

/// The text form of a `ValueSet` value other than `Empty`, an array or an `Object`
/// (`correlationVector` is "any (text form)"): a `String` as it is; numbers in decimal (floats in
/// their shortest round-trip form); `Bool` `True`/`False`; `Char16` the character (U+FFFD for a lone
/// surrogate); a GUID lower-case and hyphenated; `DateTime` in the round-trip form
/// `yyyy-MM-ddTHH:mm:ss.fffffff+00:00`; `TimeSpan` in the constant form `[-][d.]hh:mm:ss[.fffffff]`;
/// `Point`, `Size` and `Rect` their coordinates joined by `,`. `None` for the rest.
pub(crate) fn value_text(v: &Value) -> Option<String> {
    Some(match v {
        Value::String(s) => s.clone(),
        Value::UInt8(n) => n.to_string(),
        Value::Int16(n) => n.to_string(),
        Value::UInt16(n) => n.to_string(),
        Value::Int32(n) => n.to_string(),
        Value::UInt32(n) => n.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::UInt64(n) => n.to_string(),
        Value::Single(f) => f.to_string(),
        Value::Double(f) => f.to_string(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Char16(c) => char::from_u32(u32::from(*c)).unwrap_or(char::REPLACEMENT_CHARACTER).to_string(),
        Value::Guid(g) => g.hyphenated().to_string(),
        Value::DateTime(ticks) => datetime_text(*ticks),
        Value::TimeSpan(ticks) => timespan_text(*ticks),
        Value::Point(p) => format!("{},{}", p.x, p.y),
        Value::Size(s) => format!("{},{}", s.width, s.height),
        Value::Rect(r) => format!("{},{},{},{}", r.x, r.y, r.width, r.height),
        _ => return None,
    })
}

/// 100-ns ticks per day.
const TICKS_PER_DAY: i64 = 864_000_000_000;

/// A `DateTime` (100-ns ticks since 1601-01-01 UTC) as `yyyy-MM-ddTHH:mm:ss.fffffff+00:00`.
fn datetime_text(ticks: i64) -> String {
    // Days between 1601-01-01 and 1970-01-01.
    const DAYS_1601_TO_1970: i64 = 134_774;
    let (days, rem) = (ticks.div_euclid(TICKS_PER_DAY), ticks.rem_euclid(TICKS_PER_DAY));
    let (year, month, day) = phonelink_core::time::civil_from_days(days - DAYS_1601_TO_1970);
    let (h, m, s, f) = clock_parts(rem);
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{f:07}+00:00")
}

/// A `TimeSpan` (100-ns ticks) in the constant form `[-][d.]hh:mm:ss[.fffffff]`.
fn timespan_text(ticks: i64) -> String {
    let sign = if ticks < 0 { "-" } else { "" };
    let abs = ticks.unsigned_abs();
    let per_day = TICKS_PER_DAY as u64;
    let (days, rem) = (abs / per_day, abs % per_day);
    let (h, m, s, f) = clock_parts(rem as i64);
    let days = if days > 0 { format!("{days}.") } else { String::new() };
    let fraction = if f > 0 { format!(".{f:07}") } else { String::new() };
    format!("{sign}{days}{h:02}:{m:02}:{s:02}{fraction}")
}

/// Hours, minutes, seconds and 100-ns fraction of a tick count within one day.
fn clock_parts(rem: i64) -> (i64, i64, i64, i64) {
    let secs = rem / 10_000_000;
    (secs / 3600, secs / 60 % 60, secs % 60, rem % 10_000_000)
}

/// The device permission: the record's `NotificationsSync` bit, off until the phone reports it.
pub fn device_permission(record: &PhoneRecord) -> bool {
    record.permission_or(permission::NOTIFICATIONS_SYNC, false)
}

/// The sensitive-notification permission, off until the phone reports it.
pub(crate) fn sensitive_permission(record: &PhoneRecord) -> bool {
    record.permission_or(permission::NOTIFICATIONS_SENSITIVE, false)
}

/// The feature exists for the phone: `NotificationsListener`.
fn capable(record: &PhoneRecord) -> bool {
    record.has_capability(phone_capability::NOTIFICATIONS_LISTENER)
}

/// Per-app list support: `NotificationsListener` and `PhoneApps`.
pub(crate) fn app_list_supported(record: &PhoneRecord) -> bool {
    capable(record) && record.has_capability(phone_capability::PHONE_APPS)
}

/// The phone is Android for this feature unless its record says iOS (the app's phones are Android).
pub fn is_android(record: &PhoneRecord) -> bool {
    record.platform != platform::IOS
}

/// The post-notification setup stage: the phone is Android with a major OS version of 13 or later,
/// its `PostNotifications` permission is reported missing (unreported counts as granted) and its
/// `AutoLaunch` permission is not held (unreported counts as not held).
pub(crate) fn post_notification_needed(record: &PhoneRecord) -> bool {
    record.post_notification_ask_supported()
        && !record.permission_or(permission::POST_NOTIFICATIONS, true)
        && !record.permission_or(permission::AUTO_LAUNCH, false)
}

/// The permission inputs of full syncs (b) and (c), as last seen — through each `PermissionsChanged`
/// event of the App layer, in order, and each PC-permission switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PermissionState {
    pc: bool,
    device: bool,
    sensitive: bool,
}

/// In-memory state of one phone.
#[derive(Default)]
struct PhoneRuntime {
    /// Full syncs in flight ("syncing").
    full_syncs: usize,
    dedupe: Dedupe,
    throttle: Throttle,
    permissions: Option<PermissionState>,
    /// The capability bits last seen (a change is announced as a status change).
    capability_bits: Option<u32>,
    /// [`post_notification_needed`] as last seen (a change is announced as a status change).
    post_notification_needed: Option<bool>,
    /// The phone's queue sender, once started.
    queue: Option<Arc<Notify>>,
}

/// The feature's shared state. The App layer's registries, the notification router and the
/// feature's tasks reach it; it holds the App layer weakly.
pub(crate) struct Inner {
    config: NotificationsConfig,
    app: Weak<AppLayer>,
    clock: Arc<dyn Clock>,
    notifier: Arc<dyn SystemNotifier>,
    stores: PhoneStores,
    messaging: Option<Arc<dyn MessagingAlerts>>,
    /// The phone's app list (set once after install): the list's app names, the app icon of banners.
    app_directory: OnceLock<Arc<dyn PhoneAppDirectory>>,
    runtime: Handle,
    phones: Mutex<HashMap<DcgClient, PhoneRuntime>>,
    /// The selected phone as last seen by the event watcher.
    selected: Mutex<Option<DcgClient>>,
    /// One settings request in flight at a time.
    settings_gate: tokio::sync::Mutex<()>,
    events: broadcast::Sender<NotificationsEvent>,
    shutdown: CancellationToken,
    weak_self: Weak<Inner>,
    /// System notifications cannot be posted (each phone notification tries).
    post_failing: OngoingFailure,
    /// System notifications cannot be withdrawn (each removal tries).
    withdraw_failing: OngoingFailure,
    /// Stored notifications do not parse (each read of the list meets them).
    unparsable_stored: OngoingFailure,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl Inner {
    fn app(&self) -> Result<Arc<AppLayer>, NotificationsError> {
        self.app.upgrade().ok_or(NotificationsError::AppGone)
    }

    /// The App layer and its selected phone.
    fn selected(&self) -> Result<(Arc<AppLayer>, DcgClient), NotificationsError> {
        let app = self.app()?;
        let phone = app.selected_phone().ok_or(NotificationsError::NoPhone)?;
        Ok((app, phone))
    }

    fn db(&self, phone: &DcgClient) -> Result<Arc<FeatureDb>, StoreError> {
        self.stores.get(phone)
    }

    /// Wall-clock now (Unix ms).
    fn now_ms(&self) -> i64 {
        match self.clock.now().duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
        }
    }

    fn emit(&self, event: NotificationsEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    fn arc(&self) -> Option<Arc<Inner>> {
        self.weak_self.upgrade()
    }

    fn with_phone<R>(&self, phone: &DcgClient, f: impl FnOnce(&mut PhoneRuntime) -> R) -> R {
        let mut phones = self.phones.lock_or_panic();
        f(phones.entry(phone.clone()).or_default())
    }

    fn bool_value(db: &FeatureDb, name: &str, default: bool) -> Result<bool, StoreError> {
        Ok(store::stored_value(db, name)?.map_or(default, |v| v != 0))
    }

    /// "Show my phone's notifications" (default on).
    fn pc_permission(db: &FeatureDb) -> Result<bool, StoreError> {
        Self::bool_value(db, stored::PC_PERMISSION, true)
    }

    /// `NotificationsCapabilities` (default `0`).
    fn capabilities(db: &FeatureDb) -> Result<i32, StoreError> {
        Ok(store::stored_value(db, stored::CAPABILITIES)?.map_or(0, |v| v as i32))
    }

    fn throttling_enabled(db: &FeatureDb) -> Result<bool, StoreError> {
        Self::bool_value(db, stored::THROTTLING_ENABLED, true)
    }

    fn muted_all(&self, db: &FeatureDb) -> Result<bool, StoreError> {
        let until = store::stored_value(db, stored::MUTE_ALL_UNTIL_TICKS)?.unwrap_or(0);
        Ok(until > phonelink_core::time::ticks_from_unix_ms(self.now_ms()))
    }

    /// The quiet-by-default preference: the stored one once the user set it, else All.
    fn default_preference(db: &FeatureDb) -> Result<AppFlags, StoreError> {
        if Self::bool_value(db, stored::DEFAULT_PREFERENCE_SET, false)? {
            let v = store::stored_value(db, stored::DEFAULT_PREFERENCE)?.unwrap_or(3);
            return Ok(AppFlags(u32::try_from(v).unwrap_or(3)));
        }
        Ok(AppFlags::ALL)
    }

    /// An app's effective flags and its mute expiry (UTC ticks, `0` = none).
    fn app_flags(&self, db: &FeatureDb, package: &str) -> Result<(AppFlags, i64), StoreError> {
        let sync = store::setting(db, wire::setting::SYNC_GROUP, package)?;
        let local = store::setting(db, wire::setting::LOCAL_GROUP, package)?;
        let flags = settings::effective_flags(
            sync.as_ref(),
            local.as_ref(),
            self.config.quiet_by_default,
            Self::default_preference(db)?,
        );
        Ok((flags, local.map_or(0, |l| l.mute_expiration_ticks)))
    }

    fn set_last_updated(&self, phone: &DcgClient) {
        let now = self.now_ms();
        match self.db(phone).and_then(|db| store::set_value(&db, stored::LAST_UPDATED_MS, now)) {
            Ok(()) => self.emit(NotificationsEvent::StatusChanged { phone: phone.clone() }),
            Err(e) => error!(error = %e, "notifications last-updated time not stored; the sync time shown is stale"),
        }
    }

    /// Withdraws the presented notifications of `keys`.
    fn withdraw(&self, phone: &DcgClient, keys: impl IntoIterator<Item = String>) {
        let ids: Vec<String> = keys.into_iter().map(|k| presentation::notification_id(phone, &k)).collect();
        if ids.is_empty() {
            return;
        }

        match self.notifier.withdraw(ids) {
            Ok(()) => self.withdraw_failing.clear(),
            Err(e) => {
                if self.withdraw_failing.starts() {
                    warn!(error = %e, "system notifications not withdrawn; removed phone notifications stay shown");
                }
            }
        }
    }

    /// Posts `n` when the app's notification authorization allows it.
    fn post(&self, n: phonelink_features::SystemNotification) {
        if !self.notifier.is_authorized() {
            return;
        }

        match self.notifier.post(n) {
            Ok(()) => self.post_failing.clear(),
            Err(e) => {
                if self.post_failing.starts() {
                    warn!(error = %e, "system notification not posted");
                }
            }
        }
    }

    fn status(&self) -> Result<NotificationsStatus, NotificationsError> {
        let (app, phone) = self.selected()?;
        let record = app.phone_record().ok_or(NotificationsError::NoPhone)?;
        let db = self.db(&phone)?;
        let capabilities = Self::capabilities(&db)?;

        let wanted =
            wire::capability::SENSITIVE_NOTIFICATIONS_ENABLED | wire::capability::PROFILE_COMPUTER_ROLE_AVAILABLE;
        let mute_all = store::stored_value(&db, stored::MUTE_ALL_UNTIL_TICKS)?.unwrap_or(0);
        let now_ticks = phonelink_core::time::ticks_from_unix_ms(self.now_ms());

        Ok(NotificationsStatus {
            phone: phone.clone(),
            capable: capable(&record),
            app_list_supported: app_list_supported(&record),
            pc_permission: Self::pc_permission(&db)?,
            device_permission: device_permission(&record),
            sensitive_permission: sensitive_permission(&record),
            capabilities,
            sensitive_prompt_offered: capabilities & wanted == wanted,
            syncing: self.with_phone(&phone, |p| p.full_syncs > 0),
            last_updated_ms: store::stored_value(&db, stored::LAST_UPDATED_MS)?,
            muted_all_until_ms: (mute_all > now_ticks).then(|| phonelink_core::time::unix_ms_from_ticks(mute_all)),
            throttling_enabled: Self::throttling_enabled(&db)?,
            low_ram_device: Self::bool_value(&db, stored::LOW_RAM_DEVICE, false)?,
            setup_completed: Self::bool_value(&db, stored::SETUP_COMPLETED, false)?,
            post_notification_needed: post_notification_needed(&record),
        })
    }

    /// The list: newest first; media-style items never; an unpinned item only while its app's
    /// feed flag is on.
    fn list(&self) -> Result<Vec<NotificationView>, NotificationsError> {
        let (_, phone) = self.selected()?;
        self.list_for(&phone)
    }

    fn list_for(&self, phone: &DcgClient) -> Result<Vec<NotificationView>, NotificationsError> {
        let db = self.db(phone)?;
        let mut flags_cache: HashMap<String, AppFlags> = HashMap::new();
        let mut out = Vec::new();
        let mut unparsable = (0usize, None);

        for row in store::all_rows(&db)? {
            let item = match item::NotificationItem::parse(&row.json) {
                Ok(i) => i,
                Err(e) => {
                    unparsable = (unparsable.0 + 1, unparsable.1.or(Some(e.reason())));
                    continue;
                }
            };

            if item.is_media_style() {
                continue;
            }

            let pinned = row.state == store::pinned::PINNED;
            if !pinned {
                let flags = match flags_cache.get(&item.package_name) {
                    Some(f) => *f,
                    None => {
                        let f = self.app_flags(&db, &item.package_name)?.0;
                        flags_cache.insert(item.package_name.clone(), f);
                        f
                    }
                };
                if !flags.feed() {
                    continue;
                }
            }

            let mut view = view_of(&item, pinned);
            if let Some(app) = self.phone_app(phone, &item.package_name) {
                view.app_name = Some(app.name);
            }
            out.push(view);
        }

        match unparsable {
            (0, _) => self.unparsable_stored.clear(),
            (skipped_count, reason) => {
                if self.unparsable_stored.starts() {
                    error!(skipped_count, reason, "stored notifications do not parse; they are not listed");
                }
            }
        }

        Ok(out)
    }

    /// `package`'s app in the phone's app list, when it is known.
    pub(crate) fn phone_app(&self, phone: &DcgClient, package: &str) -> Option<phonelink_features::PhoneAppInfo> {
        self.app_directory.get()?.app(phone, package)
    }

    /// The unpinned, non-media items the list shows (the clear-all set), newest first.
    fn clearable_keys(&self, phone: &DcgClient) -> Result<Vec<String>, NotificationsError> {
        Ok(self.list_for(phone)?.into_iter().filter(|v| !v.pinned).map(|v| v.key).collect())
    }
}

/// A list item from a stored item.
fn view_of(item: &item::NotificationItem, pinned: bool) -> NotificationView {
    let display = item.display();
    let open_target = if item.is_messaging() {
        OpenTarget::Messaging
    } else if item.is_phone_call() {
        OpenTarget::Calls
    } else {
        OpenTarget::Notifications
    };

    NotificationView {
        key: item.key.clone(),
        package_name: item.package_name.clone(),
        app_name: item.app_name().map(str::to_string),
        title: display.title,
        subtitle: item.subtitle().map(str::to_string),
        body: display.body,
        conversation: display.conversation,
        time_ms: item.event_time_ms(),
        pinned,
        open_target,
        launch_supported: item.launch_supported(),
        is_read_only_group_message: item.is_read_only_group_message(),
        reply: item.list_reply(),
        actions: item.list_actions(),
    }
}

/// The notifications feature: what the UI reads and does.
pub struct Notifications {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Notifications {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifications").finish_non_exhaustive()
    }
}

/// Installs the feature.
///
/// Registers the `notifications` content processor, the `SyncNotifications` connect provider and
/// the `notifications` notification responder, starts the permission watcher, and starts draining
/// the selected phone's action queue.
///
/// `messaging` answers whether a group
/// message (`isReadOnlyGroupMessage`) may alert; `None` ⇒ never.
pub fn install(
    env: &FeatureEnv,
    config: NotificationsConfig,
    messaging: Option<Arc<dyn MessagingAlerts>>,
) -> Result<Arc<Notifications>, FeatureError> {
    validate_config(&config)?;
    let runtime = Handle::try_current()
        .map_err(|_| FeatureError::Programming("notifications must be installed inside a tokio runtime".into()))?;

    let inner = Arc::new_cyclic(|weak| Inner {
        config,
        app: Arc::downgrade(&env.app),
        clock: env.edges.clock.clone(),
        notifier: env.notifier.clone(),
        stores: PhoneStores::new(env.stores.clone(), FEATURE, store::MIGRATIONS),
        messaging,
        app_directory: OnceLock::new(),
        runtime,
        phones: Mutex::new(HashMap::new()),
        selected: Mutex::new(None),
        settings_gate: tokio::sync::Mutex::new(()),
        events: broadcast::channel(256).0,
        shutdown: CancellationToken::new(),
        weak_self: weak.clone(),
        post_failing: OngoingFailure::default(),
        withdraw_failing: OngoingFailure::default(),
        unparsable_stored: OngoingFailure::default(),
    });

    env.app.register_processor_factory(&[wire::CONTENT_TYPE], Arc::new(inbound::BatchFactory::new(&inner)))?;
    env.app.register_connect_contributor(
        ConnectProvider::SyncNotifications,
        Arc::new(connect::Contributor::new(&inner)),
    )?;
    env.responses.register(FEATURE, Arc::new(responses::Responder::new(&inner))).map_err(RegistrationError::from)?;

    inner.spawn_event_watcher(&env.app);
    inner.observe_selection(&env.app);
    Ok(Arc::new(Notifications { inner }))
}

impl Notifications {
    /// Sets the phone's app list (once): the notification list shows an app's name from it when the
    /// app is known, and a banner carries the app's icon.
    pub fn set_app_directory(&self, directory: Arc<dyn PhoneAppDirectory>) -> Result<(), FeatureError> {
        self.inner
            .app_directory
            .set(directory)
            .map_err(|_| FeatureError::Programming("the notifications app directory is set once".into()))
    }

    /// Events telling the UI what to re-read.
    pub fn subscribe(&self) -> broadcast::Receiver<NotificationsEvent> {
        self.inner.events.subscribe()
    }

    /// The feature's state for the selected phone.
    pub fn status(&self) -> Result<NotificationsStatus, NotificationsError> {
        self.inner.status()
    }

    /// The notification list: newest first, media-style items hidden, an unpinned item hidden while
    /// its app's feed flag is off.
    pub fn list(&self) -> Result<Vec<NotificationView>, NotificationsError> {
        self.inner.list()
    }

    /// Every stored notification of `phone`, media-style items included, newest `postTime` first.
    pub fn stored_rows(&self, phone: &DcgClient) -> Result<Vec<store::StoredRow>, NotificationsError> {
        let db = self.inner.db(phone)?;
        Ok(store::all_rows(&db)?)
    }

    /// The notifications of `phone` have been synced at least once (a "last updated" time is stored).
    pub fn synced_once(&self, phone: &DcgClient) -> Result<bool, NotificationsError> {
        let db = self.inner.db(phone)?;
        Ok(store::stored_value(&db, stored::LAST_UPDATED_MS)?.is_some())
    }

    /// Dismisses one list item: its row is deleted and its system notification withdrawn, then
    /// operation 2 is sent; a silent permission check follows unless the answer was *needs
    /// permission*.
    pub async fn dismiss(&self, key: &str) -> Result<AppOutcome, NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.dismiss(key).await
    }

    /// Clears the list (its unpinned, non-media items): operation 4 with the cleared keys that are
    /// still stored — `Ok(None)` and nothing sent when none is — and, on *Success*, their rows,
    /// queued requests and system notifications removed.
    pub async fn clear_all(&self) -> Result<Option<AppOutcome>, NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.clear_all().await
    }

    /// Pins or unpins a list item (local only).
    pub fn set_pinned(&self, key: &str, pinned: bool) -> Result<(), NotificationsError> {
        self.inner.set_pinned(key, pinned)
    }

    /// Presses a non-reply action of a list item (queued operation 5).
    pub fn press_action(&self, key: &str, action_index: i32) -> Result<(), NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.press_action(key, action_index)
    }

    /// Sends a reply from the list's reply box (queued operation 5 with `inlineReplyMessage`).
    pub fn reply(&self, key: &str, action_index: i32, text: &str) -> Result<(), NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.list_reply(key, action_index, text)
    }

    /// The notification settings view opened: settings trigger 25 when the phone supports the
    /// per-app list (`Ok(None)` otherwise).
    pub async fn open_settings_view(&self) -> Result<Option<AppOutcome>, NotificationsError> {
        self.inner.open_settings_view().await
    }

    /// An app's setting as the UI shows it.
    pub fn app_setting(&self, package_name: &str) -> Result<AppSettingView, NotificationsError> {
        self.inner.app_setting(package_name)
    }

    /// A list item's context-menu on/off toggle for its app: settings trigger 26 with the new feed
    /// flag, then the local banner flag (None or All).
    pub async fn toggle_app_from_list(
        &self,
        package_name: &str,
        enabled: bool,
    ) -> Result<AppOutcome, NotificationsError> {
        let flags = if enabled { AppFlags::ALL } else { AppFlags::NONE };
        self.inner.update_app_setting(package_name, flags, wire::names::TRIGGER_CONTEXT_MENU_TOGGLE).await
    }

    /// The undo button of the "turned off for this app" notice: re-applies `previous` (trigger 26).
    pub async fn undo_app_turn_off(
        &self,
        package_name: &str,
        previous: AppFlags,
    ) -> Result<AppOutcome, NotificationsError> {
        self.inner.update_app_setting(package_name, previous, wire::names::TRIGGER_UNDO_TURN_OFF).await
    }

    /// A change in the settings list: trigger 26 only when the feed flag changed (full sync (d)
    /// when it ends *Unknown*), the local banner flag only after that succeeded.
    pub async fn change_app_setting(
        &self,
        package_name: &str,
        flags: AppFlags,
    ) -> Result<AppOutcome, NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.change_app_setting(package_name, flags).await
    }

    /// A banner-menu change: the app's local banner flag only, nothing sent.
    pub fn set_app_local_setting(&self, package_name: &str, flags: AppFlags) -> Result<(), NotificationsError> {
        self.inner.set_local_flags(package_name, flags)
    }

    /// A multi-app change of the setup / preferences dialog: trigger 27 when batching is available
    /// (quiet by default and `deviceCapabilityNegotiation` bit 8), else one trigger 26
    /// per app.
    pub async fn apply_preferences(&self, change: PreferencesChange) -> Result<PreferencesResult, NotificationsError> {
        self.inner.apply_preferences(change).await
    }

    /// The PC permission switch (default on); turning it on with the device permission on starts
    /// full sync (b).
    pub fn set_pc_permission(&self, on: bool) -> Result<(), NotificationsError> {
        self.inner.arc().ok_or(NotificationsError::AppGone)?.set_pc_permission(on)
    }

    /// Mutes an app's banners for 1 h, 8 h or 24 h (local only).
    pub fn mute_app(&self, package_name: &str, duration: MuteDuration) -> Result<(), NotificationsError> {
        let until =
            phonelink_core::time::ticks_from_unix_ms(self.inner.now_ms() + duration.duration().as_millis() as i64);
        self.inner.set_local_mute(package_name, until)
    }

    pub fn unmute_app(&self, package_name: &str) -> Result<(), NotificationsError> {
        self.inner.set_local_mute(package_name, 0)
    }

    /// Mute-all for 24 h, or off (local only).
    pub fn set_mute_all(&self, on: bool) -> Result<(), NotificationsError> {
        self.inner.set_mute_all(on)
    }

    /// The per-phone throttling opt-out (local only).
    pub fn set_throttling_enabled(&self, on: bool) -> Result<(), NotificationsError> {
        let (_, phone) = self.inner.selected()?;
        self.inner.set_throttling_enabled(&phone, on)
    }

    /// The grant-access prompt: `/permission` 64, not silent, user-initiated.
    pub async fn request_access(&self) -> Result<AppOutcome, NotificationsError> {
        self.inner.request_access().await
    }

    /// The sensitive-notification prompt (offered when `NotificationsCapabilities` has `8` and `4`):
    /// `/permission` `-2147483648`, not silent, user-initiated. The grant arrives later as a change of
    /// the sensitive-notification permission.
    pub async fn request_sensitive_access(&self) -> Result<AppOutcome, NotificationsError> {
        self.inner.request_sensitive_access().await
    }
}

/// Refuses a non-positive throttling value or a monitoring period longer than the cooldown.
fn validate_config(config: &NotificationsConfig) -> Result<(), FeatureError> {
    let positive = config.throttling_threshold > 0
        && !config.throttling_monitoring_period.is_zero()
        && !config.throttling_cooldown.is_zero();
    if !positive || config.throttling_monitoring_period > config.throttling_cooldown {
        return Err(FeatureError::Config("notification throttling parameters".into()));
    }
    if config.queue_expiry.is_zero() || config.dedupe_window.is_zero() {
        return Err(FeatureError::Config("notification queue expiry and dedupe window must be positive".into()));
    }
    Ok(())
}

fn throttle_config(config: &NotificationsConfig) -> ThrottleConfig {
    ThrottleConfig {
        threshold: config.throttling_threshold,
        monitoring_period: config.throttling_monitoring_period,
        cooldown: config.throttling_cooldown,
    }
}

#[cfg(test)]
mod tests;
