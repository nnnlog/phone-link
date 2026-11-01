//! Inbound contact sync over the App layer: the `contactsV2` metadata and items, sessions,
//! completion, inbound error handling, the local state the protocol depends on, gating and
//! reachability.

use crate::support::app_world;
use crate::support::contacts_world;

use app_world::*;
use contacts_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::features::contacts::ContactsEvent;
use serde_json::json;

// ------------------------------------------------------------------------------------------------
// The derived example
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_example_exchange() {
    let world = world();
    let h = &world.h;

    // The PC already holds phone number 5001 (an item pushed on its own), not contact 101.
    let push = item(ValueSet::new().with("syncID", "earlier").with(
        "phoneNumbers",
        with(phone_batch(&[(5001, 101, "+15550100", "555-0100")]), ValueSet::new().with("checksums", vec![99120i64])),
    ));
    assert_eq!(send(h, push).await, result(0));

    let answer = send(
        h,
        metadata(
            ValueSet::new()
                .with("contactIds", vec![101i64])
                .with("contactChecksums", vec![7340192i64])
                .with("phoneNumberIds", vec![5001i64])
                .with("phoneNumberChecksums", vec![99120i64])
                .with("contactEmailAddressIds", int64s(&[]))
                .with("contactPostalAddressIds", int64s(&[]))
                .with("contactDateIds", int64s(&[]))
                .with("contactUrlIds", int64s(&[]))
                .with("contactSettings", ValueSet::new().with("contactCapabilities", 1i32))
                .with("sequenceNumbers", seq([42, 17, 3, 0, 0, 0])),
        ),
    )
    .await;
    assert_eq!(
        answer.to_json(),
        json!({
            "correlationVector": {"String": "cv-1"},
            "contentType": {"String": "contactsV2"},
            "contractVersion": {"Double": 3.99},
            "itemCount": {"Int32": 1},
            "contactIds": {"Int64Array": [101]},
            "phoneNumberIds": {"Int64Array": []},
            "contactEmailAddressIds": {"Int64Array": []},
            "contactPostalAddressIds": {"Int64Array": []},
            "contactDateIds": {"Int64Array": []},
            "contactUrlIds": {"Int64Array": []},
            "result": {"Int32": 0}
        })
    );
    assert_eq!(
        answer.keys().collect::<Vec<_>>(),
        [
            "correlationVector",
            "contentType",
            "contractVersion",
            "itemCount",
            "contactIds",
            "phoneNumberIds",
            "contactEmailAddressIds",
            "contactPostalAddressIds",
            "contactDateIds",
            "contactUrlIds",
            "result"
        ]
    );
    let sent = h.messenger.responses().pop().unwrap();
    assert_eq!(sent.header("result").as_deref(), Some("0"));
    assert_eq!(h.app.phone_record().unwrap().contacts_capabilities, Some(1));

    // Not complete yet: nothing written.
    assert_eq!(sync_state(&world).current, [-1; 6]);

    let contact = with(
        contacts_batch(&[101]),
        ValueSet::new()
            .with("checksums", vec![7340192i64])
            .with("thumbnailSizes", vec![3i64])
            .with("thumbnails", vec![0xFFu8, 0xD8, 0xFF]),
    );
    assert_eq!(send(h, item(ValueSet::new().with("contacts", contact))).await, result(0));

    let state = sync_state(&world);
    assert_eq!(state.current, [42, 17, 3, 0, 0, 0], "current from the session's last sequenceNumbers");
    assert!(state.last_updated.is_some());
    assert_eq!(world.contacts.thumbnail(&phone(), 101).unwrap(), Some(vec![0xFF, 0xD8, 0xFF]));
}

// ------------------------------------------------------------------------------------------------
// Reconciliation
// ------------------------------------------------------------------------------------------------

async fn seed(world: &World, contacts: ValueSet) {
    let answer = send(&world.h, item(ValueSet::new().with("syncID", "seed").with("contacts", contacts))).await;
    assert_eq!(answer, result(0));
}

#[tokio::test(start_paused = true)]
async fn metadata_deletes_by_omission_and_requests_changed_and_photo_less_contacts() {
    let world = world();
    let with_photo = with(
        contacts_batch(&[1, 2, 3]),
        ValueSet::new().with("thumbnailSizes", vec![1i64, 0, 1]).with("thumbnails", vec![7u8, 8]),
    );
    seed(&world, with_photo).await;

    // 1 unchanged (photo), 2 unchanged but photo-less, 3 gone, 4 new; 1's checksum read as 10.
    let answer = send(
        &world.h,
        metadata(ValueSet::new().with("contactIds", vec![1i64, 2, 4]).with("contactChecksums", vec![10i64, 20, 40])),
    )
    .await;

    let mut requested = answer.i64_array("contactIds").unwrap().to_vec();
    requested.sort_unstable();
    assert_eq!(requested, [2, 4]);
    assert_eq!(answer.i32("itemCount"), Some(2));
    assert_eq!(contact_names(&world), [(1, "Contact 1".into()), (2, "Contact 2".into())]);

    // Absent checksums read 0: every stored contact differs.
    let answer = send(&world.h, metadata(ValueSet::new().with("contactIds", vec![1i64]))).await;
    assert_eq!(answer.i64_array("contactIds").unwrap(), [1]);

    // Absent ids: the phone has none — every contact is deleted, nothing requested.
    let answer = send(&world.h, metadata(ValueSet::new())).await;
    assert_eq!(answer.i32("itemCount"), Some(0));
    assert!(contact_names(&world).is_empty());
}

#[tokio::test(start_paused = true)]
async fn owners_of_display_less_phone_numbers_are_requested() {
    let world = world();
    let with_photos = with(
        contacts_batch(&[1, 2]),
        ValueSet::new().with("thumbnailSizes", vec![1i64, 1]).with("thumbnails", vec![1u8, 2]),
    );
    seed(&world, with_photos).await;

    let numbers = phone_batch(&[(11, 1, "+15550101", "555-0101"), (12, 2, "+15550102", "")]);
    send(&world.h, item(ValueSet::new().with("syncID", "seed2").with("phoneNumbers", numbers))).await;

    // Everything in sync by checksum: only contact 2 (owner of 12, no display number) is requested.
    let answer = send(
        &world.h,
        metadata(
            ValueSet::new()
                .with("contactIds", vec![1i64, 2])
                .with("contactChecksums", vec![10i64, 20])
                .with("phoneNumberIds", vec![11i64, 12])
                .with("phoneNumberChecksums", vec![110i64, 120]),
        ),
    )
    .await;

    assert_eq!(answer.i64_array("contactIds").unwrap(), [2]);
    assert_eq!(answer.i64_array("phoneNumberIds").unwrap(), [] as [i64; 0]);
}

// ------------------------------------------------------------------------------------------------
// Sessions and completion
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn items_continue_the_session_until_every_pending_set_is_empty() {
    let world = world();
    let h = &world.h;

    let answer = send(
        h,
        metadata(
            ValueSet::new()
                .with("syncID", "s1")
                .with("contactIds", vec![1i64])
                .with("phoneNumberIds", vec![5i64])
                .with("contactEmailAddressIds", vec![7i64])
                .with("sequenceNumbers", seq([1, 1, 1, 1, 1, 1])),
        ),
    )
    .await;

    assert_eq!(answer.i32("itemCount"), Some(3));

    // Contact 1 plus contact 99, which was never requested: applied like any other.
    let first = item(ValueSet::new().with("syncID", "s1").with("contacts", contacts_batch(&[1, 99])));
    assert_eq!(send(h, first).await, result(0));
    assert_eq!(sync_state(&world).current, [-1; 6], "still pending");
    assert_eq!(contact_names(&world).len(), 2);

    let second = item(
        ValueSet::new()
            .with("syncID", "s1")
            .with("correlationVector", "another")
            .with("phoneNumbers", phone_batch(&[(5, 1, "+15550105", "555-0105")]))
            .with("sequenceNumbers", seq([9, 8, 7, -1, 5, 4])),
    );
    assert_eq!(send(h, second).await, result(0));
    assert_eq!(sync_state(&world).current, [-1; 6], "the e-mail is still pending");

    let third =
        item(ValueSet::new().with("syncID", "s1").with("emailAddresses", email_batch(&[(7, 1, "a@example.com")])));
    assert_eq!(send(h, third).await, result(0));

    // The last sequenceNumbers object of the session; -1 is not written.
    assert_eq!(sync_state(&world).current, [9, 8, 7, -1, 5, 4]);

    // The session is closed: the next item under s1 is processed on its own.
    let after = item(ValueSet::new().with("syncID", "s1").with("sequenceNumbers", seq([2, -1, -1, -1, -1, -1])));
    assert_eq!(send(h, after).await, result(0));
    assert_eq!(sync_state(&world).current[0], 2, "a lower value replaces");
}

#[tokio::test(start_paused = true)]
async fn an_item_without_a_session_is_applied_on_its_own() {
    let world = world();
    let mut rx = world.contacts.subscribe();

    let push = item(
        ValueSet::new()
            .with("syncID", "delta")
            .with("contacts", with(contacts_batch(&[3, 4]), ValueSet::new().with("actions", vec![0i32, 1])))
            .with("sequenceNumbers", seq([12, -1, -1, -1, -1, -1])),
    );

    assert_eq!(send(&world.h, push).await, result(0));
    assert_eq!(contact_names(&world).len(), 2);
    assert_eq!(sync_state(&world).current[0], 12, "stored at once");

    let events = drain(&mut rx);
    assert!(events.contains(&ContactsEvent::RecordsChanged { phone: phone() }));
    assert!(events.contains(&ContactsEvent::SyncStateChanged { phone: phone() }));
}

#[tokio::test(start_paused = true)]
async fn a_metadata_message_requesting_nothing_leaves_the_stored_session() {
    let world = world();
    let h = &world.h;

    // Session A under s1 requests contact 1.
    let a = metadata(
        ValueSet::new()
            .with("syncID", "s1")
            .with("contactIds", vec![1i64])
            .with("sequenceNumbers", seq([10, -1, -1, -1, -1, -1])),
    );
    assert_eq!(send(h, a).await.i32("itemCount"), Some(1));

    // B under s1 requests nothing: completes at once with its own numbers and the last-updated time.
    assert_eq!(sync_state(&world).last_updated, None);
    let b = metadata(ValueSet::new().with("syncID", "s1").with("sequenceNumbers", seq([20, -1, -1, -1, -1, -1])));
    assert_eq!(send(h, b).await.i32("itemCount"), Some(0));
    assert_eq!(sync_state(&world).current[0], 20);
    assert!(sync_state(&world).last_updated.is_some(), "completion records the last-updated time");

    // The item continues A, which is still stored: A's numbers are written.
    let i = item(ValueSet::new().with("syncID", "s1").with("contacts", contacts_batch(&[1])));
    assert_eq!(send(h, i).await, result(0));
    assert_eq!(sync_state(&world).current[0], 10);
}

#[tokio::test(start_paused = true)]
async fn a_metadata_message_requesting_ids_replaces_the_session_under_its_key() {
    let world = world();
    let h = &world.h;

    // Keyed by the correlation id (no syncID).
    send(
        h,
        metadata(ValueSet::new().with("contactIds", vec![1i64]).with("sequenceNumbers", seq([1, -1, -1, -1, -1, -1]))),
    )
    .await;
    send(
        h,
        metadata(ValueSet::new().with("contactIds", vec![2i64]).with("sequenceNumbers", seq([2, -1, -1, -1, -1, -1]))),
    )
    .await;

    // Contact 1 is not pending in the replacing session: the session stays open.
    send(h, item(ValueSet::new().with("contacts", contacts_batch(&[1])))).await;
    assert_eq!(sync_state(&world).current[0], -1);

    send(h, item(ValueSet::new().with("contacts", contacts_batch(&[2])))).await;
    assert_eq!(sync_state(&world).current[0], 2);
}

#[tokio::test(start_paused = true)]
async fn the_header_correlation_id_keys_the_session_and_answers_the_metadata() {
    let world = world();
    let h = &world.h;

    let payload = ValueSet::new()
        .with("contentType", "contactsV2")
        .with("contractVersion", 3.99f64)
        .with("metadata", true)
        .with("contactIds", vec![1i64]);
    let msg = message(
        &[("_requestId", "req-1"), ("_route", "/legacy/phonecontent"), ("correlationVector", "hdr-cv")],
        phonelink_engine::codec::valueset::encode(&payload),
    );

    let answer = send(h, msg).await;
    assert_eq!(answer.str("correlationVector"), Some("hdr-cv"));

    // An item with the same header correlation id continues it.
    let msg = message(
        &[("_requestId", "req-2"), ("_route", "/legacy/phonecontent"), ("correlationVector", "hdr-cv")],
        phonelink_engine::codec::valueset::encode(
            &ValueSet::new()
                .with("contentType", "contactsV2")
                .with("contractVersion", 3.99f64)
                .with("contacts", contacts_batch(&[1])),
        ),
    );
    assert_eq!(send(h, msg).await, result(0));
    assert!(sync_state(&world).last_updated.is_some(), "completed");
}

#[tokio::test(start_paused = true)]
async fn a_syncid_that_is_not_a_string_leaves_the_correlation_id_as_the_key() {
    let world = world();
    let open = metadata(
        ValueSet::new().with("syncID", 5i32).with("contactIds", vec![1i64]).with("sequenceNumbers", seq([8; 6])),
    );
    assert_eq!(send(&world.h, open).await.i32("itemCount"), Some(1));

    // The same correlation id (cv-1) without a syncID continues the session and completes it.
    send(&world.h, item(ValueSet::new().with("contacts", contacts_batch(&[1])))).await;
    assert_eq!(sync_state(&world).current, [8; 6]);
}

// ------------------------------------------------------------------------------------------------
// Batch rules
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn deletes_apply_before_upserts_and_contact_deletes_keep_sub_records() {
    let world = world();
    let h = &world.h;

    seed(&world, contacts_batch(&[1, 2])).await;
    send(h, item(ValueSet::new().with("syncID", "p").with("phoneNumbers", phone_batch(&[(21, 2, "+15550121", "")]))))
        .await;

    // Delete and upsert of contact 1 in one batch: the upsert wins. Delete contact 2.
    let batch = ValueSet::new()
        .with("itemCount", 3i32)
        .with("contactIds", vec![1i64, 1, 2])
        .with("lastUpdatedTimes", vec![0i64, 0, 0])
        .with("displayNames", vec!["One again", "One deleted", "Two"])
        .with("actions", vec![1i32, 2, 2]);
    assert_eq!(send(h, item(ValueSet::new().with("syncID", "d").with("contacts", batch))).await, result(0));
    assert_eq!(contact_names(&world), [(1, "One again".into())]);

    // Contact 2's phone number survived: it shows again when the contact comes back.
    send(h, item(ValueSet::new().with("syncID", "e").with("contacts", contacts_batch(&[2])))).await;
    let detail = world.contacts.contact(&phone(), 2).unwrap().unwrap();
    assert_eq!(detail.phone_numbers.iter().map(|p| p.id).collect::<Vec<_>>(), [21]);
}

#[tokio::test(start_paused = true)]
async fn an_upsert_without_a_thumbnail_keeps_the_stored_one() {
    let world = world();
    seed(
        &world,
        with(contacts_batch(&[1]), ValueSet::new().with("thumbnailSizes", vec![2i64]).with("thumbnails", vec![1u8, 2])),
    )
    .await;

    let update = with(contacts_batch(&[1]), ValueSet::new().with("displayNames", vec!["Renamed"]));
    send(&world.h, item(ValueSet::new().with("syncID", "u").with("contacts", update))).await;

    let detail = world.contacts.contact(&phone(), 1).unwrap().unwrap();
    assert_eq!(detail.contact.display_name, "Renamed");
    assert_eq!(detail.contact.thumbnail, Some(vec![1, 2]));
    assert!(world.contacts.contacts(&phone()).unwrap()[0].has_thumbnail);
}

/// One phone-number batch of a single entry with every per-entry array present.
fn number(id: i64, owner: i64, checksum: i64, number: &str, display: &str, kind: i32, label: &str) -> ValueSet {
    ValueSet::new()
        .with("itemCount", 1i32)
        .with("phoneNumberIds", vec![id])
        .with("contactIds", vec![owner])
        .with("checksums", vec![checksum])
        .with("phoneNumbers", vec![number])
        .with("displayNumbers", vec![display])
        .with("types", vec![kind])
        .with("labels", vec![label])
}

#[tokio::test(start_paused = true)]
async fn a_phone_number_upsert_replaces_the_record_when_a_compared_field_differs() {
    let world = world();
    let numbers_of = |owner: i64| world.contacts.contact(&phone(), owner).unwrap().unwrap().phone_numbers;
    let apply = |b: ValueSet| item(ValueSet::new().with("phoneNumbers", b));

    send(&world.h, item(ValueSet::new().with("contacts", contacts_batch(&[1, 2])))).await;
    send(&world.h, apply(number(5, 1, 7, "+1", "d", 2, "home"))).await;

    // Both labels non-empty and different, all else equal: replaced (the empty display number goes).
    send(&world.h, apply(number(5, 1, 7, "+1", "", 2, "work"))).await;
    assert_eq!((numbers_of(1)[0].label.as_str(), numbers_of(1)[0].display_number.as_str()), ("work", ""));

    // The type differs: replaced.
    send(&world.h, apply(number(5, 1, 7, "+1", "", 3, ""))).await;
    assert_eq!((numbers_of(1)[0].kind, numbers_of(1)[0].label.as_str()), (3, ""));

    // The number differs: replaced.
    send(&world.h, apply(number(5, 1, 7, "+2", "x", 3, "l"))).await;
    assert_eq!((numbers_of(1)[0].number.as_str(), numbers_of(1)[0].label.as_str()), ("+2", "l"));

    // The owner differs: replaced.
    send(&world.h, apply(number(5, 2, 7, "+2", "", 3, ""))).await;
    assert!(numbers_of(1).is_empty());
    assert_eq!(numbers_of(2)[0].display_number, "");
}

#[tokio::test(start_paused = true)]
async fn a_deleted_entrys_thumbnail_size_is_not_read() {
    let world = world();
    let batch =
        with(contacts_batch(&[1]), ValueSet::new().with("thumbnailSizes", vec![i64::MAX]).with("actions", vec![2i32]));
    assert_eq!(send(&world.h, item(ValueSet::new().with("contacts", batch))).await, result(0));
}

#[tokio::test(start_paused = true)]
async fn a_negative_item_count_bounds_nothing_when_every_array_is_present_and_enums_are_stored_as_sent() {
    let world = world();
    let urls = ValueSet::new()
        .with("itemCount", -1i32)
        .with("contactUrlIds", vec![1i64, 2])
        .with("contactIds", vec![7i64, 7])
        .with("checksums", vec![0i64, 0])
        .with("type", vec![99i32, -4])
        .with("label", vec!["a", "b"])
        .with("address", vec!["x", "y"])
        .with("actions", vec![0i32, 0]);

    let msg = item(ValueSet::new().with("contacts", contacts_batch(&[7])).with("contactUrls", urls));
    assert_eq!(send(&world.h, msg).await, result(0));

    let urls = world.contacts.contact(&phone(), 7).unwrap().unwrap().urls;
    assert_eq!(urls.iter().map(|u| (u.id, u.kind)).collect::<Vec<_>>(), [(1, 99), (2, -4)]);
}

#[tokio::test(start_paused = true)]
#[expect(clippy::too_many_lines, reason = "checks every field of every record type")]
async fn every_record_type_is_stored_with_its_fields() {
    let world = world();
    let full_contact = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactIds", vec![1i64])
        .with("lastUpdatedTimes", vec![1_700_000_000_123i64])
        .with("checksums", vec![5i64])
        .with("displayNames", vec!["D"])
        .with("nicknames", vec!["N"])
        .with("companies", vec!["C"])
        .with("jobTitles", vec!["J"])
        .with("notes", vec!["No"])
        .with("namePrefixes", vec!["Dr"])
        .with("nameSuffixes", vec!["Jr"])
        .with("givenNames", vec!["G"])
        .with("middleNames", vec!["M"])
        .with("familyNames", vec!["F"]);
    let postal = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactPostalAddressIds", vec![31i64])
        .with("contactIds", vec![1i64])
        .with("postalAddressType", vec![2i32])
        .with("postalAddressLabel", vec!["office"])
        .with("postalAddressDisplayAddress", vec!["1 Main St"])
        .with("postalAddressStreet", vec!["Main St 1"])
        .with("postalAddressCity", vec!["Town"])
        .with("postalAddressRegion", vec!["State"])
        .with("postalAddressPostalCode", vec!["12345"])
        .with("postalAddressCountryCode", vec!["US"])
        .with("postalAddressSubAdministrativeArea", vec!["County"])
        .with("postalAddressSubLocality", vec!["Quarter"]);
    let dates = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactDateIds", vec![41i64])
        .with("contactIds", vec![1i64])
        .with("type", vec![1i32])
        .with("label", vec![""])
        .with("displayDate", vec!["1990-05-17"]);
    let urls = ValueSet::new()
        .with("itemCount", 1i32)
        .with("contactUrlIds", vec![51i64])
        .with("contactIds", vec![1i64])
        .with("type", vec![1i32])
        .with("label", vec!["blog"])
        .with("address", vec!["https://example.com"]);
    let emails = with(
        email_batch(&[(61, 1, "d@example.com")]),
        ValueSet::new().with("emailAddressType", vec![2i32]).with("emailAddressLabel", vec!["w"]),
    );
    let msg = item(
        ValueSet::new()
            .with("contacts", full_contact)
            .with(
                "phoneNumbers",
                with(phone_batch(&[(11, 1, "+15550111", "555-0111")]), ValueSet::new().with("labels", vec!["cell"])),
            )
            .with("emailAddresses", emails)
            .with("postalAddresses", postal)
            .with("contactDates", dates)
            .with("contactUrls", urls),
    );

    assert_eq!(send(&world.h, msg).await, result(0));

    let d = world.contacts.contact(&phone(), 1).unwrap().unwrap();
    let c = &d.contact;
    assert_eq!(
        (
            c.checksum,
            c.last_updated_ms,
            c.display_name.as_str(),
            c.nickname.as_str(),
            c.company.as_str(),
            c.job_title.as_str()
        ),
        (5, 1_700_000_000_123, "D", "N", "C", "J")
    );
    assert_eq!(
        [&c.notes, &c.name_prefix, &c.name_suffix, &c.given_name, &c.middle_name, &c.family_name].map(|s| s.as_str()),
        ["No", "Dr", "Jr", "G", "M", "F"]
    );
    assert_eq!(d.phone_numbers[0].display_number, "555-0111");
    assert_eq!(d.phone_numbers[0].label, "cell");
    assert_eq!(d.phone_numbers[0].kind, 2);
    assert_eq!(
        (d.email_addresses[0].address.as_str(), d.email_addresses[0].kind, d.email_addresses[0].label.as_str()),
        ("d@example.com", 2, "w")
    );
    let p = &d.postal_addresses[0];
    assert_eq!(
        [
            &p.label,
            &p.display_address,
            &p.street,
            &p.city,
            &p.region,
            &p.postal_code,
            &p.country_code,
            &p.sub_administrative_area,
            &p.sub_locality
        ]
        .map(|s| s.as_str()),
        ["office", "1 Main St", "Main St 1", "Town", "State", "12345", "US", "County", "Quarter"]
    );
    assert_eq!(p.kind, 2);
    assert_eq!(d.birthday(), Some("1990-05-17"));
    assert_eq!(
        (d.urls[0].address.as_str(), d.urls[0].label.as_str(), d.urls[0].kind),
        ("https://example.com", "blog", 1)
    );
}

// ------------------------------------------------------------------------------------------------
// contactSettings, gating
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn contact_settings_write_the_contacts_capabilities() {
    let world = world();
    let caps = || world.h.app.phone_record().unwrap().contacts_capabilities;
    assert_eq!(caps(), None);

    send(
        &world.h,
        metadata(ValueSet::new().with("contactSettings", ValueSet::new().with("contactCapabilities", 5i32))),
    )
    .await;
    assert_eq!(caps(), Some(5));

    send(&world.h, metadata(ValueSet::new())).await;
    assert_eq!(caps(), Some(5), "no contactSettings: unchanged");
    send(&world.h, metadata(ValueSet::new().with("contactSettings", 1i32))).await;
    assert_eq!(caps(), Some(5), "not an Object: ignored");
    send(&world.h, metadata(ValueSet::new().with("contactSettings", ValueSet::new()))).await;
    assert_eq!(caps(), Some(0), "contactSettings without contactCapabilities: None (0)");
}

#[tokio::test(start_paused = true)]
async fn inbound_contacts_is_processed_whatever_capability_permission_or_consent() {
    // Feature bits without Contacts; the contacts permission denied; PC consent off.
    let world = world_with_custom_data(r#"{"yppCapabilities":32,"registeredPermissionTypes":"1"}"#);
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    world.contacts.set_pc_consent(&phone(), false).unwrap();
    assert!(!world.h.app.phone_record().unwrap().has_capability(0x800));

    seed(&world, contacts_batch(&[1])).await;
    assert_eq!(contact_names(&world).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn other_contacts_content_types_have_no_processor() {
    let world = world();
    for ct in ["contacts", "contactsVCard", "contactThumbs", "phoneNumbers"] {
        let meta = phone_content(ct, ValueSet::new().with("metadata", true).with("contactIds", vec![1i64]));
        assert_eq!(send(&world.h, meta).await, result(2), "{ct} metadata");

        let content = phone_content(ct, ValueSet::new().with("contacts", contacts_batch(&[1])));
        assert_eq!(send(&world.h, content).await, result(1), "{ct} content");
    }
    assert!(contact_names(&world).is_empty());
}

// ------------------------------------------------------------------------------------------------
// Malformed messages
// ------------------------------------------------------------------------------------------------

fn base_metadata(extra: ValueSet) -> ValueSet {
    with(
        ValueSet::new().with("contentType", "contactsV2").with("contractVersion", 3.99f64).with("metadata", true),
        extra,
    )
}

#[tokio::test(start_paused = true)]
async fn malformed_metadata_is_rejected_changes_nothing_and_discards_the_session_under_its_key() {
    let cases: Vec<(&str, ValueSet)> = vec![
        ("no correlationVector", base_metadata(ValueSet::new())),
        (
            "Int32Array ids",
            base_metadata(ValueSet::new().with("correlationVector", "cv-1").with("contactIds", vec![1i32])),
        ),
        (
            "short checksums",
            base_metadata(
                ValueSet::new()
                    .with("correlationVector", "cv-1")
                    .with("contactIds", vec![1i64, 2])
                    .with("contactChecksums", vec![1i64]),
            ),
        ),
        (
            "Int64 contactCapabilities",
            base_metadata(
                ValueSet::new()
                    .with("correlationVector", "cv-1")
                    .with("contactSettings", ValueSet::new().with("contactCapabilities", 1i64)),
            ),
        ),
        (
            "Int32 sequence number",
            base_metadata(
                ValueSet::new()
                    .with("correlationVector", "cv-1")
                    .with("sequenceNumbers", ValueSet::new().with("contact", 1i32)),
            ),
        ),
    ];
    for (name, payload) in cases {
        let world = world();
        seed(&world, contacts_batch(&[1, 2])).await;

        // A session under s1 waiting for both (photo-less) contacts; completing it would write 7.
        let open = metadata(
            ValueSet::new()
                .with("syncID", "s1")
                .with("contactIds", vec![1i64, 2])
                .with("contactChecksums", vec![10i64, 20])
                .with("sequenceNumbers", seq([7, -1, -1, -1, -1, -1])),
        );
        assert_eq!(send(&world.h, open).await.i32("itemCount"), Some(2), "{name}");
        let before = sync_state(&world);

        // The malformed metadata message under the same key.
        let answer = send(&world.h, raw(with(payload, ValueSet::new().with("syncID", "s1")))).await;
        assert_eq!(answer, result(1), "{name}");
        assert_eq!(world.h.messenger.responses().last().unwrap().header("result").as_deref(), Some("1"), "{name}");
        assert_eq!(contact_names(&world).len(), 2, "{name}: nothing deleted");
        assert_eq!(sync_state(&world), before, "{name}");
        assert_eq!(world.h.app.phone_record().unwrap().contacts_capabilities, None, "{name}");

        // The stored session was discarded: the item under s1 is processed on its own, so the
        // session's 7 is never written.
        let i = item(ValueSet::new().with("syncID", "s1").with("contacts", contacts_batch(&[1, 2])));
        assert_eq!(send(&world.h, i).await, result(0), "{name}");
        assert_eq!(sync_state(&world).current[0], -1, "{name}: the session under its key was discarded");
    }
}

#[tokio::test(start_paused = true)]
async fn malformed_items_are_rejected_drop_their_session_and_change_nothing() {
    let short_thumbnails =
        with(contacts_batch(&[5]), ValueSet::new().with("thumbnailSizes", vec![4i64]).with("thumbnails", vec![1u8]));
    let cases: Vec<(&str, ValueSet)> = vec![
        ("batch without itemCount", ValueSet::new().with("contacts", ValueSet::new().with("contactIds", vec![5i64]))),
        (
            "Int64 itemCount",
            ValueSet::new().with("contacts", with(contacts_batch(&[5]), ValueSet::new().with("itemCount", 1i64))),
        ),
        (
            "missing lastUpdatedTimes",
            ValueSet::new().with("contacts", ValueSet::new().with("itemCount", 1i32).with("contactIds", vec![5i64])),
        ),
        (
            "fewer entries than itemCount",
            ValueSet::new().with("contacts", with(contacts_batch(&[5]), ValueSet::new().with("itemCount", 2i32))),
        ),
        ("thumbnails too short", ValueSet::new().with("contacts", short_thumbnails)),
        (
            "lastUpdatedTimes below 0001-01-01",
            ValueSet::new().with(
                "contacts",
                with(contacts_batch(&[5]), ValueSet::new().with("lastUpdatedTimes", vec![-62_135_596_800_001i64])),
            ),
        ),
        (
            "mistyped sub-record array",
            ValueSet::new().with(
                "emailAddresses",
                with(email_batch(&[(1, 5, "x@y.z")]), ValueSet::new().with("emailAddresses", vec![1i64])),
            ),
        ),
        // A valid contacts batch with a bad phone-number batch: neither is applied.
        (
            "second batch malformed",
            ValueSet::new()
                .with("contacts", contacts_batch(&[5]))
                .with("phoneNumbers", ValueSet::new().with("itemCount", 1i32).with("phoneNumberIds", vec![1i64])),
        ),
        ("Int32 sequence number", ValueSet::new().with("sequenceNumbers", ValueSet::new().with("contact", 1i32))),
    ];
    for (name, payload) in cases {
        let world = world();
        let h = &world.h;

        // An open session under cv-1 waiting for contact 5.
        let open = metadata(
            ValueSet::new().with("contactIds", vec![5i64]).with("sequenceNumbers", seq([3, -1, -1, -1, -1, -1])),
        );
        assert_eq!(send(h, open).await.i32("itemCount"), Some(1));

        assert_eq!(send(h, item(payload)).await, result(1), "{name}");
        assert!(contact_names(&world).is_empty(), "{name}: nothing applied");

        // The session was discarded: contact 5 now arrives on its own and completes with no numbers.
        assert_eq!(send(h, item(ValueSet::new().with("contacts", contacts_batch(&[5])))).await, result(0), "{name}");
        assert_eq!(sync_state(&world).current, [-1; 6], "{name}: the dropped session's numbers are never written");
    }
}

fn raw_item(ids: &[i64]) -> ValueSet {
    ValueSet::new()
        .with("contentType", "contactsV2")
        .with("contractVersion", 3.99f64)
        .with("contacts", contacts_batch(ids))
}

#[tokio::test(start_paused = true)]
async fn an_item_that_opens_a_session_needs_a_correlation_vector_of_any_type() {
    let world = world();
    // Absent from the payload and the envelope: refused (Phone Link on Windows does not answer it).
    assert_eq!(send(&world.h, raw(raw_item(&[1]))).await, result(1));
    assert!(contact_names(&world).is_empty());

    // Present with another DataValueType: read as none, and the item is applied.
    for (id, cv) in [(1i64, Value::Int32(3)), (2, Value::Empty)] {
        let opening = with(raw_item(&[id]), ValueSet::new().with("correlationVector", cv));
        assert_eq!(send(&world.h, raw(opening)).await, result(0), "contact {id}");
    }
    assert_eq!(contact_names(&world).len(), 2);

    // Continuing a session (same syncID) it is not read.
    send(&world.h, metadata(ValueSet::new().with("syncID", "s").with("contactIds", vec![1i64, 2, 9]))).await;
    let continuing = with(raw_item(&[9]), ValueSet::new().with("syncID", "s"));
    assert_eq!(send(&world.h, raw(continuing)).await, result(0));
    assert_eq!(contact_names(&world).len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_metadata_message_with_a_non_string_correlation_vector_is_processed() {
    let world = world();
    seed(&world, contacts_batch(&[1, 2])).await;

    let payload = base_metadata(
        ValueSet::new()
            .with("correlationVector", 5i32)
            .with("contactIds", vec![2i64, 3])
            .with("contactChecksums", vec![20i64, 30])
            .with("contactSettings", ValueSet::new().with("contactCapabilities", 1i32))
            .with("sequenceNumbers", seq([6, -1, -1, -1, -1, -1])),
    );
    let answer = send(&world.h, raw(payload)).await;

    // Reconciled as any metadata message; the answer carries the value read (none) as Empty.
    assert_eq!(answer.get("correlationVector"), Some(&Value::Empty));
    assert_eq!(answer.str("contentType"), Some("contactsV2"));
    assert_eq!(answer.i32("itemCount"), Some(2));
    let mut requested = answer.i64_array("contactIds").unwrap().to_vec();
    requested.sort_unstable();
    assert_eq!(requested, [2, 3], "2 has no photo, 3 is new");
    assert_eq!(answer.i32("result"), Some(0));
    assert_eq!(world.h.messenger.responses().last().unwrap().header("result").as_deref(), Some("0"));
    assert_eq!(contact_names(&world), [(2, "Contact 2".into())], "1 deleted by omission");
    assert_eq!(world.h.app.phone_record().unwrap().contacts_capabilities, Some(1));

    // No syncID and no envelope header: an item keyed the same way (a payload value of another
    // type is not a correlation id) continues the session and completes it.
    let continuing = with(raw_item(&[2, 3]), ValueSet::new().with("correlationVector", 7i32));
    assert_eq!(send(&world.h, raw(continuing)).await, result(0));
    assert_eq!(sync_state(&world).current[0], 6, "the session's sequence numbers");
}

#[tokio::test(start_paused = true)]
async fn ignored_shapes_are_not_rejections() {
    let world = world();
    // A batch or sequenceNumbers that is not an Object, and a batch with itemCount 0 and no arrays.
    let msg = item(
        ValueSet::new()
            .with("contacts", 5i32)
            .with("phoneNumbers", ValueSet::new().with("itemCount", 0i32))
            .with("sequenceNumbers", Value::Int64(4)),
    );
    assert_eq!(send(&world.h, msg).await, result(0));
    assert_eq!(sync_state(&world).current, [-1; 6]);
    assert!(sync_state(&world).last_updated.is_some(), "an item on its own completes");
}

// ------------------------------------------------------------------------------------------------
// Persistence
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn records_sequence_numbers_and_state_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let world = world_at(dir.path());
        let push = item(
            ValueSet::new()
                .with(
                    "contacts",
                    with(
                        contacts_batch(&[1]),
                        ValueSet::new().with("thumbnailSizes", vec![1i64]).with("thumbnails", vec![9u8]),
                    ),
                )
                .with("phoneNumbers", phone_batch(&[(2, 1, "+15550102", "")]))
                .with("sequenceNumbers", seq([4, 5, -1, -1, -1, -1])),
        );
        send(&world.h, push).await;
        world.contacts.set_pc_consent(&phone(), false).unwrap();
    }

    let world = world_at(dir.path());
    assert_eq!(contact_names(&world), [(1, "Contact 1".into())]);
    assert_eq!(world.contacts.thumbnail(&phone(), 1).unwrap(), Some(vec![9]));
    assert_eq!(sync_state(&world).current, [4, 5, -1, -1, -1, -1]);
    assert!(!world.contacts.permission_state(&phone()).unwrap().pc_consent);

    // The display-less phone number still makes its owner requested.
    let answer = send(
        &world.h,
        metadata(
            ValueSet::new()
                .with("contactIds", vec![1i64])
                .with("contactChecksums", vec![10i64])
                .with("phoneNumberIds", vec![2i64])
                .with("phoneNumberChecksums", vec![20i64]),
        ),
    )
    .await;
    assert_eq!(answer.i64_array("contactIds").unwrap(), [1]);
    assert_eq!(answer.i64_array("phoneNumberIds").unwrap(), [] as [i64; 0]);
}
