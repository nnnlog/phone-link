use super::*;
use phonelink_core::types::{DcgClient, Environment};
use phonelink_features::shared::db::StoreLocation;

fn store() -> ContactsStore {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    ContactsStore::new(Arc::new(FeatureDb::open_phone(&StoreLocation::Memory, &phone, FEATURE, MIGRATIONS).unwrap()))
}

fn contact(id: i64, checksum: i64, thumbnail: Option<Vec<u8>>) -> ContactRecord {
    ContactRecord {
        id,
        checksum,
        last_updated_ms: 0,
        display_name: format!("c{id}"),
        nickname: String::new(),
        company: String::new(),
        job_title: String::new(),
        notes: String::new(),
        name_prefix: String::new(),
        name_suffix: String::new(),
        given_name: String::new(),
        middle_name: String::new(),
        family_name: String::new(),
        thumbnail,
    }
}

fn phone_number(id: i64, contact_id: i64, checksum: i64, display: &str, label: &str) -> PhoneNumberRecord {
    PhoneNumberRecord {
        id,
        contact_id,
        checksum,
        number: format!("+1555{id}"),
        display_number: display.into(),
        kind: 2,
        label: label.into(),
    }
}

fn upserts<R>(records: Vec<R>) -> Option<Batch<R>> {
    Some(Batch { deletes: vec![], processed: vec![], upserts: records })
}

fn metadata(ids: [Vec<i64>; 6], checksums: [Vec<i64>; 6]) -> MetadataMessage {
    MetadataMessage { ids, checksums, contact_capabilities: None, sequence_numbers: None }
}

#[test]
fn reconciliation_deletes_by_omission_and_requests_changes() {
    let s = store();
    let item = ItemMessage {
        contacts: upserts(vec![contact(1, 10, Some(vec![1])), contact(2, 20, Some(vec![2])), contact(3, 30, None)]),
        phone_numbers: upserts(vec![phone_number(7, 2, 70, "", ""), phone_number(8, 1, 80, "555", "")]),
        ..Default::default()
    };
    s.apply_item(&item, None).unwrap();

    let mut ids: [Vec<i64>; 6] = Default::default();
    let mut sums: [Vec<i64>; 6] = Default::default();
    // Contact 1 unchanged, 2 changed, 3 thumbnail-less, 4 new; phone number 8 unchanged, 9 new.
    ids[0] = vec![1, 2, 3, 4];
    sums[0] = vec![10, 21, 30, 40];
    ids[1] = vec![8, 9];
    sums[1] = vec![80, 90];
    let requested = s.apply_metadata(&metadata(ids, sums), |_| None).unwrap();

    // Contact 2 also as the owner of display-less phone number 7 (evaluated before phone numbers
    // are reconciled, which then delete 7).
    let mut contacts = requested[0].clone();
    contacts.sort_unstable();
    assert_eq!(contacts, [2, 3, 4]);
    assert_eq!(requested[1], [9]);
    assert!(requested[2..].iter().all(Vec::is_empty));
    assert_eq!(s.records(RecordType::PhoneNumber).unwrap(), [(8, 80)]);

    // An absent id list deletes every record of the type.
    let requested = s.apply_metadata(&metadata(Default::default(), Default::default()), |_| None).unwrap();
    assert!(requested.iter().all(Vec::is_empty));
    assert!(s.records(RecordType::Contact).unwrap().is_empty());
    assert!(s.records(RecordType::PhoneNumber).unwrap().is_empty());
}

#[test]
fn owners_of_display_less_numbers_are_requested_even_when_not_on_the_phone() {
    let s = store();
    let item = ItemMessage { phone_numbers: upserts(vec![phone_number(5001, 101, 1, "", "")]), ..Default::default() };
    s.apply_item(&item, None).unwrap();

    let mut ids: [Vec<i64>; 6] = Default::default();
    let mut sums: [Vec<i64>; 6] = Default::default();
    ids[1] = vec![5001];
    sums[1] = vec![1];
    let requested = s.apply_metadata(&metadata(ids, sums), |_| None).unwrap();

    assert_eq!(requested[0], [101]);
    assert!(requested[1].is_empty());
}

#[test]
fn duplicate_sub_record_ids_are_requested_per_entry() {
    let s = store();
    let mut ids: [Vec<i64>; 6] = Default::default();
    let mut sums: [Vec<i64>; 6] = Default::default();
    ids[2] = vec![4, 4];
    sums[2] = vec![0, 0];
    ids[0] = vec![1, 1];
    sums[0] = vec![0, 0];

    let requested = s.apply_metadata(&metadata(ids, sums), |_| None).unwrap();
    assert_eq!(requested[2], [4, 4]);
    assert_eq!(requested[0], [1], "the contact list has no duplicates");
}

#[test]
fn contact_upsert_keeps_a_stored_thumbnail() {
    let s = store();
    s.apply_item(&ItemMessage { contacts: upserts(vec![contact(1, 1, Some(vec![9, 9]))]), ..Default::default() }, None)
        .unwrap();

    let mut update = contact(1, 2, None);
    update.display_name = "renamed".into();
    s.apply_item(&ItemMessage { contacts: upserts(vec![update]), ..Default::default() }, None).unwrap();

    let stored = s.contact(1).unwrap().unwrap();
    assert_eq!(stored.display_name, "renamed");
    assert_eq!(stored.checksum, 2);
    assert_eq!(stored.thumbnail, Some(vec![9, 9]));
}

#[test]
fn phone_number_upsert_compares_with_sql_null_semantics() {
    let s = store();
    let apply = |r: PhoneNumberRecord| {
        s.apply_item(&ItemMessage { phone_numbers: upserts(vec![r]), ..Default::default() }, None).unwrap()
    };
    let get = || s.phone_numbers(1).unwrap().remove(0);

    apply(phone_number(5, 1, 7, "555-01", "work"));

    // Same checksum/owner/number/type, empty display number and label: kept whole.
    apply(phone_number(5, 1, 7, "", ""));
    assert_eq!((get().display_number.as_str(), get().label.as_str()), ("555-01", "work"));

    // A changed checksum replaces: the empty display number clears the stored one.
    apply(phone_number(5, 1, 8, "", ""));
    assert_eq!((get().display_number.as_str(), get().label.as_str()), ("", ""));

    // A display number where none is stored is written.
    apply(phone_number(5, 1, 8, "555-02", ""));
    assert_eq!(get().display_number, "555-02");

    // A label where none is stored, all else equal: kept (NULL compares unknown).
    apply(phone_number(5, 1, 8, "555-02", "home"));
    assert_eq!(get().label, "");

    // A different non-empty display number replaces.
    apply(phone_number(5, 1, 8, "555-03", "home"));
    assert_eq!((get().display_number.as_str(), get().label.as_str()), ("555-03", "home"));
}

#[test]
fn completion_writes_current_and_last_updated_and_clear_resets() {
    let s = store();
    let seq = SequenceNumbers([5, -1, 3, -1, -1, 0]);
    s.apply_metadata(&metadata(Default::default(), Default::default()), |r| {
        assert!(r.iter().all(Vec::is_empty));
        Some(Completion { sequence_numbers: seq, now_ms: 1234 })
    })
    .unwrap();

    let stored = s.sequence_numbers().unwrap();
    assert_eq!(stored[0], Some(StoredSequence { current: 5, expected: -1 }));
    assert_eq!(stored[1], None, "-1 is not written");
    assert_eq!(stored[5], Some(StoredSequence { current: 0, expected: -1 }));
    assert_eq!(s.state().unwrap().last_updated_ms, Some(1234));

    // A lower value replaces.
    s.apply_item(
        &ItemMessage::default(),
        Some(Completion { sequence_numbers: SequenceNumbers([2, -1, -1, -1, -1, -1]), now_ms: 2000 }),
    )
    .unwrap();
    assert_eq!(s.sequence_numbers().unwrap()[0], Some(StoredSequence { current: 2, expected: -1 }));

    s.set_expected([9, 9, 9, 9, 9, 9]).unwrap();
    assert_eq!(s.sequence_numbers().unwrap()[1], Some(StoredSequence { current: -1, expected: 9 }));

    s.apply_item(&ItemMessage { contacts: upserts(vec![contact(1, 1, None)]), ..Default::default() }, None).unwrap();
    s.clear().unwrap();
    assert!(s.records(RecordType::Contact).unwrap().is_empty());
    assert_eq!(s.sequence_numbers().unwrap(), [None; 6]);
    assert_eq!(s.state().unwrap().last_updated_ms, Some(2000), "state survives a clear");
}

#[test]
fn state_flags() {
    let s = store();
    assert_eq!(
        s.state().unwrap(),
        StoredState { pc_consent: true, setup_completed_once: false, last_updated_ms: None }
    );

    assert!(!s.set_pc_consent(true).unwrap());
    assert!(s.set_pc_consent(false).unwrap());
    assert!(s.set_setup_completed_once().unwrap());
    assert!(!s.set_setup_completed_once().unwrap());

    assert_eq!(
        s.state().unwrap(),
        StoredState { pc_consent: false, setup_completed_once: true, last_updated_ms: None }
    );
}

#[test]
fn the_view_lists_contacts_by_shown_name_with_the_text_their_search_reads() {
    let s = store();
    let mut named = contact(1, 1, Some(vec![1]));
    named.display_name = "Bo".into();
    named.job_title = "Chef".into();
    let mut company_only = contact(2, 1, None);
    company_only.display_name = String::new();
    company_only.company = "Acme".into();

    let item = ItemMessage {
        contacts: upserts(vec![named, company_only]),
        phone_numbers: upserts(vec![phone_number(7, 1, 1, "010-1", ""), phone_number(8, 2, 1, "", "")]),
        ..Default::default()
    };
    s.apply_item(&item, None).unwrap();

    let listed = s.listed_contacts().unwrap();
    let got: Vec<_> = listed.iter().map(|c| (c.id, c.name.as_str(), c.has_thumbnail)).collect();
    assert_eq!(got, vec![(2, "Acme", false), (1, "Bo", true)], "the empty display name sorts first");

    let mut bo_terms = listed[1].terms.clone();
    bo_terms.sort();
    assert_eq!(bo_terms, ["+15557", "010-1", "Chef"], "the job title and both forms of the number");
    assert_eq!(listed[0].terms, ["Acme", "+15558"], "the company, and a number without a display form once");
}
