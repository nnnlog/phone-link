//! The contacts as the QML reads them.
//!
//! The list's items, a contact's card, and the Contacts section's state.

use phonelink_engine::api::contacts::{
    ContactCard, ContactCardField, ContactFieldLabel, ContactListItem, ContactsAccess, ContactsState,
};
use serde_json::{Value, json};

use super::state::PhoneRequest;
use super::sync_wait::SyncWait;
use crate::l10n::translate;

/// A field's label: its kind in lowercase ("mobile", "work mobile"), translated.
///
/// A label the user typed on the phone stays as typed, and a field without one has its kind's
/// `generic` label.
pub(super) fn label(label: &ContactFieldLabel, generic: &'static str) -> String {
    let text = match label {
        ContactFieldLabel::Home => "home",
        ContactFieldLabel::Mobile => "mobile",
        ContactFieldLabel::Work => "work",
        ContactFieldLabel::WorkMobile => "work mobile",
        ContactFieldLabel::Main => "main",
        ContactFieldLabel::School => "school",
        ContactFieldLabel::Other => "other",
        ContactFieldLabel::Birthday => "birthday",
        ContactFieldLabel::Anniversary => "anniversary",
        ContactFieldLabel::Custom { label } => return label.clone(),
        ContactFieldLabel::Generic => generic,
    };
    translate(text).to_owned()
}

fn fields(fields: &[ContactCardField], generic: &'static str) -> Vec<Value> {
    fields.iter().map(|field| json!({ "label": label(&field.label, generic), "value": field.value })).collect()
}

pub(super) fn contact_item_json(contact: &ContactListItem, photo: Option<String>) -> Value {
    json!({ "id": contact.contact_id, "name": contact.name, "photo": photo, "terms": contact.search_terms })
}

pub(super) fn card_json(card: &ContactCard, photo: Option<String>) -> Value {
    let phones: Vec<Value> = card
        .phone_numbers
        .iter()
        .map(|number| json!({ "label": label(&number.label, "phone"), "value": number.shown, "number": number.number }))
        .collect();
    json!({
        "id": card.contact_id,
        "name": card.name,
        "nickname": card.nickname,
        "company": card.company,
        "jobTitle": card.job_title,
        "notes": card.notes,
        "photo": photo,
        "phones": phones,
        "emails": fields(&card.email_addresses, "email"),
        "addresses": fields(&card.postal_addresses, "address"),
        "urls": fields(&card.urls, "homepage"),
        "dates": fields(&card.dates, "date"),
    })
}

fn contacts_access(access: ContactsAccess) -> &'static str {
    match access {
        ContactsAccess::Granted => "granted",
        ContactsAccess::PcConsentRequired => "consentRequired",
        ContactsAccess::DeviceConsentRequired => "permissionRequired",
        ContactsAccess::DeviceConsentDenied => "permissionDenied",
    }
}

pub(super) fn contacts_state_json(
    state: &ContactsState,
    wait: &SyncWait,
    request: &PhoneRequest,
    access_pending: bool,
) -> Value {
    json!({
        "capable": state.capable,
        "pcConsent": state.pc_consent,
        "access": contacts_access(state.access),
        "syncing": state.access == ContactsAccess::Granted && !state.up_to_date && !wait.over,
        "requesting": request.pending,
        "timedOut": request.timed_out,
        "accessPending": access_pending,
    })
}
