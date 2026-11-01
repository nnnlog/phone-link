//! The contacts wire vocabulary.
//!
//! It covers the `contentType` and route, the six record types with every key name they use
//! (inbound sync, the `/contacts/v2` request, the connect message), the enums, and the
//! `/contacts/v2` request builder.

use phonelink_app::{AppRequest, RequestOptions};
use phonelink_core::codec::valueset::{Value, ValueSet};
use phonelink_core::types::TraceContext;

/// The `contentType` of every contacts message, inbound and outbound.
pub const CONTENT_TYPE: &str = "contactsV2";
/// The sync request's route.
pub const SYNC_ROUTE: &str = "/contacts/v2";

/// `subcontentTypes` of the sync request, in this order.
pub const SUBCONTENT_TYPES: [&str; 7] =
    ["contactsV2", "contactThumbs", "phoneNumbers", "emailAddresses", "postalAddresses", "contactDates", "contactUrls"];

/// The two `configuration` flags of the sync request and of the connect contribution.
pub const CONFIG_CONTACTS_V2_SYNC: &str = "ContactsV2Sync";
pub const CONFIG_CONTACT_THUMBNAILS: &str = "ContactThumbnails";

/// Top-level keys of the inbound messages and the metadata response.
pub mod key {
    pub const CORRELATION_VECTOR: &str = "correlationVector";
    pub const CONTENT_TYPE: &str = "contentType";
    pub const CONTRACT_VERSION: &str = "contractVersion";
    pub const SUBCONTENT_TYPES: &str = "subcontentTypes";
    pub const CONFIGURATION: &str = "configuration";
    pub const ITEM_COUNT: &str = "itemCount";
    pub const RESULT: &str = "result";
    pub const CONTACT_SETTINGS: &str = "contactSettings";
    pub const CONTACT_CAPABILITIES: &str = "contactCapabilities";
    pub const SEQUENCE_NUMBERS: &str = "sequenceNumbers";
    pub const PERMISSIONS: &str = "permissions";
    /// `permissions.contacts` of the connect response.
    pub const PERMISSIONS_CONTACTS: &str = "contacts";
    /// Shared by every batch: the per-entry `SyncAction`s and checksums.
    pub const ACTIONS: &str = "actions";
    pub const CHECKSUMS: &str = "checksums";
    /// The owning contact id of every sub-record batch.
    pub const CONTACT_IDS: &str = "contactIds";
}

/// Keys of the `contacts` batch.
pub mod contact_key {
    pub const CONTACT_IDS: &str = "contactIds";
    pub const LAST_UPDATED_TIMES: &str = "lastUpdatedTimes";
    pub const DISPLAY_NAMES: &str = "displayNames";
    pub const NICKNAMES: &str = "nicknames";
    pub const COMPANIES: &str = "companies";
    pub const JOB_TITLES: &str = "jobTitles";
    pub const NOTES: &str = "notes";
    pub const NAME_PREFIXES: &str = "namePrefixes";
    pub const NAME_SUFFIXES: &str = "nameSuffixes";
    pub const GIVEN_NAMES: &str = "givenNames";
    pub const MIDDLE_NAMES: &str = "middleNames";
    pub const FAMILY_NAMES: &str = "familyNames";
    pub const THUMBNAIL_SIZES: &str = "thumbnailSizes";
    /// Not a per-entry array: one blob (*`contacts` batch*).
    pub const THUMBNAILS: &str = "thumbnails";
}

/// Keys of the `phoneNumbers` batch.
pub mod phone_key {
    pub const PHONE_NUMBER_IDS: &str = "phoneNumberIds";
    pub const PHONE_NUMBERS: &str = "phoneNumbers";
    pub const DISPLAY_NUMBERS: &str = "displayNumbers";
    pub const TYPES: &str = "types";
    pub const LABELS: &str = "labels";
}

/// Keys of the `emailAddresses` batch.
pub mod email_key {
    pub const IDS: &str = "contactEmailAddressIds";
    pub const ADDRESSES: &str = "emailAddresses";
    pub const TYPE: &str = "emailAddressType";
    pub const LABEL: &str = "emailAddressLabel";
}

/// Keys of the `postalAddresses` batch.
pub mod postal_key {
    pub const IDS: &str = "contactPostalAddressIds";
    pub const TYPE: &str = "postalAddressType";
    pub const LABEL: &str = "postalAddressLabel";
    pub const DISPLAY_ADDRESS: &str = "postalAddressDisplayAddress";
    pub const STREET: &str = "postalAddressStreet";
    pub const CITY: &str = "postalAddressCity";
    pub const REGION: &str = "postalAddressRegion";
    pub const POSTAL_CODE: &str = "postalAddressPostalCode";
    pub const COUNTRY_CODE: &str = "postalAddressCountryCode";
    pub const SUB_ADMINISTRATIVE_AREA: &str = "postalAddressSubAdministrativeArea";
    pub const SUB_LOCALITY: &str = "postalAddressSubLocality";
}

/// Keys of the `contactDates` batch.
pub mod date_key {
    pub const IDS: &str = "contactDateIds";
    pub const TYPE: &str = "type";
    pub const LABEL: &str = "label";
    pub const DISPLAY_DATE: &str = "displayDate";
}

/// Keys of the `contactUrls` batch.
pub mod url_key {
    pub const IDS: &str = "contactUrlIds";
    pub const TYPE: &str = "type";
    pub const LABEL: &str = "label";
    pub const ADDRESS: &str = "address";
}

/// The six record types, in the order the PC processes them during reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RecordType {
    Contact,
    PhoneNumber,
    EmailAddress,
    PostalAddress,
    ContactDate,
    ContactUrl,
}

impl RecordType {
    /// Processing order.
    pub const ALL: [RecordType; 6] = [
        RecordType::Contact,
        RecordType::PhoneNumber,
        RecordType::EmailAddress,
        RecordType::PostalAddress,
        RecordType::ContactDate,
        RecordType::ContactUrl,
    ];

    /// Index into per-type arrays (processing order).
    pub fn index(self) -> usize {
        self as usize
    }

    /// The id array of the metadata message and of the metadata response.
    pub fn ids_key(self) -> &'static str {
        match self {
            RecordType::Contact => "contactIds",
            RecordType::PhoneNumber => "phoneNumberIds",
            RecordType::EmailAddress => "contactEmailAddressIds",
            RecordType::PostalAddress => "contactPostalAddressIds",
            RecordType::ContactDate => "contactDateIds",
            RecordType::ContactUrl => "contactUrlIds",
        }
    }

    /// The checksum array of the metadata message.
    pub fn checksums_key(self) -> &'static str {
        match self {
            RecordType::Contact => "contactChecksums",
            RecordType::PhoneNumber => "phoneNumberChecksums",
            RecordType::EmailAddress => "contactEmailAddressChecksums",
            RecordType::PostalAddress => "contactPostalAddressChecksums",
            RecordType::ContactDate => "contactDateChecksums",
            RecordType::ContactUrl => "contactUrlChecksums",
        }
    }

    /// The type's key in `sequenceNumbers` (connect request, inbound messages) and at the top level
    /// of the connect response.
    pub fn sequence_key(self) -> &'static str {
        match self {
            RecordType::Contact => "contact",
            RecordType::PhoneNumber => "phonenumber",
            RecordType::EmailAddress => "emailaddress",
            RecordType::PostalAddress => "postaladdress",
            RecordType::ContactDate => "contactdate",
            RecordType::ContactUrl => "contacturl",
        }
    }

    /// The item message's batch key.
    pub fn batch_key(self) -> &'static str {
        match self {
            RecordType::Contact => "contacts",
            RecordType::PhoneNumber => "phoneNumbers",
            RecordType::EmailAddress => "emailAddresses",
            RecordType::PostalAddress => "postalAddresses",
            RecordType::ContactDate => "contactDates",
            RecordType::ContactUrl => "contactUrls",
        }
    }

    /// The record id array inside the type's batch.
    pub fn batch_ids_key(self) -> &'static str {
        match self {
            RecordType::Contact => contact_key::CONTACT_IDS,
            RecordType::PhoneNumber => phone_key::PHONE_NUMBER_IDS,
            RecordType::EmailAddress => email_key::IDS,
            RecordType::PostalAddress => postal_key::IDS,
            RecordType::ContactDate => date_key::IDS,
            RecordType::ContactUrl => url_key::IDS,
        }
    }

    /// The store table holding the type's records.
    pub(crate) fn table(self) -> &'static str {
        match self {
            RecordType::Contact => "contact",
            RecordType::PhoneNumber => "phone_number",
            RecordType::EmailAddress => "email_address",
            RecordType::PostalAddress => "postal_address",
            RecordType::ContactDate => "contact_date",
            RecordType::ContactUrl => "contact_url",
        }
    }
}

/// `SyncAction`: `2` deletes the record; every other value (including the absent array's `Sync`)
/// inserts or replaces it.
pub mod sync_action {
    pub const SYNC: i32 = -1;
    pub const CREATE: i32 = 0;
    pub const UPDATE: i32 = 1;
    pub const DELETE: i32 = 2;
}

/// `ContactsCapabilities` flags (connect `capabilities.contacts`, metadata
/// `contactSettings.contactCapabilities`).
pub mod contacts_capabilities {
    pub const NONE: i32 = 0;
    pub const SYNC_V2: i32 = 1;
    pub const PBAP_1_1: i32 = 2;
    pub const TFL_CONTACT_SYNC: i32 = 4;
}

/// `PhoneNumberType` (stored as received, unvalidated).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhoneNumberType {
    Unknown = 0,
    PhoneHome = 1,
    PhoneMobile = 2,
    PhoneWork = 3,
    PhoneMobileWork = 4,
    PhoneMain = 5,
    PhoneOther = 6,
    PhoneSchool = 7,
    PhoneCustom = 8,
}

/// `EmailAddressType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EmailAddressType {
    Unknown = 0,
    Home = 1,
    Work = 2,
    School = 3,
    Other = 4,
    Custom = 5,
    Mobile = 6,
}

/// `PostalAddressType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PostalAddressType {
    Unknown = 0,
    Home = 1,
    Work = 2,
    School = 3,
    Other = 4,
    Custom = 5,
}

/// `ContactDateType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactDateType {
    Unknown = 0,
    Birthday = 1,
    Anniversary = 2,
    Custom = 3,
}

/// `ContactUrlType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContactUrlType {
    Unknown = 0,
    Homepage = 1,
    Home = 2,
    Work = 3,
    School = 4,
    Other = 5,
    Custom = 6,
}

impl PhoneNumberType {
    /// The named value of a stored `Int32`; `None` for a value outside the enum.
    pub fn from_i32(v: i32) -> Option<Self> {
        use PhoneNumberType::{
            PhoneCustom, PhoneHome, PhoneMain, PhoneMobile, PhoneMobileWork, PhoneOther, PhoneSchool, PhoneWork,
            Unknown,
        };
        [Unknown, PhoneHome, PhoneMobile, PhoneWork, PhoneMobileWork, PhoneMain, PhoneOther, PhoneSchool, PhoneCustom]
            .into_iter()
            .find(|t| *t as i32 == v)
    }
}

impl EmailAddressType {
    pub fn from_i32(v: i32) -> Option<Self> {
        use EmailAddressType::{Custom, Home, Mobile, Other, School, Unknown, Work};
        [Unknown, Home, Work, School, Other, Custom, Mobile].into_iter().find(|t| *t as i32 == v)
    }
}

impl PostalAddressType {
    pub fn from_i32(v: i32) -> Option<Self> {
        use PostalAddressType::{Custom, Home, Other, School, Unknown, Work};
        [Unknown, Home, Work, School, Other, Custom].into_iter().find(|t| *t as i32 == v)
    }
}

impl ContactDateType {
    pub fn from_i32(v: i32) -> Option<Self> {
        use ContactDateType::{Anniversary, Birthday, Custom, Unknown};
        [Unknown, Birthday, Anniversary, Custom].into_iter().find(|t| *t as i32 == v)
    }
}

impl ContactUrlType {
    pub fn from_i32(v: i32) -> Option<Self> {
        use ContactUrlType::{Custom, Home, Homepage, Other, School, Unknown, Work};
        [Unknown, Homepage, Home, Work, School, Other, Custom].into_iter().find(|t| *t as i32 == v)
    }
}

/// `{ContactsV2Sync: true, ContactThumbnails: true}` — the sync request's `configuration` and the
/// connect contribution's two `configuration` keys.
pub fn configuration_flags() -> ValueSet {
    ValueSet::new().with(CONFIG_CONTACTS_V2_SYNC, true).with(CONFIG_CONTACT_THUMBNAILS, true)
}

/// The `/contacts/v2` legacy request: `correlationVector` = the request's trace correlation id,
/// `contentType`, `subcontentTypes`, `configuration`; no headers.
///
/// The shared layer adds the legacy keys (`isuserinitiated` from `options`) and the standard keys.
pub fn sync_request(trace: TraceContext, options: RequestOptions) -> AppRequest {
    let payload = ValueSet::new()
        .with(key::CORRELATION_VECTOR, trace.correlation_id.clone())
        .with(key::CONTENT_TYPE, CONTENT_TYPE)
        .with(key::SUBCONTENT_TYPES, Value::StringArray(SUBCONTENT_TYPES.iter().map(|s| s.to_string()).collect()))
        .with(key::CONFIGURATION, configuration_flags());
    AppRequest::legacy(SYNC_ROUTE, payload).with_options(options).with_trace(trace)
}

#[cfg(test)]
mod tests;
