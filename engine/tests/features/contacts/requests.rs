//! The PC's contacts requests and the local views: the outbound `/contacts/v2` sync request, the
//! `/permission` request for contacts, startup, reconnect and triggers, gating and reachability,
//! the local lookup by address, and the local state the protocol depends on.

use crate::support::app_world;
use crate::support::common::{eventually, payload};
use crate::support::contacts_world;
use crate::support::feature_world;
use phonelink_engine::config::features::ContactsConfig;

use std::sync::Arc;
use std::time::Duration;

use app_world::*;
use contacts_world::*;
use phonelink_engine::app::consts::permission;
use phonelink_engine::app::{AppOutcome, ConnectKind};
use phonelink_engine::codec::valueset::{Value, ValueSet};
use phonelink_engine::edge::testing::RegionHost;
use phonelink_engine::features::contacts::{
    BirthdayBanner, CivilDate, ContactsEvent, ContactsPrompt, PermissionPromptResult,
};
use phonelink_engine::features::{ContactDirectory, FeatureError};
use phonelink_engine::seams::PlatformRequest;

fn trace_state<'a>(req: &'a PlatformRequest, k: &str) -> Option<&'a str> {
    req.trace.trace_state.iter().find(|(name, _)| name == k).map(|(_, v)| v.as_str())
}

/// Answers each request by its route.
fn respond_by_route(h: &Harness, f: impl Fn(&str) -> Reply + Send + 'static) {
    h.messenger.set_responder(move |req| f(req.message.route().unwrap_or_default()));
}

fn permitted(world: &World) -> bool {
    world.h.app.phone_record().unwrap().permission_or(permission::CONTACTS, true)
}

// ------------------------------------------------------------------------------------------------
// /contacts/v2
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_sync_request_on_the_wire() {
    let world = world();
    assert_eq!(world.contacts.sync(true, "Test").await.unwrap(), AppOutcome::Success);

    let reqs = world.h.messenger.requests_on("/contacts/v2");
    assert_eq!(reqs.len(), 1);
    let req = &reqs[0];
    let headers: Vec<_> = req.message.headers.iter().map(|(k, _)| k).collect();
    assert_eq!(headers, ["_requestId", "_route"], "no extra headers");

    let vs = payload(req);
    assert_eq!(
        vs.keys().collect::<Vec<_>>(),
        [
            "correlationVector",
            "contentType",
            "subcontentTypes",
            "configuration",
            "isuserinitiated",
            "ignoreResponse",
            "transportType",
            "displayName",
            "appVersion",
            "installationId",
            "contractVersion"
        ]
    );
    let cv = vs.str("correlationVector").unwrap();
    assert_eq!(cv, req.trace.correlation_id, "the request's trace correlation id");
    assert_eq!(cv.len(), 16);
    assert_eq!(vs.str("contentType"), Some("contactsV2"));
    assert_eq!(
        vs.string_array("subcontentTypes").unwrap(),
        [
            "contactsV2",
            "contactThumbs",
            "phoneNumbers",
            "emailAddresses",
            "postalAddresses",
            "contactDates",
            "contactUrls"
        ]
    );
    let config = vs.object("configuration").unwrap();
    assert_eq!((config.bool("ContactsV2Sync"), config.bool("ContactThumbnails")), (Some(true), Some(true)));
    assert_eq!(vs.bool("isuserinitiated"), Some(true));
    assert_eq!(vs.bool("ignoreResponse"), Some(false));
    assert_eq!(vs.i32("transportType"), Some(0));
    assert_eq!(vs.f64("contractVersion"), Some(3.99));
    assert_eq!(req.priority, 0);
    assert!(req.options.ignore_device_disabled);
    assert_eq!(trace_state(req, "ypTriggerId"), Some("Test"));
    assert_eq!(trace_state(req, "ypScenarioId"), Some("SyncAllContacts"));
    assert_eq!(trace_state(req, "isuserinitiated"), Some("True"));

    world.contacts.sync(false, "CallLogRefresh").await.unwrap();

    let req = world.h.messenger.requests_on("/contacts/v2").pop().unwrap();
    assert_eq!(payload(&req).bool("isuserinitiated"), Some(false));
    assert_eq!(req.priority, 100);
}

#[tokio::test(start_paused = true)]
async fn the_sync_outcome_table() {
    // (response result, contractVersion, permission afterwards (None = unchanged), set up once)
    type Case = (Option<i32>, Option<f64>, Option<bool>, bool);
    let cases: Vec<Case> = vec![
        (Some(0), None, Some(true), true),
        (Some(10), None, Some(true), true),
        (Some(11), None, Some(true), true),
        (Some(16), None, Some(true), true),
        (Some(4), None, Some(true), true),
        (Some(3), Some(3.5), Some(true), true),
        (Some(3), Some(2.0), None, false),
        (Some(3), Some(4.0), None, false),
        (Some(5), None, Some(false), false),
        (Some(6), None, Some(false), false),
        (Some(7), None, Some(false), false),
        (Some(9), None, None, false),
        (Some(1), None, None, false),
        (Some(2), None, None, false),
        (Some(13), None, None, false),
        (None, None, None, false),
    ];
    for (code, contract, expected, setup) in cases {
        for initial in [false, true] {
            let world = world();
            world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, initial));

            let mut values = ValueSet::new();
            if let Some(code) = code {
                values.insert("result", code);
            }
            if let Some(v) = contract {
                values.insert("contractVersion", v);
            }

            world.h.messenger.answer_all(response_with(values));
            world.contacts.sync(false, "Test").await.unwrap();

            assert_eq!(permitted(&world), expected.unwrap_or(initial), "result {code:?}, initially {initial}");
            assert_eq!(
                world.contacts.permission_state(&phone()).unwrap().setup_completed_once,
                setup,
                "result {code:?}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_or_failed_sync_changes_nothing() {
    let world = world();
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    world.h.messenger.set_responder(|_| Reply::Hang);

    let started = tokio::time::Instant::now();
    assert_eq!(world.contacts.sync(false, "Test").await.unwrap(), AppOutcome::TimedOut);
    assert_eq!(started.elapsed(), Duration::from_secs(120));
    assert!(!permitted(&world));
    assert_eq!(world.h.messenger.requests_on("/contacts/v2").len(), 1, "one attempt, no retry");
}

#[tokio::test(start_paused = true)]
async fn the_directory_sends_the_sync_without_waiting_and_applies_its_outcome() {
    let world = world();
    world.h.messenger.answer_all(result_response(5));
    let directory: Arc<dyn ContactDirectory> = world.contacts.directory();
    directory.request_sync(true);
    world.h.messenger.wait_requests(1).await;

    let req = world.h.messenger.requests_on("/contacts/v2").pop().unwrap();
    assert_eq!(payload(&req).bool("isuserinitiated"), Some(true));
    assert!(eventually(|| !permitted(&world)).await, "condition never held");
}

#[tokio::test(start_paused = true)]
async fn no_sync_request_at_startup_or_connect() {
    let world = world();
    world.h.app.start().unwrap();
    world.h.messenger.wait_requests(1).await;

    tokio::time::sleep(Duration::from_secs(30)).await;
    world.h.app.send_connect(ConnectKind::Immediate, "Refresh").await;
    tokio::time::sleep(Duration::from_secs(30)).await;

    assert!(!world.h.messenger.requests_on("/connect/v1").is_empty());
    assert!(world.h.messenger.requests_on("/contacts/v2").is_empty());
}

// ------------------------------------------------------------------------------------------------
// /permission
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_permission_prompt_then_the_sync() {
    let world = world();
    respond_by_route(&world.h, |route| match route {
        "/permission" => Reply::Now(result_response(4)),
        _ => Reply::Now(result_response(0)),
    });

    let r = world.contacts.request_device_permission().await.unwrap();
    assert_eq!(
        r,
        PermissionPromptResult { outcome: AppOutcome::Accepted, timed_out: false, sync: Some(AppOutcome::Success) }
    );

    let reqs = world.h.messenger.requests();
    let routes: Vec<_> = reqs.iter().map(|r| r.message.route().unwrap()).collect();
    assert_eq!(routes, ["/permission", "/contacts/v2"]);

    let perm = &reqs[0];
    let vs = payload(perm);
    assert_eq!(vs.str("contentType"), Some("permission"));
    assert_eq!(vs.get("permissionType"), Some(&Value::Int32(2048)));
    assert_eq!(vs.get("silent"), Some(&Value::Bool(false)));
    assert_eq!(vs.str("permissionSessionId"), Some(world.h.app.app_session_id()));
    assert_eq!(vs.str("correlationVector"), Some(perm.trace.correlation_id.as_str()));
    assert!(!vs.contains_key("isuserinitiated"), "a KVP request: user-initiated by its options");
    assert_eq!(perm.priority, 0);
    assert_eq!(trace_state(perm, "isuserinitiated"), Some("True"));
    assert_eq!(trace_state(perm, "ypScenarioId"), Some("ContactsDevicePermission"));

    let sync = &reqs[1];
    assert_eq!(payload(sync).bool("isuserinitiated"), Some(false));
    assert_eq!(
        sync.trace.parent_id.as_deref(),
        Some(perm.trace.correlation_id.as_str()),
        "a child of the prompt's trace"
    );

    assert!(permitted(&world));
    assert!(world.contacts.permission_state(&phone()).unwrap().setup_completed_once);
}

#[tokio::test(start_paused = true)]
async fn a_denied_prompt_sends_no_sync() {
    let world = world();
    world.h.messenger.answer_all(result_response(5));

    let r = world.contacts.request_device_permission().await.unwrap();
    assert_eq!(r, PermissionPromptResult { outcome: AppOutcome::Denied, timed_out: false, sync: None });
    assert!(!permitted(&world));
    assert!(world.h.messenger.requests_on("/contacts/v2").is_empty());
    assert_eq!(world.contacts.prompt(&phone()).unwrap(), ContactsPrompt::DeviceConsentRequired);
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_prompt_changes_only_the_prompt() {
    // The permission is still at its default (true) after a timeout: the sync follows.
    let first = world();
    respond_by_route(&first.h, |route| match route {
        "/permission" => Reply::Hang,
        _ => Reply::Now(result_response(0)),
    });

    let r = first.contacts.request_device_permission().await.unwrap();
    assert_eq!(
        r,
        PermissionPromptResult { outcome: AppOutcome::TimedOut, timed_out: true, sync: Some(AppOutcome::Success) }
    );

    // A denied permission stays denied: no sync.
    let world = world();
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    world.h.messenger.set_responder(|_| Reply::Hang);

    let r = world.contacts.request_device_permission().await.unwrap();
    assert_eq!(r, PermissionPromptResult { outcome: AppOutcome::TimedOut, timed_out: true, sync: None });
    assert!(!permitted(&world));
    assert!(world.h.messenger.requests_on("/contacts/v2").is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_permission_prompt_times_out_after_120_s_with_one_attempt() {
    let world = world();
    // Denied beforehand, so no sync follows the timeout and only the prompt's wait is measured.
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    world.h.messenger.set_responder(|_| Reply::Hang);

    let started = tokio::time::Instant::now();
    let r = world.contacts.request_device_permission().await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(120));
    assert_eq!(r.outcome, AppOutcome::TimedOut);
    assert_eq!(world.h.messenger.requests_on("/permission").len(), 1, "one attempt, no retry");
}

#[tokio::test(start_paused = true)]
async fn the_prompt_is_offered_exactly_for_a_contacts_capable_phone() {
    // The app has no contacts browser; its prompts (the composer's, Settings › Messages') send
    // `/permission` 2048 for every phone the state the UI reads calls contacts-capable, and for no
    // other (Phone Link on Windows asks from its contacts view).
    let world = world();
    assert!(world.contacts.permission_state(&phone()).unwrap().contacts_capable);

    let r = world.contacts.request_device_permission().await.unwrap();
    assert_eq!(r.outcome, AppOutcome::Success);
    assert_eq!(world.h.messenger.requests_on("/permission").len(), 1);

    let world = world_with_custom_data(r#"{"yppCapabilities":32,"registeredPermissionTypes":"1"}"#);
    assert!(!world.contacts.permission_state(&phone()).unwrap().contacts_capable);

    assert!(matches!(world.contacts.request_device_permission().await, Err(FeatureError::Programming(_))));
    assert!(world.h.messenger.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn inbound_sync_the_connect_contribution_and_the_view_syncs_need_no_prompt() {
    // Inbound processing, the connect contribution and the other views' `/contacts/v2` run whether
    // or not a prompt was ever shown.
    let world = world();
    let push = item(ValueSet::new().with("contacts", contacts_batch(&[1])).with("sequenceNumbers", seq([3; 6])));
    assert_eq!(send(&world.h, push).await, result(0));
    assert_eq!(world.contacts.contacts(&phone()).unwrap().len(), 1);

    world.h.app.send_connect(ConnectKind::Background, "Test").await;
    let connect = payload(&world.h.messenger.requests_on("/connect/v1").pop().unwrap());
    assert_eq!(connect.object("configuration").unwrap().bool("ContactsV2Sync"), Some(true));
    assert_eq!(connect.object("sequenceNumbers").unwrap().get("contact"), Some(&Value::Int64(3)));

    world.contacts.sync(true, "Test").await.unwrap();
    assert_eq!(world.h.messenger.requests_on("/contacts/v2").len(), 1);
    assert!(world.h.messenger.requests_on("/permission").is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_prompt_follows_consent_permission_and_setup() {
    let world = world();
    let mut rx = world.contacts.subscribe();
    let prompt = || world.contacts.prompt(&phone()).unwrap();
    assert_eq!(prompt(), ContactsPrompt::None, "consent and permission default to true");

    world.contacts.set_pc_consent(&phone(), false).unwrap();
    assert_eq!(prompt(), ContactsPrompt::PcConsentRequired);
    assert_eq!(drain(&mut rx), [ContactsEvent::PromptChanged { phone: phone() }]);
    world.contacts.set_pc_consent(&phone(), false).unwrap();
    assert!(drain(&mut rx).is_empty(), "no change, no event");

    world.contacts.set_pc_consent(&phone(), true).unwrap();
    drain(&mut rx);

    // The phone reports the contacts permission denied (the shared `permissions` push).
    let push = phone_content(
        "permissions",
        ValueSet::new()
            .with("itemCount", 1i32)
            .with("permissionTypes", vec![0x800i32])
            .with("permissionStates", vec![1i32]),
    );

    send(&world.h, push).await;
    assert_eq!(prompt(), ContactsPrompt::DeviceConsentRequired);

    let mut seen = Vec::new();
    eventually(|| {
        seen.extend(drain(&mut rx));
        seen.contains(&ContactsEvent::PromptChanged { phone: phone() })
    })
    .await;

    // A successful sync grants it and marks contacts set up once; a later denial is "denied".
    world.contacts.sync(false, "Test").await.unwrap();
    assert_eq!(prompt(), ContactsPrompt::None);

    world.h.messenger.answer_all(result_response(6));
    world.contacts.sync(false, "Test").await.unwrap();
    assert_eq!(prompt(), ContactsPrompt::DeviceConsentDenied);
    let state = world.contacts.permission_state(&phone()).unwrap();
    assert!(state.contacts_capable);
    assert_eq!(state.contacts_capabilities, 0);
}

#[tokio::test(start_paused = true)]
async fn the_permissions_content_sets_the_contacts_permission() {
    let world = world();
    let push = |state: i32| {
        phone_content(
            "permissions",
            ValueSet::new()
                .with("itemCount", 1i32)
                .with("permissionTypes", vec![0x800i32])
                .with("permissionStates", vec![state]),
        )
    };

    // Any state but Granted (0) denies; Granted grants.
    for (state, granted) in [(2, false), (0, true), (4, false), (0, true)] {
        assert_eq!(send(&world.h, push(state)).await, result(0));
        assert_eq!(permitted(&world), granted, "state {state}");
    }
}

// ------------------------------------------------------------------------------------------------
// Lookup by address
// ------------------------------------------------------------------------------------------------

async fn seed_address_book(world: &World) {
    let contacts = with(
        contacts_batch(&[1, 2, 3, 4, 5, 6]),
        ValueSet::new()
            .with("displayNames", vec!["Kim", "Lee", "", "Four", "Zed", "Amy"])
            .with("thumbnailSizes", vec![1i64, 0, 0, 2, 1, 0])
            .with("thumbnails", vec![1u8, 4, 4, 5]),
    );
    let numbers = phone_batch(&[
        (11, 1, "010-1234-5678", "010-1234-5678"),
        (12, 2, "+82 10-9999-0000", ""),
        (13, 3, "+15550100", ""),
        (14, 4, "+1 555 0100", ""),
        (15, 77, "010-7777-7777", ""),
        (16, 5, "+15550200", ""),
        (17, 6, "+15550200", ""),
        (18, 3, "+15550300", ""),
    ]);
    let msg = item(
        ValueSet::new()
            .with("syncID", "book")
            .with("contacts", contacts)
            .with("phoneNumbers", numbers)
            .with("emailAddresses", email_batch(&[(21, 1, "kim@example.com"), (22, 77, "orphan@example.com")])),
    );
    assert_eq!(send(&world.h, msg).await, result(0));
}

#[tokio::test(start_paused = true)]
async fn lookup_normalizes_numbers_with_the_phone_country() {
    let world = world();
    world.h.app.update_phone_record(|r| r.country_code = Some("KR".into()));
    seed_address_book(&world).await;

    let d = world.contacts.directory();
    let id = |address: &str, region: Option<&str>| d.lookup(&phone(), address, region).map(|m| m.contact_id);

    assert_eq!(id("+821012345678", None), Some(1));
    assert_eq!(id("01012345678", None), Some(1), "the address with the phone's country");
    assert_eq!(id("01099990000", None), Some(2), "a stored E.164 number");
    assert_eq!(id("01012345678", Some("US")), None, "the caller's region wins for the address");
    assert_eq!(id("01077777777", None), None, "the owning contact is not stored");
    assert_eq!(id("kim@example.com", None), Some(1));
    assert_eq!(id("KIM@example.com", None), None, "e-mail addresses verbatim");
    assert_eq!(id("orphan@example.com", None), None, "the owning contact is not stored");

    let m = d.lookup(&phone(), "+821012345678", None).unwrap();
    assert_eq!((m.contact_id, m.display_name.as_deref(), m.photo_contact_id), (1, Some("Kim"), Some(1)));

    // Two contacts share +1 555 0100: the one with a name names the match.
    let m = d.lookup(&phone(), "+1 (555) 0100", Some("US")).unwrap();
    assert_eq!((m.contact_id, m.display_name.as_deref(), m.photo_contact_id), (4, Some("Four"), Some(4)));

    // By id while `birthday_text` is on (the default): Zed (5) names it and gives the photo.
    let m = d.lookup(&phone(), "+15550200", None).unwrap();
    assert_eq!((m.contact_id, m.display_name.as_deref(), m.photo_contact_id), (5, Some("Zed"), Some(5)));

    // A new record is found (the index is rebuilt after a change).
    send(
        &world.h,
        item(ValueSet::new().with("syncID", "more").with("phoneNumbers", phone_batch(&[(16, 2, "02-123-4567", "")]))),
    )
    .await;
    assert_eq!(id("021234567", None), Some(2));
}

#[tokio::test(start_paused = true)]
async fn an_address_whose_contacts_have_no_name_resolves_without_a_name() {
    // The lookup by address: the view shows the first non-empty display name among the
    // address's contacts, else the address — an empty name is no name.
    for birthday_text in [true, false] {
        let world = world_with(ContactsConfig { birthday_text, ..ContactsConfig::default() });
        seed_address_book(&world).await;
        let m = world.contacts.directory().lookup(&phone(), "+15550300", None).unwrap();
        assert_eq!((m.contact_id, m.display_name, m.photo_contact_id), (3, None, None), "contact 3 has no name");
    }
}

#[tokio::test(start_paused = true)]
async fn lookup_takes_shared_addresses_by_name_while_birthday_text_is_off() {
    let world = world_with(ContactsConfig { birthday_text: false, ..ContactsConfig::default() });
    seed_address_book(&world).await;

    // Amy names it; Zed, the first with a photo in display-name order, gives the photo.
    let m = world.contacts.directory().lookup(&phone(), "+15550200", None).unwrap();
    assert_eq!((m.contact_id, m.display_name.as_deref(), m.photo_contact_id), (6, Some("Amy"), Some(5)));
}

#[tokio::test(start_paused = true)]
async fn lookup_falls_back_to_the_pc_region() {
    let h = Harness::new();
    let w = feature_world::feature_world(&h);
    let mut env = w.env.clone();
    env.edges.host = Arc::new(RegionHost { name: "test-mac.local".into(), region: "KR".into() });
    let contacts = install_on(&env);
    let world = World { h, w, contacts };

    seed_address_book(&world).await;
    assert_eq!(world.h.app.phone_record().unwrap().country_code, None);

    let d = world.contacts.directory();
    assert_eq!(d.lookup(&phone(), "+821012345678", None).map(|m| m.contact_id), Some(1));

    // The phone's own country, once known, wins over the PC's region: the stored national number
    // no longer normalizes to the Korean E.164 form.
    world.h.app.update_phone_record(|r| r.country_code = Some("US".into()));
    assert_eq!(d.lookup(&phone(), "+821012345678", None), None);
}

// ------------------------------------------------------------------------------------------------
// Views
// ------------------------------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn suggestions_search_the_numbers_while_the_access_is_granted() {
    // The composer suggests recipients from the stored contacts only while PC consent is on and the
    // device permission is `true` (differs from Phone Link on Windows).
    let world = world();
    seed_address_book(&world).await;

    let numbers = |w: &World| -> Vec<(String, String, bool)> {
        w.contacts
            .suggestion_numbers(&phone())
            .unwrap()
            .into_iter()
            .map(|n| (n.display_name, n.number, n.has_thumbnail))
            .collect()
    };
    let row = |name: &str, number: &str, photo: bool| (name.to_string(), number.to_string(), photo);

    // By display name, contact id, then number id; the number of a contact that is not stored (77)
    // is left out.
    assert_eq!(
        numbers(&world),
        [
            row("", "+15550100", false),
            row("", "+15550300", false),
            row("Amy", "+15550200", false),
            row("Four", "+1 555 0100", true),
            row("Kim", "010-1234-5678", true),
            row("Lee", "+82 10-9999-0000", false),
            row("Zed", "+15550200", true),
        ]
    );

    world.contacts.set_pc_consent(&phone(), false).unwrap();
    assert!(numbers(&world).is_empty(), "PC consent off");

    world.contacts.set_pc_consent(&phone(), true).unwrap();
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    assert!(numbers(&world).is_empty(), "the phone does not allow contacts");

    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, true));
    assert_eq!(numbers(&world).len(), 7);
}

#[tokio::test(start_paused = true)]
async fn the_contact_list_and_detail() {
    let world = world();
    seed_address_book(&world).await;

    let list = world.contacts.contacts(&phone()).unwrap();
    let names: Vec<_> = list.iter().map(|c| (c.display_name.as_str(), c.has_thumbnail)).collect();
    assert_eq!(names, [("", false), ("Amy", false), ("Four", true), ("Kim", true), ("Lee", false), ("Zed", true)]);

    let kim = world.contacts.contact(&phone(), 1).unwrap().unwrap();
    assert_eq!(kim.phone_numbers.len(), 1);
    assert_eq!(kim.email_addresses[0].address, "kim@example.com");
    assert_eq!(world.contacts.contact(&phone(), 77).unwrap(), None);
}

#[tokio::test(start_paused = true)]
async fn the_contacts_view_lists_and_shows_contacts_while_the_access_is_granted() {
    let world = world();
    seed_address_book(&world).await;

    let listed = |w: &World| -> Vec<(i64, String, Vec<String>)> {
        w.contacts.listed_contacts(&phone()).unwrap().into_iter().map(|c| (c.id, c.name, c.terms)).collect()
    };

    let list = listed(&world);
    assert_eq!(list.iter().map(|c| c.0).collect::<Vec<_>>(), [3, 6, 4, 1, 2, 5], "by display name, then id");
    assert_eq!(
        list[3],
        (1, "Kim".to_string(), vec!["010-1234-5678".into(), "010-1234-5678".into(), "kim@example.com".into()])
    );

    let kim = world.contacts.card(&phone(), 1).unwrap().unwrap();
    assert_eq!((kim.name.as_str(), kim.has_thumbnail), ("Kim", true));
    assert_eq!(kim.phone_numbers[0].shown, "010-1234-5678");
    assert_eq!(kim.email_addresses[0].value, "kim@example.com");
    assert_eq!(world.contacts.card(&phone(), 77).unwrap(), None, "a number's owner that is not stored");

    world.contacts.set_pc_consent(&phone(), false).unwrap();
    assert!(listed(&world).is_empty(), "PC consent off: the prompt stands in for the list");
    assert_eq!(world.contacts.card(&phone(), 1).unwrap(), None);

    world.contacts.set_pc_consent(&phone(), true).unwrap();
    world.h.app.update_phone_record(|r| r.set_permission(permission::CONTACTS, false));
    assert!(listed(&world).is_empty(), "the phone does not allow contacts");
}

/// The e-mail test: `(?i)\b((mailto:)?[\w.+,-]+@[a-z0-9.-]+\.[a-z0-9.-]+[a-z0-9]*)` matched at index 0
/// over the full length, case-insensitive, with Unicode `\w` and case folding.
#[test]
fn the_email_test_gives_the_expected_answers() {
    use phonelink_engine::features::contacts::lookup::is_email;
    let cases: &[(&str, bool)] = &[
        ("e\u{301}@x.com", true),
        ("x\u{203F}y@x.com", true),
        ("a\u{B2}@x.com", false),
        ("\u{939}\u{93E}@x.com", false),
        ("\u{216B}@x.com", false),
        ("x@\u{212A}a.com", true),
        ("x@\u{17F}a.com", false),
        ("x@\u{130}a.com", false),
        ("x@\u{131}a.com", false),
        ("x@A.COM", true),
        ("\u{1D400}@x.com", false),
        ("a\u{1D400}@x.com", false),
        ("\u{200D}x@x.com", false),
        ("x\u{200D}y@x.com", false),
        ("\u{301}e@x.com", true),
        ("_x@x.com", true),
        ("x@x_y.com", false),
        ("1x@x.com", true),
        ("\u{661}@x.com", true),
        ("\u{B5}@x.com", true),
        ("\u{2B0}@x.com", true),
        ("\u{2160}@x.com", false),
        ("MAILTO:x@y.io", true),
        ("mailto:.dot@y.io", true),
        ("mailto:@y.io", false),
        (".dot@example.com", false),
        ("x@a..", true),
        ("x@a.b-", true),
        ("x@ab.", false),
        ("x@.com", false),
        ("someone@example.com", true),
        ("a.b+c,d-e@mail.example.co.kr", true),
        ("x@example", false),
        ("x y@example.com", false),
        ("x@y@example.com", false),
        ("x@y.com\n", false),
        ("x@y.co\u{301}", false),
        ("\u{3042}@x.com", true),
        ("x@\u{FF41}.com", false),
        ("\u{345}@x.com", true),
        ("x@y.c\u{212A}", true),
        ("\u{1C5}@x.com", true),
        ("mai\u{212A}to:x@y.z", false),
        ("\u{17F}x@y.z", true),
        ("x@\u{1E9E}.com", false),
    ];

    let wrong: Vec<_> = cases.iter().filter(|(s, want)| is_email(s) != *want).collect();
    assert!(wrong.is_empty(), "is_email disagrees with the expected answers on {wrong:?}");

    // Through the lookup: the Kelvin-sign address is an e-mail address, matched verbatim.
    assert!(is_email("kim@\u{212A}orea.com"));
}

/// More date forms the banner reads, each as 2026-10-22 on 2026-10-21 (`yyyy <month> d` needs no
/// separator between the month and the day).
#[test]
fn the_birthday_banner_reads_more_date_forms() {
    let today = CivilDate::new(2026, 10, 21).unwrap();
    for form in [
        "10-22",
        "Oct 22",
        "October 22, 2026",
        "22 October 2026",
        "2026-10-22T00:00:00",
        "2026.10.22",
        "10/22/26",
        "2026 Oct22",
        "2026 Oct.22",
        "2026,Oct22",
        "2026 October22",
    ] {
        assert_eq!(
            phonelink_engine::features::contacts::birthday_banner(form, today),
            BirthdayBanner::Tomorrow,
            "{form}"
        );
    }

    // A day the month does not have shows no banner (another reading could swap its fields:
    // `February 31 26` as 2031-02-26).
    for impossible in ["February 31 26", "31 Feb 2026", "2026 Feb31", "2/30/26"] {
        assert_eq!(
            phonelink_engine::features::contacts::birthday_banner(impossible, today),
            BirthdayBanner::None,
            "{impossible}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn the_birthday_banner() {
    let world = world();
    let dates = ValueSet::new()
        .with("itemCount", 2i32)
        .with("contactDateIds", vec![31i64, 32])
        .with("contactIds", vec![1i64, 1])
        .with("type", vec![2i32, 1])
        .with("displayDate", vec!["2026-10-21", "2026-10-22"]);
    send(&world.h, item(ValueSet::new().with("contacts", contacts_batch(&[1])).with("contactDates", dates))).await;

    let today = CivilDate::new(2026, 10, 21).unwrap();
    assert_eq!(
        world.contacts.birthday_banner(&phone(), 1, today).unwrap(),
        BirthdayBanner::Tomorrow,
        "the Birthday date"
    );
    assert_eq!(world.contacts.birthday_banner(&phone(), 9, today).unwrap(), BirthdayBanner::None);

    let off = world_with(ContactsConfig { birthday_text: false, ..ContactsConfig::default() });
    send(&off.h, item(ValueSet::new().with("contacts", contacts_batch(&[1])))).await;
    assert_eq!(off.contacts.birthday_banner(&phone(), 1, today).unwrap(), BirthdayBanner::None);
}

#[tokio::test(start_paused = true)]
async fn clearing_resets_the_sequence_numbers() {
    let world = world();
    let push =
        item(ValueSet::new().with("contacts", contacts_batch(&[1])).with("sequenceNumbers", seq([5, 5, 5, 5, 5, 5])));
    send(&world.h, push).await;

    let mut rx = world.contacts.subscribe();
    world.contacts.clear(&phone()).unwrap();

    assert!(world.contacts.contacts(&phone()).unwrap().is_empty());
    let state = sync_state(&world);
    assert_eq!(state.current, [-1; 6]);
    assert!(state.up_to_date);

    let events = drain(&mut rx);
    assert!(events.contains(&ContactsEvent::RecordsChanged { phone: phone() }));
    assert!(events.contains(&ContactsEvent::SyncStateChanged { phone: phone() }));

    world.h.app.send_connect(ConnectKind::Background, "Test").await;
    let req = world.h.messenger.requests_on("/connect/v1").pop().unwrap();
    assert_eq!(payload(&req).object("sequenceNumbers").unwrap().get("contact"), Some(&Value::Int64(-1)));
}

// ------------------------------------------------------------------------------------------------
// Install
// ------------------------------------------------------------------------------------------------

#[test]
fn install_needs_a_runtime() {
    let h = Harness::new();
    let w = feature_world::feature_world(&h);
    assert!(matches!(
        phonelink_engine::features::contacts::Contacts::install(&w.env, ContactsConfig::default()),
        Err(FeatureError::Programming(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn install_registers_once() {
    let world = world();
    // The processor factory and the connect provider are taken.
    assert!(matches!(
        phonelink_engine::features::contacts::Contacts::install(&world.w.env, ContactsConfig::default()),
        Err(FeatureError::App(_))
    ));
}

/// The lookup normalizes every phone-supplied `phoneNumbers` value: hostile text must not panic the
/// E.164 normalizer (a deterministic sample of the review's fuzz probe).
#[test]
fn normalizing_arbitrary_phone_numbers_never_panics() {
    use phonelink_engine::features::contacts::lookup::normalize;
    let alphabet: Vec<char> =
        "0123456789+()- .xXeEtT;:=#*/,pPwWaAbBcC٣０１\u{200F}\u{FF0B}extn+ext.tel:phone-context=isub="
            .chars()
            .collect();
    let regions =
        [None, Some("KR"), Some("US"), Some("DE"), Some("GB"), Some("NZ"), Some("BR"), Some("JP"), Some("IN")];
    let mut seed = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    };

    for _ in 0..40_000 {
        let len = next() % 24;
        let s: String = (0..len).map(|_| alphabet[next() % alphabet.len()]).collect();
        let region = regions[next() % regions.len()];
        let input = s.clone();
        assert!(
            std::panic::catch_unwind(move || normalize(&input, region)).is_ok(),
            "normalize panicked on {s:?} / {region:?}"
        );
    }
}
