use super::*;
use crate::parse::{
    ContactDateRecord, ContactRecord, ContactUrlRecord, EmailAddressRecord, PhoneNumberRecord, PostalAddressRecord,
};

#[test]
fn the_shown_name_falls_back_to_the_company_then_the_nickname() {
    let cases = [
        (("Alex Kim", "Contoso", "Al"), "Alex Kim"),
        (("", "Contoso", "Al"), "Contoso"),
        (("", "", "Al"), "Al"),
        (("", "", ""), ""),
    ];

    for ((display, company, nickname), shown) in cases {
        assert_eq!(shown_name(display, company, nickname), shown, "{display:?} {company:?} {nickname:?}");
    }
}

/// A record type's kind function.
type KindOf = fn(i32, &str) -> FieldKind;

#[test]
fn field_kinds_follow_each_record_types_labels() {
    let custom = |s: &str| FieldKind::Custom(s.into());
    let cases: [(KindOf, i32, &str, FieldKind); 16] = [
        (phone_kind, 1, "", FieldKind::Home),
        (phone_kind, 2, "", FieldKind::Mobile),
        (phone_kind, 4, "", FieldKind::WorkMobile),
        (phone_kind, 5, "", FieldKind::Main),
        (phone_kind, 8, "Boat", custom("Boat")),
        (phone_kind, 8, "", FieldKind::Generic),
        (phone_kind, 0, "ignored", FieldKind::Generic),
        (phone_kind, 42, "", FieldKind::Generic),
        (email_kind, 2, "", FieldKind::Work),
        (email_kind, 6, "", FieldKind::Generic),
        (email_kind, 5, "Club", custom("Club")),
        (postal_kind, 3, "", FieldKind::School),
        (url_kind, 1, "", FieldKind::Generic),
        (url_kind, 5, "", FieldKind::Other),
        (date_kind, 1, "", FieldKind::Birthday),
        (date_kind, 3, "", FieldKind::Generic),
    ];

    for (kind_of, raw, label, expected) in cases {
        assert_eq!(kind_of(raw, label), expected, "type {raw} label {label:?}");
    }
}

fn detail() -> ContactDetail {
    ContactDetail {
        contact: ContactRecord {
            id: 7,
            checksum: 1,
            last_updated_ms: 0,
            display_name: String::new(),
            nickname: "Al".into(),
            company: "Contoso".into(),
            job_title: "Engineer".into(),
            notes: "Met at the conference".into(),
            name_prefix: String::new(),
            name_suffix: String::new(),
            given_name: String::new(),
            middle_name: String::new(),
            family_name: String::new(),
            thumbnail: Some(vec![1, 2, 3]),
        },
        phone_numbers: vec![
            PhoneNumberRecord {
                id: 1,
                contact_id: 7,
                checksum: 0,
                number: "+821011112222".into(),
                display_number: "010-1111-2222".into(),
                kind: 2,
                label: String::new(),
            },
            PhoneNumberRecord {
                id: 2,
                contact_id: 7,
                checksum: 0,
                number: "+8221234567".into(),
                display_number: String::new(),
                kind: 8,
                label: "Desk".into(),
            },
        ],
        email_addresses: vec![EmailAddressRecord {
            id: 3,
            contact_id: 7,
            checksum: 0,
            address: "al@contoso.com".into(),
            kind: 2,
            label: String::new(),
        }],
        postal_addresses: vec![PostalAddressRecord {
            id: 4,
            contact_id: 7,
            checksum: 0,
            kind: 1,
            label: String::new(),
            display_address: "1 Main St, Seattle".into(),
            street: "1 Main St".into(),
            city: "Seattle".into(),
            region: String::new(),
            postal_code: String::new(),
            country_code: String::new(),
            sub_administrative_area: String::new(),
            sub_locality: String::new(),
        }],
        dates: vec![ContactDateRecord {
            id: 5,
            contact_id: 7,
            checksum: 0,
            kind: 1,
            label: String::new(),
            display_date: "1990-05-01".into(),
        }],
        urls: vec![ContactUrlRecord {
            id: 6,
            contact_id: 7,
            checksum: 0,
            kind: 1,
            label: String::new(),
            address: "contoso.com".into(),
        }],
    }
}

#[test]
fn a_card_shows_each_field_with_its_label_and_numbers_by_their_display_form() {
    let card = ContactCard::from_detail(detail());

    assert_eq!(card.name, "Contoso", "no display name: the company");
    assert!(card.has_thumbnail);
    assert_eq!(
        card.phone_numbers,
        vec![
            CardNumber { number: "+821011112222".into(), shown: "010-1111-2222".into(), kind: FieldKind::Mobile },
            CardNumber {
                number: "+8221234567".into(),
                shown: "+8221234567".into(),
                kind: FieldKind::Custom("Desk".into())
            },
        ]
    );
    assert_eq!(card.email_addresses, vec![CardField { value: "al@contoso.com".into(), kind: FieldKind::Work }]);
    assert_eq!(card.postal_addresses, vec![CardField { value: "1 Main St, Seattle".into(), kind: FieldKind::Home }]);
    assert_eq!(card.dates, vec![CardField { value: "1990-05-01".into(), kind: FieldKind::Birthday }]);
    assert_eq!(card.urls, vec![CardField { value: "contoso.com".into(), kind: FieldKind::Generic }]);
    assert_eq!((card.job_title.as_str(), card.notes.as_str()), ("Engineer", "Met at the conference"));
}
