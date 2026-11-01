//! Contacts for the UI ([`crate::features::contacts`]).
//!
//! Contacts give messages their names and photos (through messaging's lookup), the new-message
//! composer its recipient suggestions, and the contacts view its list and cards. This object serves
//! those, the contact photos, PC consent and the phone-side permission prompt.

use std::sync::Arc;
use std::time::UNIX_EPOCH;

use crate::api::engine::Engine;
use crate::features::FeatureError;
use crate::features::contacts::wire::PhoneNumberType;
use crate::features::contacts::{
    CardField, CardNumber, ContactCard as FeatureContactCard, ContactNumber, Contacts, ContactsEvent, ContactsPrompt,
    FieldKind, ListedContact, PermissionPromptResult,
};
use crate::types::DcgClient;

use super::{FeatureApiError, RequestOutcome, core_of, features_of, selected_phone};

/// Which action the contacts state offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ContactsAccess {
    /// Contacts are available.
    Granted,
    /// "Use my phone's contacts" is off on this Mac.
    PcConsentRequired,
    /// The phone has not allowed contacts yet.
    DeviceConsentRequired,
    /// The phone denied contacts after they were set up once.
    DeviceConsentDenied,
}

impl From<ContactsPrompt> for ContactsAccess {
    fn from(p: ContactsPrompt) -> Self {
        match p {
            ContactsPrompt::None => ContactsAccess::Granted,
            ContactsPrompt::PcConsentRequired => ContactsAccess::PcConsentRequired,
            ContactsPrompt::DeviceConsentRequired => ContactsAccess::DeviceConsentRequired,
            ContactsPrompt::DeviceConsentDenied => ContactsAccess::DeviceConsentDenied,
        }
    }
}

/// Contacts' state for the selected phone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactsState {
    /// The phone supports contacts sync.
    pub capable: bool,
    /// "Use my phone's contacts" (default on).
    pub pc_consent: bool,
    pub access: ContactsAccess,
    pub up_to_date: bool,
    /// When contacts last finished syncing, Unix ms.
    pub last_updated_ms: Option<i64>,
}

/// What a phone number is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PhoneNumberKind {
    Mobile,
    Home,
    Work,
    WorkMobile,
    Main,
    School,
    Other,
    /// A label the user typed on the phone (see the suggestion's `label`).
    Custom,
    Unknown,
}

fn number_kind(kind: i32) -> PhoneNumberKind {
    match PhoneNumberType::from_i32(kind) {
        Some(PhoneNumberType::PhoneMobile) => PhoneNumberKind::Mobile,
        Some(PhoneNumberType::PhoneHome) => PhoneNumberKind::Home,
        Some(PhoneNumberType::PhoneWork) => PhoneNumberKind::Work,
        Some(PhoneNumberType::PhoneMobileWork) => PhoneNumberKind::WorkMobile,
        Some(PhoneNumberType::PhoneMain) => PhoneNumberKind::Main,
        Some(PhoneNumberType::PhoneSchool) => PhoneNumberKind::School,
        Some(PhoneNumberType::PhoneOther) => PhoneNumberKind::Other,
        Some(PhoneNumberType::PhoneCustom) => PhoneNumberKind::Custom,
        Some(PhoneNumberType::Unknown) | None => PhoneNumberKind::Unknown,
    }
}

/// One recipient suggestion: a contact's phone number.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactSuggestion {
    pub contact_id: i64,
    pub name: String,
    /// The number as the phone displays it.
    pub number: String,
    pub kind: PhoneNumberKind,
    /// The phone's custom label, when the kind is `Custom`.
    pub label: String,
    pub has_thumbnail: bool,
}

/// One contact of the contacts view's list.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactListItem {
    pub contact_id: i64,
    /// The display name, else the company, else the nickname; empty when the contact has none.
    pub name: String,
    pub has_thumbnail: bool,
    /// The contact's other text that the view's search reads: nickname, company, job title, notes,
    /// and its phone numbers (both forms), e-mail addresses, postal addresses, URLs and dates.
    pub search_terms: Vec<String>,
}

impl From<ListedContact> for ContactListItem {
    fn from(c: ListedContact) -> Self {
        Self { contact_id: c.id, name: c.name, has_thumbnail: c.has_thumbnail, search_terms: c.terms }
    }
}

/// The label of a field of a contact card.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ContactFieldLabel {
    Home,
    Mobile,
    Work,
    WorkMobile,
    Main,
    School,
    Other,
    Birthday,
    Anniversary,
    /// A label the user typed on the phone (never empty).
    Custom {
        label: String,
    },
    /// The field's generic label: "Phone", "Email", "Address", "Homepage" or "Date".
    Generic,
}

impl From<FieldKind> for ContactFieldLabel {
    fn from(k: FieldKind) -> Self {
        match k {
            FieldKind::Home => Self::Home,
            FieldKind::Mobile => Self::Mobile,
            FieldKind::Work => Self::Work,
            FieldKind::WorkMobile => Self::WorkMobile,
            FieldKind::Main => Self::Main,
            FieldKind::School => Self::School,
            FieldKind::Other => Self::Other,
            FieldKind::Birthday => Self::Birthday,
            FieldKind::Anniversary => Self::Anniversary,
            FieldKind::Custom(label) => Self::Custom { label },
            FieldKind::Generic => Self::Generic,
        }
    }
}

/// A phone number of a contact card.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactCardNumber {
    /// The number as stored: the address a message to it goes to.
    pub number: String,
    /// The number as the view shows it.
    pub shown: String,
    pub label: ContactFieldLabel,
}

/// An e-mail address, postal address, URL or date of a contact card.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactCardField {
    pub value: String,
    pub label: ContactFieldLabel,
}

impl From<CardField> for ContactCardField {
    fn from(f: CardField) -> Self {
        Self { value: f.value, label: f.kind.into() }
    }
}

impl From<CardNumber> for ContactCardNumber {
    fn from(n: CardNumber) -> Self {
        Self { number: n.number, shown: n.shown, label: n.kind.into() }
    }
}

/// One contact as the contacts view's detail shows it; each list in the phone's record order.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactCard {
    pub contact_id: i64,
    /// As [`ContactListItem::name`].
    pub name: String,
    pub nickname: String,
    pub company: String,
    pub job_title: String,
    pub notes: String,
    pub has_thumbnail: bool,
    pub phone_numbers: Vec<ContactCardNumber>,
    pub email_addresses: Vec<ContactCardField>,
    pub postal_addresses: Vec<ContactCardField>,
    pub urls: Vec<ContactCardField>,
    pub dates: Vec<ContactCardField>,
}

impl From<FeatureContactCard> for ContactCard {
    fn from(c: FeatureContactCard) -> Self {
        fn all<A, B: From<A>>(v: Vec<A>) -> Vec<B> {
            v.into_iter().map(Into::into).collect()
        }

        Self {
            contact_id: c.id,
            name: c.name,
            nickname: c.nickname,
            company: c.company,
            job_title: c.job_title,
            notes: c.notes,
            has_thumbnail: c.has_thumbnail,
            phone_numbers: all(c.phone_numbers),
            email_addresses: all(c.email_addresses),
            postal_addresses: all(c.postal_addresses),
            urls: all(c.urls),
            dates: all(c.dates),
        }
    }
}

/// The outcome of the phone-side contacts permission prompt.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ContactsPermissionResult {
    pub outcome: RequestOutcome,
    /// The phone did not answer in time; the permission is unchanged.
    pub timed_out: bool,
    /// The contacts sync that followed a granted permission.
    pub sync: Option<RequestOutcome>,
}

impl From<PermissionPromptResult> for ContactsPermissionResult {
    fn from(r: PermissionPromptResult) -> Self {
        Self { outcome: r.outcome.into(), timed_out: r.timed_out, sync: r.sync.map(Into::into) }
    }
}

/// A change to contacts: re-read the contacts, or re-read [`ContactsApi::state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ContactsChange {
    /// Contacts changed: re-read suggestions and photos.
    RecordsChanged,
    /// What `state` reports changed.
    StateChanged,
}

impl From<&ContactsEvent> for ContactsChange {
    fn from(e: &ContactsEvent) -> Self {
        match e {
            ContactsEvent::RecordsChanged { .. } => ContactsChange::RecordsChanged,
            ContactsEvent::SyncStateChanged { .. } | ContactsEvent::PromptChanged { .. } => {
                ContactsChange::StateChanged
            }
        }
    }
}

/// The digits of `s` (what a number query matches on).
fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// The suggestions for `query` from `numbers` (in their order): every number of a contact whose name
/// contains the query, and every number containing the query's digits; at most `limit`.
fn suggest(numbers: Vec<ContactNumber>, query: &str, limit: usize) -> Vec<ContactSuggestion> {
    let needle = query.trim().to_lowercase();
    let needle_digits = digits(&needle);

    numbers
        .into_iter()
        .filter(|n| {
            n.display_name.to_lowercase().contains(&needle)
                || (!needle_digits.is_empty() && digits(&n.number).contains(&needle_digits))
        })
        .take(limit)
        .map(|n| ContactSuggestion {
            contact_id: n.contact_id,
            number: if n.display_number.is_empty() { n.number } else { n.display_number },
            name: n.display_name,
            kind: number_kind(n.kind),
            label: n.label,
            has_thumbnail: n.has_thumbnail,
        })
        .collect()
}

/// Contacts of the selected phone.
#[derive(uniffi::Object)]
pub struct ContactsApi {
    engine: Arc<Engine>,
}

impl ContactsApi {
    pub(crate) fn new(engine: Arc<Engine>) -> Arc<Self> {
        Arc::new(Self { engine })
    }

    fn run<T>(&self, f: impl FnOnce(&Contacts, &DcgClient) -> Result<T, FeatureError>) -> Result<T, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let _runtime = core.enter_runtime();
        let feature = features_of(&core)?.contacts.clone();
        let phone = selected_phone(&core)?;
        Ok(f(&feature, &phone)?)
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl ContactsApi {
    /// The selected phone's contacts setting, its contacts access, and how fresh the sync is.
    ///
    /// Fails with `NoPhone` when no phone is selected.
    pub fn state(&self) -> Result<ContactsState, FeatureApiError> {
        self.run(|f, phone| {
            let permission = f.permission_state(phone)?;
            let sync = f.sync_state(phone)?;

            Ok(ContactsState {
                capable: permission.contacts_capable,
                pc_consent: permission.pc_consent,
                access: f.prompt(phone)?.into(),
                up_to_date: sync.up_to_date,
                last_updated_ms: sync.last_updated.map(|t| match t.duration_since(UNIX_EPOCH) {
                    Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
                    Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
                }),
            })
        })
    }

    /// Phone numbers of contacts whose name contains `query` (case-insensitively), or whose number
    /// contains the query's digits; at most `limit`, in the list's order (by name). None while the
    /// access is not granted (`ContactsAccess::Granted`: PC consent on and the phone allowing
    /// contacts; differs from Phone Link on Windows).
    pub fn suggestions(&self, query: String, limit: u32) -> Result<Vec<ContactSuggestion>, FeatureApiError> {
        self.run(|f, phone| Ok(suggest(f.suggestion_numbers(phone)?, &query, limit as usize)))
    }

    /// The contacts view's list: every contact, by display name then id. None while the access is not
    /// granted (`ContactsAccess::Granted`): the view shows its prompt instead.
    pub fn list(&self) -> Result<Vec<ContactListItem>, FeatureApiError> {
        self.run(|f, phone| Ok(f.listed_contacts(phone)?.into_iter().map(Into::into).collect()))
    }

    /// One contact's card for the contacts view; `None` when it is gone, or while the access is not
    /// granted.
    pub fn card(&self, contact_id: i64) -> Result<Option<ContactCard>, FeatureApiError> {
        self.run(|f, phone| Ok(f.card(phone, contact_id)?.map(Into::into)))
    }

    /// A contact's photo.
    pub fn thumbnail(&self, contact_id: i64) -> Result<Option<Vec<u8>>, FeatureApiError> {
        self.run(|f, phone| f.thumbnail(phone, contact_id))
    }

    /// "Use my phone's contacts".
    pub fn set_pc_consent(&self, consent: bool) -> Result<(), FeatureApiError> {
        self.run(|f, phone| f.set_pc_consent(phone, consent))
    }

    /// Asks the phone to allow contacts; a grant is followed by a contacts sync. Only for a phone the
    /// state calls `capable` (refused otherwise): the composer's and Settings' prompts ask for
    /// contacts access, as does the contacts view.
    pub async fn request_permission(&self) -> Result<ContactsPermissionResult, FeatureApiError> {
        let core = core_of(&self.engine)?;
        let feature = features_of(&core)?.contacts.clone();
        Ok(feature.request_device_permission().await?.into())
    }
}

#[cfg(test)]
mod tests;
