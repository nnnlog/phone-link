//! Contacts, the Android path.
//!
//! It covers the phone mirrors its address book onto the PC through `contactsV2` content on
//! `/legacy/phonecontent`; the PC asks for a sync with `/contacts/v2`, asks for the contacts
//! permission with `/permission` 2048, and contributes sequence numbers to the connect message.
//!
//! Sync is one-way, phone → PC.
//!
//! | Module | Implements |
//! |---|---|
//! | [`wire`] | the message catalog, every key name, the type enums, the outbound `/contacts/v2` sync request |
//! | [`parse`] | validation of inbound `contactsV2`: metadata, items (the batch rules, the `contacts` batch, the sub-record batches), inbound error handling |
//! | [`store`] | the local state the protocol depends on; reconciliation; deletes before upserts; the writes of completion |
//! | [`session`] | the `contactsV2` processor: reconciliation, both responses (PC → phone), sessions, completion |
//! | [`connect`] | the contribution to the connect message |
//! | [`lookup`] | the local lookup by address — the app's single rule |
//! | [`model`] | what the UI shows: list, detail, the up-to-date state, the contacts-view prompt, the birthday banner |
//! | [`view`] | the contacts view: the list with the text its search reads, a contact's card with its field labels |
//!
//! This module installs the feature and holds the requests: `/contacts/v2` with its outcome table,
//! the contacts-view `/permission` flow, which the app's own prompts send for a contacts-capable
//! phone (Phone Link on Windows asks from its contacts view), and the triggers —
//! there is no startup pull; `/contacts/v2` is sent only when a view asks
//! ([`ContactDirectory::request_sync`], [`Contacts::sync`],
//! [`Contacts::request_device_permission`]). Inbound `contactsV2` is processed for every phone,
//! whatever its capability, permission or consent. Synced contacts are not copied into the Mac's
//! contacts.

#![cfg_attr(feature = "test-support", warn(unreachable_pub))]

pub mod connect;
pub mod lookup;
pub mod model;
pub mod parse;
pub mod session;
pub mod store;
pub mod view;
pub mod wire;

use phonelink_core::config::features::ContactsConfig;
use std::sync::{Arc, Weak};

use tokio::runtime::Handle;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

use phonelink_app::consts::{capability, permission};
use phonelink_app::{AppEvent, AppLayer, AppOutcome, ConnectProvider, PhoneRecord, RequestOptions};
use phonelink_core::edge::{Clock, HostInfo};
use phonelink_core::types::{DcgClient, TraceContext};

use phonelink_features::shared::db::{PhoneStores, StoreError};
use phonelink_features::shared::ongoing::OngoingFailure;
use phonelink_features::shared::phone_number::{phone_country, region_from_code};
use phonelink_features::{ContactDirectory, ContactMatch, FeatureEnv, FeatureError};

pub use model::{
    BirthdayBanner, CivilDate, ContactDetail, ContactNumber, ContactSummary, ContactsPrompt, PermissionPromptResult,
    PermissionState, SyncState, birthday_banner,
};
pub use view::{CardField, CardNumber, ContactCard, FieldKind, ListedContact};
#[cfg(feature = "test-support")]
pub use wire::RecordType;

use connect::ContactsConnect;
use lookup::LookupCache;
use session::ContactsFactory;
use store::ContactsStore;

/// `ypScenarioId` of the sync request and of the contacts permission request (free-form; Phone
/// Link's names).
const SCENARIO_SYNC: &str = "SyncAllContacts";
const SCENARIO_PERMISSION: &str = "ContactsDevicePermission";
/// `ypTriggerId` of the contacts view's permission action and of the syncs other features request.
const TRIGGER_PROMPT: &str = "ErrorActionInvoked";
const TRIGGER_DIRECTORY: &str = "ContactsSyncRequested";

/// What tells the UI to re-read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::enum_variant_names, reason = "each event names what changed")]
pub enum ContactsEvent {
    /// A metadata or item message changed `phone`'s records (or they were cleared).
    RecordsChanged { phone: DcgClient },
    /// `phone`'s sequence numbers or last-updated time changed: re-read [`Contacts::sync_state`].
    SyncStateChanged { phone: DcgClient },
    /// An input of the contacts-view prompt changed — PC consent, "set up once", the contacts device
    /// permission, the phone's contacts capability: re-read [`Contacts::prompt`].
    PromptChanged { phone: DcgClient },
}

/// State shared by the handle, the processor factory, the connect provider and spawned requests.
/// Holds the App layer weakly: the App layer's registries hold the factory and the provider.
pub(crate) struct Shared {
    config: ContactsConfig,
    stores: PhoneStores,
    app: Weak<AppLayer>,
    clock: Arc<dyn Clock>,
    host: Arc<dyn HostInfo>,
    runtime: Handle,
    events: broadcast::Sender<ContactsEvent>,
    lookup: LookupCache,
    /// The lookups for other features (one per message or call shown) cannot read the store.
    lookup_failing: OngoingFailure,
}

impl Shared {
    pub(crate) fn store(&self, phone: &DcgClient) -> Result<ContactsStore, StoreError> {
        Ok(ContactsStore::new(self.stores.get(phone)?))
    }

    pub(crate) fn now_ms(&self) -> i64 {
        phonelink_core::time::unix_ms(self.clock.now())
    }

    pub(crate) fn emit(&self, event: ContactsEvent) {
        // No subscriber is not an error.
        let _ = self.events.send(event);
    }

    /// After a committed change of `phone`'s records.
    pub(crate) fn records_changed(&self, phone: &DcgClient) {
        self.lookup.invalidate(phone);
        self.emit(ContactsEvent::RecordsChanged { phone: phone.clone() });
    }

    fn app(&self) -> Result<Arc<AppLayer>, FeatureError> {
        self.app.upgrade().ok_or_else(|| FeatureError::Programming("the App layer is gone".into()))
    }

    /// `phone`'s record when it is the selected phone.
    fn record_for(&self, phone: &DcgClient) -> Option<PhoneRecord> {
        self.app.upgrade()?.phone_record().filter(|r| r.phone().is_same_client(phone))
    }

    /// The phone's country: the record's country code, else the PC's region.
    fn phone_country(&self, phone: &DcgClient) -> Option<String> {
        let pc_region = self.host.region_code();
        match self.record_for(phone) {
            Some(record) => phone_country(&record, pc_region.as_deref()),
            None => pc_region.as_deref().and_then(region_from_code),
        }
    }

    fn lookup(
        &self,
        phone: &DcgClient,
        address: &str,
        region: Option<&str>,
    ) -> Result<Option<ContactMatch>, StoreError> {
        let store = self.store(phone)?;
        let candidates = self.lookup.candidates(&store, phone, self.phone_country(phone), address, region)?;
        // By contact id while the birthday flag is on, by display name while it is off.
        lookup::build_match(&store, &candidates, !self.config.birthday_text)
    }

    /// The outcome table of `/contacts/v2` and of the contacts `/permission` request: success and
    /// accepted grant the contacts device permission and mark contacts set up once; denied,
    /// permanently denied and needs permission revoke it; any other outcome changes nothing.
    fn apply_outcome(&self, app: &AppLayer, phone: &DcgClient, outcome: &AppOutcome) {
        match outcome {
            AppOutcome::Success | AppOutcome::Accepted => {
                app.update_record_for(phone, |r| r.set_permission(permission::CONTACTS, true));
                match self.store(phone).and_then(|s| s.set_setup_completed_once()) {
                    Ok(true) => self.emit(ContactsEvent::PromptChanged { phone: phone.clone() }),
                    Ok(false) => {}
                    Err(e) => error!(error = %e, "contacts set-up-once flag not stored; the prompt may show again"),
                }
            }
            AppOutcome::Denied | AppOutcome::PermanentlyDenied | AppOutcome::NeedsPermission => {
                app.update_record_for(phone, |r| r.set_permission(permission::CONTACTS, false));
            }
            _ => {}
        }
    }

    /// Sends `/contacts/v2` to the selected phone (one attempt) and applies its outcome.
    async fn send_sync(
        &self,
        app: &AppLayer,
        user_initiated: bool,
        trace: TraceContext,
        trigger: &str,
    ) -> Result<AppOutcome, FeatureError> {
        let phone = app.selected_phone();
        let options = if user_initiated { RequestOptions::user_initiated() } else { RequestOptions::default() }
            .with_timeout(self.config.request_timeout)
            .named(trigger, SCENARIO_SYNC);

        let response = app.request(wire::sync_request(trace, options)).await?;
        info!(user_initiated, outcome = ?response.outcome, "/contacts/v2 request finished");

        if let Some(phone) = phone {
            self.apply_outcome(app, &phone, &response.outcome);
        }
        Ok(response.outcome)
    }
}

/// The contacts feature handle.
pub struct Contacts {
    shared: Arc<Shared>,
}

impl Contacts {
    /// Registers the `contactsV2` processor factory and the connect provider, and watches the App
    /// layer's events for the prompt's inputs. Must run inside a tokio runtime.
    pub fn install(env: &FeatureEnv, config: ContactsConfig) -> Result<Arc<Self>, FeatureError> {
        let runtime = Handle::try_current()
            .map_err(|_| FeatureError::Programming("contacts must be installed inside a tokio runtime".into()))?;

        let shared = Arc::new(Shared {
            config,
            stores: PhoneStores::new(env.stores.clone(), store::FEATURE, store::MIGRATIONS),
            app: Arc::downgrade(&env.app),
            clock: env.edges.clock.clone(),
            host: env.edges.host.clone(),
            runtime: runtime.clone(),
            events: broadcast::channel(64).0,
            lookup: LookupCache::new(),
            lookup_failing: OngoingFailure::default(),
        });

        env.app
            .register_processor_factory(&[wire::CONTENT_TYPE], Arc::new(ContactsFactory { shared: shared.clone() }))?;
        env.app.register_connect_contributor(
            ConnectProvider::Contacts,
            Arc::new(ContactsConnect { shared: shared.clone() }),
        )?;
        spawn_prompt_watch(&runtime, &shared, &env.app);
        Ok(Arc::new(Self { shared }))
    }

    /// This feature as the [`ContactDirectory`] other features read.
    pub fn directory(self: &Arc<Self>) -> Arc<dyn ContactDirectory> {
        self.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ContactsEvent> {
        self.shared.events.subscribe()
    }

    pub fn config(&self) -> &ContactsConfig {
        &self.shared.config
    }

    // --- views --------------------------------------------------------------------------------------

    /// `phone`'s contacts, by display name then id.
    pub fn contacts(&self, phone: &DcgClient) -> Result<Vec<ContactSummary>, FeatureError> {
        let rows = self.shared.store(phone)?.contact_list()?;
        Ok(rows
            .into_iter()
            .map(|(id, display_name, has_thumbnail)| ContactSummary { id, display_name, has_thumbnail })
            .collect())
    }

    /// Every phone number of `phone`'s contacts with the contact's name, in the list's order (by
    /// display name, then contact id), then by number id. No thumbnail bytes or other details are
    /// read.
    pub fn contact_numbers(&self, phone: &DcgClient) -> Result<Vec<ContactNumber>, FeatureError> {
        Ok(self.shared.store(phone)?.contact_numbers()?)
    }

    /// What the composer's recipient suggestions search: [`Contacts::contact_numbers`] while the
    /// access is granted — PC consent on and the contacts device permission `true` (the prompt is
    /// [`ContactsPrompt::None`]) — else none (differs from Phone Link on Windows).
    pub fn suggestion_numbers(&self, phone: &DcgClient) -> Result<Vec<ContactNumber>, FeatureError> {
        if self.prompt(phone)? != ContactsPrompt::None {
            return Ok(Vec::new());
        }
        self.contact_numbers(phone)
    }

    /// One contact with its sub-records.
    pub fn contact(&self, phone: &DcgClient, id: i64) -> Result<Option<ContactDetail>, FeatureError> {
        let store = self.shared.store(phone)?;
        let Some(contact) = store.contact(id)? else {
            return Ok(None);
        };

        Ok(Some(ContactDetail {
            contact,
            phone_numbers: store.phone_numbers(id)?,
            email_addresses: store.email_addresses(id)?,
            postal_addresses: store.postal_addresses(id)?,
            dates: store.contact_dates(id)?,
            urls: store.contact_urls(id)?,
        }))
    }

    /// The contacts view's list ([`ListedContact`]), by display name then id; none while the view's
    /// prompt stands in for it (the prompt is not [`ContactsPrompt::None`]).
    pub fn listed_contacts(&self, phone: &DcgClient) -> Result<Vec<ListedContact>, FeatureError> {
        if self.prompt(phone)? != ContactsPrompt::None {
            return Ok(Vec::new());
        }
        Ok(self.shared.store(phone)?.listed_contacts()?)
    }

    /// One contact's card for the contacts view; `None` when it is not stored, or while the view's
    /// prompt stands in for the view.
    pub fn card(&self, phone: &DcgClient, id: i64) -> Result<Option<ContactCard>, FeatureError> {
        if self.prompt(phone)? != ContactsPrompt::None {
            return Ok(None);
        }
        Ok(self.contact(phone, id)?.map(ContactCard::from_detail))
    }

    /// A contact's thumbnail bytes (opaque image bytes, decoded only for display).
    pub fn thumbnail(&self, phone: &DcgClient, id: i64) -> Result<Option<Vec<u8>>, FeatureError> {
        Ok(self.shared.store(phone)?.thumbnail(id)?)
    }

    /// The sequence numbers, the up-to-date state and the last-updated time.
    pub fn sync_state(&self, phone: &DcgClient) -> Result<SyncState, FeatureError> {
        let store = self.shared.store(phone)?;
        let stored = store.sequence_numbers()?;
        let state = store.state()?;
        Ok(SyncState {
            current: stored.map(|s| s.map_or(-1, |s| s.current)),
            expected: stored.map(|s| s.map_or(-1, |s| s.expected)),
            up_to_date: stored.iter().flatten().all(|s| s.current >= s.expected),
            last_updated: state.last_updated_ms.and_then(phonelink_core::time::checked_system_time),
        })
    }

    /// The permission, consent and capability state the contacts view and the calls view read.
    pub fn permission_state(&self, phone: &DcgClient) -> Result<PermissionState, FeatureError> {
        let state = self.shared.store(phone)?.state()?;
        let record = self.shared.record_for(phone);
        Ok(PermissionState {
            device_permission: record.as_ref().is_none_or(|r| r.permission_or(permission::CONTACTS, true)),
            pc_consent: state.pc_consent,
            setup_completed_once: state.setup_completed_once,
            contacts_capable: record.as_ref().is_some_and(|r| r.has_capability(capability::CONTACTS)),
            contacts_capabilities: record.and_then(|r| r.contacts_capabilities).unwrap_or(0),
        })
    }

    /// The contacts view's prompt.
    pub fn prompt(&self, phone: &DcgClient) -> Result<ContactsPrompt, FeatureError> {
        let s = self.permission_state(phone)?;
        Ok(ContactsPrompt::from_state(s.pc_consent, s.device_permission, s.setup_completed_once))
    }

    /// The contacts view's "give consent" action and the settings toggle.
    pub fn set_pc_consent(&self, phone: &DcgClient, consent: bool) -> Result<(), FeatureError> {
        if self.shared.store(phone)?.set_pc_consent(consent)? {
            info!(enabled = consent, "contacts consent on this Mac switched");
            self.shared.emit(ContactsEvent::PromptChanged { phone: phone.clone() });
        }
        Ok(())
    }

    /// The birthday banner for `contact_id` (its first `Birthday` date) on `today`, the UI's local
    /// date; [`BirthdayBanner::None`] while the birthday flag is off.
    pub fn birthday_banner(
        &self,
        phone: &DcgClient,
        contact_id: i64,
        today: CivilDate,
    ) -> Result<BirthdayBanner, FeatureError> {
        if !self.shared.config.birthday_text {
            return Ok(BirthdayBanner::None);
        }

        let Some(detail) = self.contact(phone, contact_id)? else {
            return Ok(BirthdayBanner::None);
        };
        Ok(detail.birthday().map_or(BirthdayBanner::None, |b| birthday_banner(b, today)))
    }

    // --- actions ------------------------------------------------------------------------------------

    /// The "allow access on your phone" action of the app's prompts (Phone Link on Windows offers it
    /// in its contacts view): `/permission` with `permissionType` 2048 and `silent: false`,
    /// user-initiated, one attempt; its outcome applied as `/contacts/v2`'s; then, when the contacts
    /// device permission is `true`, `/contacts/v2` with `isuserinitiated: false`. Only for a
    /// contacts-capable phone; a timeout changes only the prompt
    /// ([`PermissionPromptResult::timed_out`]).
    pub async fn request_device_permission(&self) -> Result<PermissionPromptResult, FeatureError> {
        let app = self.shared.app()?;
        let phone = app.selected_phone().ok_or_else(|| FeatureError::Programming("no phone is selected".into()))?;
        if !self.shared.record_for(&phone).is_some_and(|r| r.has_capability(capability::CONTACTS)) {
            return Err(FeatureError::Programming("the selected phone is not contacts-capable".into()));
        }

        let options = RequestOptions::user_initiated()
            .with_timeout(self.shared.config.request_timeout)
            .named(TRIGGER_PROMPT, SCENARIO_PERMISSION);
        let request = app.permission_request(permission::CONTACTS, false, options);
        let trace = request.trace.clone().unwrap_or_else(|| TraceContext::new_root(Vec::new()));
        let response = app.request(request).await?;
        info!(outcome = ?response.outcome, "contacts /permission request finished");
        self.shared.apply_outcome(&app, &phone, &response.outcome);

        let granted = self.shared.record_for(&phone).is_some_and(|r| r.permission_or(permission::CONTACTS, true));
        let sync =
            if granted { Some(self.shared.send_sync(&app, false, trace.child(), TRIGGER_PROMPT).await?) } else { None };

        Ok(PermissionPromptResult {
            timed_out: response.outcome == AppOutcome::TimedOut,
            outcome: response.outcome,
            sync,
        })
    }

    /// Sends `/contacts/v2` now and waits for its outcome (applied as the outcome table says) — the
    /// views' triggers: messaging after its permission prompt (`user_initiated: true`), the calls view
    /// after a call-log refresh (`false`). `trigger` names the trigger in the trace state.
    pub async fn sync(&self, user_initiated: bool, trigger: &str) -> Result<AppOutcome, FeatureError> {
        let app = self.shared.app()?;
        self.shared.send_sync(&app, user_initiated, TraceContext::new_root(Vec::new()), trigger).await
    }

    /// Deletes `phone`'s records and resets its sequence numbers to `-1` (clearing the records
    /// resets them, as a new store holds).
    pub fn clear(&self, phone: &DcgClient) -> Result<(), FeatureError> {
        self.shared.store(phone)?.clear()?;
        self.shared.records_changed(phone);
        self.shared.emit(ContactsEvent::SyncStateChanged { phone: phone.clone() });
        Ok(())
    }
}

impl ContactDirectory for Contacts {
    fn lookup(&self, phone: &DcgClient, address: &str, region: Option<&str>) -> Option<ContactMatch> {
        match self.shared.lookup(phone, address, region) {
            Ok(found) => {
                self.shared.lookup_failing.clear();
                found
            }
            Err(e) => {
                if self.shared.lookup_failing.starts() {
                    error!(error = %e, "contacts store not read; other features show numbers without contact names");
                }
                None
            }
        }
    }

    fn sync_allowed(&self, phone: &DcgClient) -> bool {
        match self.prompt(phone) {
            Ok(prompt) => prompt == ContactsPrompt::None,
            Err(e) => {
                error!(error = %e, "contacts state not read; no contacts sync follows");
                false
            }
        }
    }

    fn request_sync(&self, user_initiated: bool) {
        let shared = self.shared.clone();
        self.shared.runtime.spawn(async move {
            let Some(app) = shared.app.upgrade() else {
                debug!("/contacts/v2 not sent; the App layer is gone");
                return;
            };

            // `send_sync` logs the outcome.
            if let Err(e) =
                shared.send_sync(&app, user_initiated, TraceContext::new_root(Vec::new()), TRIGGER_DIRECTORY).await
            {
                warn!(error = %e, "/contacts/v2 not sent");
            }
        });
    }
}

/// Emits [`ContactsEvent::PromptChanged`] when the selected phone's contacts device permission or
/// contacts capability changes (both live in the App layer's phone record).
fn spawn_prompt_watch(runtime: &Handle, shared: &Arc<Shared>, app: &Arc<AppLayer>) {
    let mut events = app.subscribe_events();
    let shared = Arc::downgrade(shared);
    let app = Arc::downgrade(app);

    runtime.spawn(async move {
        let mut last: Option<(DcgClient, bool, bool)> = None;
        loop {
            let phone = match events.recv().await {
                Ok(AppEvent::PermissionsChanged { phone, .. } | AppEvent::PhoneRecordChanged { phone }) => phone,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let (Some(shared), Some(app)) = (shared.upgrade(), app.upgrade()) else {
                break;
            };
            let Some(record) = app.phone_record().filter(|r| r.phone().is_same_client(&phone)) else {
                continue;
            };

            let now = (
                phone.clone(),
                record.has_capability(capability::CONTACTS),
                record.permission_or(permission::CONTACTS, true),
            );
            if last.as_ref() != Some(&now) {
                last = Some(now);
                shared.emit(ContactsEvent::PromptChanged { phone });
            }
        }
    });
}
