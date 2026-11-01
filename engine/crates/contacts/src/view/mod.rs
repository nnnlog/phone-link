//! What the contacts view shows: every contact with the text its search reads, and one contact's
//! card with the label of each field.
//!
//! The view's grouping, order and search run in the UI, on the system's own string comparison.

use super::model::ContactDetail;
use super::wire::{ContactDateType, ContactUrlType, EmailAddressType, PhoneNumberType, PostalAddressType};

/// One contact of the view's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedContact {
    pub id: i64,
    /// The shown name ([`shown_name`]); empty when the contact has none.
    pub name: String,
    pub has_thumbnail: bool,
    /// The other text the search reads, in no particular order: nickname, company, job title, notes,
    /// and the text of the contact's phone numbers (both forms), e-mail addresses, postal display
    /// addresses, URLs and dates. Empty fields are left out.
    pub terms: Vec<String>,
}

/// The label a field carries, from its record type's type value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
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
    Custom(String),
    /// The record type's generic label ("phone", "email", "address", "homepage", "date"): an unknown
    /// type, or a custom one without a label.
    Generic,
}

/// One field of a card: an e-mail address, a postal display address, a URL or a date's display text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardField {
    pub value: String,
    pub kind: FieldKind,
}

/// One phone number of a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardNumber {
    /// The number as stored: what Messaging is addressed to.
    pub number: String,
    /// What the view shows: the display number when one is stored, else the number.
    pub shown: String,
    pub kind: FieldKind,
}

/// One contact as the view's detail shows it; each list in record-id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactCard {
    pub id: i64,
    /// The shown name ([`shown_name`]); empty when the contact has none.
    pub name: String,
    pub nickname: String,
    pub company: String,
    pub job_title: String,
    pub notes: String,
    pub has_thumbnail: bool,
    pub phone_numbers: Vec<CardNumber>,
    pub email_addresses: Vec<CardField>,
    pub postal_addresses: Vec<CardField>,
    pub urls: Vec<CardField>,
    pub dates: Vec<CardField>,
}

/// The name the view shows: the display name, else the company, else the nickname; empty when all
/// three are.
pub fn shown_name<'a>(display_name: &'a str, company: &'a str, nickname: &'a str) -> &'a str {
    [display_name, company, nickname].into_iter().find(|s| !s.is_empty()).unwrap_or("")
}

/// `Custom` with its label, or the generic label when the label is empty.
fn custom(label: &str) -> FieldKind {
    if label.is_empty() { FieldKind::Generic } else { FieldKind::Custom(label.to_owned()) }
}

pub fn phone_kind(kind: i32, label: &str) -> FieldKind {
    match PhoneNumberType::from_i32(kind) {
        Some(PhoneNumberType::PhoneHome) => FieldKind::Home,
        Some(PhoneNumberType::PhoneMobile) => FieldKind::Mobile,
        Some(PhoneNumberType::PhoneWork) => FieldKind::Work,
        Some(PhoneNumberType::PhoneMobileWork) => FieldKind::WorkMobile,
        Some(PhoneNumberType::PhoneMain) => FieldKind::Main,
        Some(PhoneNumberType::PhoneOther) => FieldKind::Other,
        Some(PhoneNumberType::PhoneSchool) => FieldKind::School,
        Some(PhoneNumberType::PhoneCustom) => custom(label),
        Some(PhoneNumberType::Unknown) | None => FieldKind::Generic,
    }
}

pub fn email_kind(kind: i32, label: &str) -> FieldKind {
    match EmailAddressType::from_i32(kind) {
        Some(EmailAddressType::Home) => FieldKind::Home,
        Some(EmailAddressType::Work) => FieldKind::Work,
        Some(EmailAddressType::School) => FieldKind::School,
        Some(EmailAddressType::Other) => FieldKind::Other,
        Some(EmailAddressType::Custom) => custom(label),
        // `Mobile` has no label of its own.
        Some(EmailAddressType::Mobile | EmailAddressType::Unknown) | None => FieldKind::Generic,
    }
}

pub fn postal_kind(kind: i32, label: &str) -> FieldKind {
    match PostalAddressType::from_i32(kind) {
        Some(PostalAddressType::Home) => FieldKind::Home,
        Some(PostalAddressType::Work) => FieldKind::Work,
        Some(PostalAddressType::School) => FieldKind::School,
        Some(PostalAddressType::Other) => FieldKind::Other,
        Some(PostalAddressType::Custom) => custom(label),
        Some(PostalAddressType::Unknown) | None => FieldKind::Generic,
    }
}

pub fn url_kind(kind: i32, label: &str) -> FieldKind {
    match ContactUrlType::from_i32(kind) {
        Some(ContactUrlType::Home) => FieldKind::Home,
        Some(ContactUrlType::Work) => FieldKind::Work,
        Some(ContactUrlType::School) => FieldKind::School,
        Some(ContactUrlType::Other) => FieldKind::Other,
        Some(ContactUrlType::Custom) => custom(label),
        // A homepage's label is the generic one.
        Some(ContactUrlType::Homepage | ContactUrlType::Unknown) | None => FieldKind::Generic,
    }
}

pub fn date_kind(kind: i32, label: &str) -> FieldKind {
    match ContactDateType::from_i32(kind) {
        Some(ContactDateType::Birthday) => FieldKind::Birthday,
        Some(ContactDateType::Anniversary) => FieldKind::Anniversary,
        Some(ContactDateType::Custom) => custom(label),
        Some(ContactDateType::Unknown) | None => FieldKind::Generic,
    }
}

impl ContactCard {
    /// The card of a stored contact and its sub-records.
    pub fn from_detail(detail: ContactDetail) -> Self {
        let c = detail.contact;
        Self {
            id: c.id,
            name: shown_name(&c.display_name, &c.company, &c.nickname).to_owned(),
            has_thumbnail: c.thumbnail.is_some(),
            phone_numbers: detail
                .phone_numbers
                .into_iter()
                .map(|p| CardNumber {
                    kind: phone_kind(p.kind, &p.label),
                    shown: if p.display_number.is_empty() { p.number.clone() } else { p.display_number },
                    number: p.number,
                })
                .collect(),
            email_addresses: detail
                .email_addresses
                .into_iter()
                .map(|e| CardField { kind: email_kind(e.kind, &e.label), value: e.address })
                .collect(),
            postal_addresses: detail
                .postal_addresses
                .into_iter()
                .map(|a| CardField { kind: postal_kind(a.kind, &a.label), value: a.display_address })
                .collect(),
            urls: detail
                .urls
                .into_iter()
                .map(|u| CardField { kind: url_kind(u.kind, &u.label), value: u.address })
                .collect(),
            dates: detail
                .dates
                .into_iter()
                .map(|d| CardField { kind: date_kind(d.kind, &d.label), value: d.display_date })
                .collect(),
            nickname: c.nickname,
            company: c.company,
            job_title: c.job_title,
            notes: c.notes,
        }
    }
}

#[cfg(test)]
mod tests;
