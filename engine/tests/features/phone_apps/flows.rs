//! Phone apps: the `phoneApps` sync (metadata, phone-app items, recent items, completion), its
//! malformed-content rejections, the connect contribution, the lists the UI shows and the app names
//! and icons other features read.

use phonelink_engine::config::features::PhoneAppsConfig;
use std::sync::Arc;
use std::time::Duration;

use phonelink_engine::app::{ConnectKind, ConnectSendResult, InboundDisposition};
use phonelink_engine::codec::valueset::{self, Value, ValueSet};
use phonelink_engine::features::phone_apps::edge::testing::RecordingAppsHost;
use phonelink_engine::features::phone_apps::{PhoneApps, PhoneAppsError};
use phonelink_engine::features::shared::launch_gates;
use phonelink_engine::types::TransportMessage;

use crate::support::app_world::{ASYNC_TRUST_CUSTOM_DATA, Harness, phone, phone_content, response_with, trace};
use crate::support::feature_world::{FeatureWorld, feature_world};

/// The remoting capabilities of a phone with recent apps.
const RECENT: i32 = 0x1 | 0x2 | 0x800;

struct World {
    h: Harness,
    fw: FeatureWorld,
    apps: Arc<PhoneApps>,
    host: Arc<RecordingAppsHost>,
}

fn world_with(config: PhoneAppsConfig, host: Arc<RecordingAppsHost>) -> World {
    let h = Harness::with_custom_data(Some(ASYNC_TRUST_CUSTOM_DATA));
    let w = feature_world(&h);
    let apps = PhoneApps::install(&w.env, config, host.clone()).expect("phone apps install");
    World { h, fw: w, apps, host }
}

fn world() -> World {
    world_with(PhoneAppsConfig::default(), RecordingAppsHost::new())
}

impl World {
    async fn send(&self, msg: TransportMessage) -> ValueSet {
        match self.h.app.handle_inbound(phone(), trace(), msg).await {
            InboundDisposition::Answered(v) => v,
            other => panic!("not answered: {other:?}"),
        }
    }

    fn set_remoting(&self, flags: i32) {
        self.h.app.update_phone_record(|r| r.set_remoting_flags(flags as u32));
    }

    fn packages(&self) -> Vec<String> {
        self.apps.apps().unwrap().into_iter().map(|a| a.package_name).collect()
    }

    fn record_flags(&self) -> u32 {
        self.h.app.phone_record().unwrap().remoting_flags()
    }
}

fn with(mut a: ValueSet, b: ValueSet) -> ValueSet {
    for (k, v) in b.iter() {
        a.insert(k, v.clone());
    }
    a
}

fn metadata(ids: &[i64], checksums: &[i64], sequence: Option<i64>) -> ValueSet {
    let mut vs =
        ValueSet::new().with("metadata", 1i32).with("phoneAppsIds", ids.to_vec()).with("checksums", checksums.to_vec());
    if let Some(s) = sequence {
        vs.insert("sequenceNumber", s);
    }
    vs
}

/// One phone app of an item message.
#[derive(Clone)]
struct App {
    id: i64,
    package: &'static str,
    name: &'static str,
    checksum: i64,
    icon: Vec<u8>,
}

fn app(id: i64, package: &'static str, name: &'static str, checksum: i64) -> App {
    App { id, package, name, checksum, icon: Vec::new() }
}

fn items(apps: &[App], actions: Option<&[i32]>) -> ValueSet {
    let mut vs = ValueSet::new()
        .with("itemCount", apps.len() as i32)
        .with("phoneAppsIds", apps.iter().map(|a| a.id).collect::<Vec<_>>())
        .with("appPackageNames", apps.iter().map(|a| a.package.to_string()).collect::<Vec<_>>())
        .with("appNames", apps.iter().map(|a| a.name.to_string()).collect::<Vec<_>>())
        .with("appVersions", apps.iter().map(|_| "1.0".to_string()).collect::<Vec<_>>())
        .with("phoneAppsLastUpdatedTimes", apps.iter().map(|_| 1_700_000_000_000i64).collect::<Vec<_>>())
        .with("checksums", apps.iter().map(|a| a.checksum).collect::<Vec<_>>())
        .with("iconSizes", apps.iter().map(|a| a.icon.len() as i64).collect::<Vec<_>>())
        .with("content", apps.iter().flat_map(|a| a.icon.clone()).collect::<Vec<u8>>());
    if let Some(a) = actions {
        vs.insert("actions", a.to_vec());
    }
    vs
}

/// One recent entry of an item message.
#[derive(Clone)]
struct Entry {
    id: i64,
    package: &'static str,
    description: &'static str,
    intent: &'static str,
    rank: i32,
    web: &'static str,
    desktop: &'static str,
}

fn entry(id: i64, package: &'static str, rank: i32) -> Entry {
    Entry { id, package, description: "", intent: "android.intent.action.MAIN", rank, web: "", desktop: "" }
}

fn recent(entries: &[Entry], actions: Option<&[i32]>) -> ValueSet {
    let s = |f: fn(&Entry) -> &'static str| entries.iter().map(|e| f(e).to_string()).collect::<Vec<_>>();
    let mut vs = ValueSet::new()
        .with("recentAppsItemCount", entries.len() as i32)
        .with("recentAppsIds", entries.iter().map(|e| e.id).collect::<Vec<_>>())
        .with("recentAppsPackageNames", s(|e| e.package))
        .with("recentAppsChecksums", entries.iter().map(|e| e.id * 7).collect::<Vec<_>>())
        .with("recentAppsDescriptions", s(|e| e.description))
        .with("recentAppsIntentActions", s(|e| e.intent))
        .with("recentAppsTaskIds", entries.iter().map(|e| e.id + 1000).collect::<Vec<_>>())
        .with("recentAppsRanks", entries.iter().map(|e| e.rank).collect::<Vec<_>>())
        .with("recentAppsWebDocumentUris", s(|e| e.web))
        .with("recentAppsWindowsDocumentUris", s(|e| e.desktop));
    if let Some(a) = actions {
        vs.insert("recentAppsActions", a.to_vec());
    }
    vs
}

fn ok() -> ValueSet {
    ValueSet::new().with("result", 0i32)
}

fn rejected(reason: &str) -> ValueSet {
    ValueSet::new().with("result", 1i32).with("message", format!("{reason} 80004005"))
}

fn settings(flags: i32) -> ValueSet {
    ValueSet::new().with("phoneAppsSettings", ValueSet::new().with("phoneAppsCapabilities", flags))
}

/// Stores `apps` with an incremental push (no metadata).
async fn push(w: &World, apps: &[App]) {
    assert_eq!(w.send(phone_content("phoneApps", items(apps, None))).await, ok());
}

#[tokio::test(start_paused = true)]
async fn metadata_requests_the_missing_and_changed_ids_once_per_entry() {
    let w = world();
    push(&w, &[app(1, "com.a", "A", 10), app(2, "com.b", "B", 20), app(3, "com.c", "C", 30)]).await;
    assert_eq!(w.packages(), ["com.a", "com.b", "com.c"]);

    let answer = w.send(phone_content("phoneApps", metadata(&[1, 2, 4, 4], &[10, 21, 40, 40], Some(40)))).await;
    assert_eq!(
        answer,
        ValueSet::new()
            .with("correlationVector", "cv-1")
            .with("contentType", "phoneApps")
            .with("contractVersion", 3.99f64)
            .with("itemCount", 3i32)
            .with("phoneAppsIds", vec![2i64, 4, 4])
            .with("result", 0i32)
    );
    // The unlisted app is deleted at once; the changed one stays until its item comes.
    assert_eq!(w.packages(), ["com.a", "com.b"]);
    let st = w.apps.status().unwrap();
    assert_eq!(st.sync.phone_apps.current, -1, "not complete yet");

    // The items of the open sync complete it and record its sequence number.
    assert_eq!(
        w.send(phone_content("phoneApps", items(&[app(2, "com.b", "B2", 21), app(4, "com.d", "D", 40)], None))).await,
        ok()
    );
    let st = w.apps.status().unwrap();
    assert_eq!(st.sync.phone_apps.current, 40);
    assert!(st.sync.last_sync_ms.is_some());
    let names: Vec<_> = w.apps.apps().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["A", "B2", "D"]);

    // An empty list deletes every app and requests nothing: the sync completes at once.
    let answer = w.send(phone_content("phoneApps", metadata(&[], &[], Some(41)))).await;
    assert_eq!(answer.i64_array("phoneAppsIds"), Some(&[][..]));
    assert_eq!(answer.i32("itemCount"), Some(0));
    assert!(w.packages().is_empty());
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.current, 41);
}

#[tokio::test(start_paused = true)]
async fn a_sync_stays_open_while_an_id_is_pending() {
    let w = world();
    w.send(phone_content("phoneApps", metadata(&[1, 2], &[10, 20], Some(5)))).await;
    push(&w, &[app(1, "com.a", "A", 10)]).await;

    let st = w.apps.status().unwrap();
    // No recent id is pending, so the sequence number is recorded already; the sync is not complete.
    assert_eq!(st.sync.phone_apps.current, 5);
    assert_eq!(st.sync.last_sync_ms, None, "id 2 still pending");

    push(&w, &[app(2, "com.b", "B", 20)]).await;
    assert!(w.apps.status().unwrap().sync.last_sync_ms.is_some());

    // The sync is over: a later item message is processed on its own and records no sequence number.
    w.h.app.update_phone_record(|r| r.set_remoting_flags(0));
    push(&w, &[app(3, "com.c", "C", 30)]).await;
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.current, 5);
}

#[tokio::test(start_paused = true)]
async fn items_upsert_by_package_and_delete_with_actions() {
    let w = world();
    let mut a = app(1, "com.a", "A", 1);
    a.icon = vec![0x89, b'P', b'N', b'G'];
    let mut b = app(2, "com.b", "B", 2);
    b.icon = vec![1, 2, 3];

    push(&w, &[a.clone(), b, app(3, "com.c", "C", 3)]).await;

    assert_eq!(w.apps.icon("com.a").unwrap(), Some(vec![0x89, b'P', b'N', b'G']));
    assert_eq!(w.apps.icon("com.b").unwrap(), Some(vec![1, 2, 3]));
    assert_eq!(w.apps.icon("com.c").unwrap(), None);

    // Same package, new id: overwritten; a missing icon clears the stored one.
    push(&w, &[app(11, "com.a", "A new", 5)]).await;
    assert_eq!(w.apps.icon("com.a").unwrap(), None);
    let a_now = w.apps.apps().unwrap().into_iter().find(|x| x.package_name == "com.a").unwrap();
    assert_eq!(a_now.name, "A new");

    // With actions: id 2 deleted, id 3 updated; deleted items carry empty strings.
    let deleted = App { id: 2, package: "", name: "", checksum: 0, icon: Vec::new() };
    let msg = items(&[deleted, app(3, "com.c", "C2", 4)], Some(&[2, 1]));
    assert_eq!(w.send(phone_content("phoneApps", msg)).await, ok());
    let names: Vec<_> = w.apps.apps().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["A new", "C2"]);
}

#[tokio::test(start_paused = true)]
async fn favorites_survive_syncs_and_leave_with_their_app() {
    let w = world();
    push(&w, &[app(1, "com.a", "A", 1), app(2, "com.b", "B", 2), app(3, "com.c", "C", 3)]).await;
    w.apps.add_favorite("com.c").unwrap();
    w.fw.clock.advance(Duration::from_secs(1));
    w.apps.add_favorite("com.a").unwrap();

    let favs = |w: &World| w.apps.favorites().unwrap().into_iter().map(|a| a.package_name).collect::<Vec<_>>();
    assert_eq!(favs(&w), ["com.c", "com.a"], "added last, shown last");

    w.apps.reorder_favorites(&["com.a".into(), "com.c".into()]).unwrap();
    assert_eq!(favs(&w), ["com.a", "com.c"]);
    assert!(matches!(
        w.apps.reorder_favorites(&["com.a".into(), "com.a".into()]),
        Err(PhoneAppsError::InvalidInput(_))
    ));
    assert!(matches!(w.apps.reorder_favorites(&["com.x".into()]), Err(PhoneAppsError::UnknownApp(_))));
    assert_eq!(favs(&w), ["com.a", "com.c"], "a refused reorder writes nothing");

    // A sync that updates both keeps their ranks; one that drops com.c deletes it with its rank.
    w.send(phone_content("phoneApps", metadata(&[1, 2], &[9, 2], None))).await;
    push(&w, &[app(1, "com.a", "A2", 9)]).await;
    assert_eq!(favs(&w), ["com.a"]);
    assert!(w.apps.apps().unwrap().iter().any(|a| a.package_name == "com.a" && a.favorite));

    w.apps.remove_favorite("com.a").unwrap();
    assert!(favs(&w).is_empty());
    assert!(matches!(w.apps.add_favorite("com.gone"), Err(PhoneAppsError::UnknownApp(_))));
}

#[tokio::test(start_paused = true)]
async fn the_list_leaves_out_link_to_windows_and_sorts_and_searches_by_name() {
    let w = world();
    push(
        &w,
        &[
            app(1, "com.microsoft.appmanager", "Link to Windows", 1),
            app(2, "com.z", "zebra Maps", 1),
            app(3, "com.y", "Apple\nNews", 1),
            app(4, "COM.SAMSUNG.ANDROID.MDX", "Samsung", 1),
            app(5, "com.x", "Maps", 1),
        ],
    )
    .await;

    let names: Vec<_> = w.apps.apps().unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Apple News", "Maps", "zebra Maps"]);
    let found: Vec<_> = w.apps.search("MAPS ze").unwrap().into_iter().map(|a| a.name).collect();
    assert_eq!(found, ["zebra Maps"]);

    // The notifications' view of the list: any stored package, by its shown name and icon.
    let dir = w.apps.directory();
    assert_eq!(dir.app(&phone(), "com.y").unwrap().name, "Apple News");
    assert!(dir.app(&phone(), "com.microsoft.appmanager").is_some());
    assert!(dir.app(&phone(), "com.unknown").is_none());

    // A mirroring window's title: the stored raw name, excluded packages included.
    assert_eq!(w.apps.stored_name("com.y").unwrap().as_deref(), Some("Apple\nNews"));
    assert_eq!(w.apps.stored_name("com.microsoft.appmanager").unwrap().as_deref(), Some("Link to Windows"));
    assert_eq!(w.apps.stored_name("com.unknown").unwrap(), None);

    // A host order that is not a permutation fails loudly.
    *w.host.forced_order.lock().unwrap() = Some(vec![0, 0, 1]);
    assert!(matches!(w.apps.apps(), Err(PhoneAppsError::Edge(_))));
}

#[tokio::test(start_paused = true)]
async fn recent_entries_need_their_app_and_replace_the_list_with_actions() {
    let host = RecordingAppsHost::opening(&["ms-word:ofe|u|https://d/x.docx"]);
    let w = world_with(PhoneAppsConfig::default(), host);
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.a", "A", 1), app(2, "com.b", "B", 2), app(3, "com.c", "C", 3)]).await;

    let mut doc = entry(20, "com.b", 1);
    doc.description = "Report.docx";
    doc.web = "https://d/x.docx";
    doc.desktop = "ms-word:ofe|u|https://d/x.docx";

    let mut http_desktop = entry(21, "com.c", 2);
    http_desktop.desktop = "https://d/y.docx";
    let msg = recent(&[entry(10, "com.a", 0), doc, http_desktop, entry(30, "com.unknown", 3)], None);
    assert_eq!(w.send(phone_content("phoneApps", msg)).await, ok());

    let r = w.apps.recent().unwrap();
    assert_eq!(r.iter().map(|e| e.recent_id).collect::<Vec<_>>(), [10, 20, 21], "the entry without an app is skipped");
    assert!(r[0].main_activity);
    assert_eq!(r[0].name, "A");
    assert!(!r[1].main_activity);
    assert_eq!(r[1].name, "Report.docx");
    assert_eq!(r[1].task_id, 1020);
    assert_eq!(r[1].web_uri.as_deref(), Some("https://d/x.docx"));
    assert_eq!(r[1].desktop_uri.as_deref(), Some("ms-word:ofe|u|https://d/x.docx"));
    assert_eq!(r[2].desktop_uri, None, "an http(s) desktop URI is dropped");
    assert!(r[2].main_activity);
    assert_eq!(w.host.queries(), ["ms-word:ofe|u|https://d/x.docx"]);

    // Without actions: upserted on top.
    w.send(phone_content("phoneApps", recent(&[entry(10, "com.a", 5)], None))).await;
    assert_eq!(w.apps.recent().unwrap().iter().map(|e| e.recent_id).collect::<Vec<_>>(), [20, 21, 10]);

    // With actions: the message replaces the list; deleted entries are not stored.
    w.send(phone_content("phoneApps", recent(&[entry(40, "com.c", 0), entry(20, "com.b", 1)], Some(&[0, 2])))).await;
    assert_eq!(w.apps.recent().unwrap().iter().map(|e| e.recent_id).collect::<Vec<_>>(), [40]);

    // Recent apps not supported: the keys are ignored and the list shows none.
    w.set_remoting(0x1);
    assert!(w.apps.recent().unwrap().is_empty());
    assert_eq!(w.send(phone_content("phoneApps", recent(&[entry(50, "com.a", 0)], None))).await, ok());
    w.set_remoting(RECENT);
    assert_eq!(w.apps.recent().unwrap().iter().map(|e| e.recent_id).collect::<Vec<_>>(), [40]);
}

#[tokio::test(start_paused = true)]
async fn the_office_rewrite_turns_document_urls_into_office_uris_when_on() {
    let rewritten = "ms-word:ofv|u|https://d/x.docx";
    let config = PhoneAppsConfig { office_desktop_uri_rewrite: true, ..PhoneAppsConfig::default() };
    let w = world_with(config, RecordingAppsHost::opening(&[rewritten]));
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.microsoft.office.officehubrow", "Office", 1)]).await;

    let mut e = entry(10, "com.microsoft.office.officehubrow", 0);
    e.desktop = "https://d/x.docx";
    let mut pdf = entry(11, "com.microsoft.office.officehubrow", 1);
    pdf.desktop = "https://d/x.pdf";
    pdf.description = "x.pdf";

    w.send(phone_content("phoneApps", recent(&[e, pdf], None))).await;

    let r = w.apps.recent().unwrap();
    assert_eq!(r[0].desktop_uri.as_deref(), Some(rewritten));
    assert_eq!(r[1].desktop_uri, None);
    assert_eq!(w.host.queries(), [rewritten], "only the rewritten URI is checked");

    // A URI no installed application opens is dropped.
    let w = world_with(
        PhoneAppsConfig { office_desktop_uri_rewrite: true, ..PhoneAppsConfig::default() },
        RecordingAppsHost::new(),
    );
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.microsoft.office.officehubrow", "Office", 1)]).await;

    let mut e = entry(10, "com.microsoft.office.officehubrow", 0);
    e.desktop = "https://d/x.docx";

    w.send(phone_content("phoneApps", recent(&[e], None))).await;
    assert_eq!(w.apps.recent().unwrap()[0].desktop_uri, None);
}

#[tokio::test(start_paused = true)]
async fn a_desktop_uri_that_is_not_absolute_refuses_the_message() {
    let w = world_with(PhoneAppsConfig::default(), RecordingAppsHost::opening(&["ms-word:ofe|u|x"]));
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.a", "A", 1)]).await;

    // An entry whose package has no stored app is skipped before its URI matters.
    let mut orphan = entry(10, "com.unknown", 0);
    orphan.desktop = "docs/report.docx";
    assert_eq!(w.send(phone_content("phoneApps", recent(&[orphan], None))).await, ok());

    // With a stored app the whole message is refused: neither its phone apps (including the one that
    // gives the entry its app), nor its recent entries, nor its settings are applied.
    let mut relative = entry(20, "com.b", 1);
    relative.desktop = "docs/report.docx";
    let mut good = entry(21, "com.a", 0);
    good.desktop = "ms-word:ofe|u|x";
    let msg =
        with(with(items(&[app(2, "com.b", "B", 2)], None), recent(&[good, relative], None)), settings(RECENT | 0x4));

    assert_eq!(
        w.send(phone_content("phoneApps", msg)).await,
        rejected("a recent entry's desktop URI is not an absolute URI")
    );

    assert_eq!(w.packages(), ["com.a"]);
    assert!(w.apps.recent().unwrap().is_empty());
    assert_eq!(w.record_flags(), RECENT as u32);
    assert_eq!(w.host.queries(), ["ms-word:ofe|u|x"], "only an absolute URI is checked for an application");

    // An empty desktop URI is none, not a relative one.
    let mut empty = entry(30, "com.a", 0);
    empty.web = "https://d/x.docx";
    assert_eq!(w.send(phone_content("phoneApps", recent(&[empty], None))).await, ok());
    assert_eq!(w.apps.recent().unwrap()[0].desktop_uri, None);
}

#[tokio::test(start_paused = true)]
async fn settings_apply_first_and_decide_the_recent_part() {
    let w = world();
    assert_eq!(w.record_flags(), 0);
    let msg = with(
        with(metadata(&[1], &[10], Some(40)), settings(RECENT)),
        ValueSet::new()
            .with("recentAppsIds", vec![2001i64])
            .with("recentAppsChecksums", vec![6001i64])
            .with("recentAppsSequenceNumber", 70i64),
    );

    let answer = w.send(phone_content("phoneApps", msg)).await;

    assert_eq!(answer.i32("itemCount"), Some(2));
    assert_eq!(answer.i64_array("recentAppsIds"), Some(&[2001i64][..]));
    assert_eq!(w.record_flags(), RECENT as u32, "the record keeps what the message reported");

    // The phone app arrives: the phone-app part is complete, so both sequence numbers are recorded,
    // though the recent id is still pending.
    push(&w, &[app(1, "com.a", "A", 10)]).await;
    let st = w.apps.status().unwrap();
    assert_eq!(st.sync.phone_apps.current, 40);
    assert_eq!(st.sync.recent_apps.current, 70);

    // Settings without an Int32 capability report 0.
    let msg = with(items(&[], None), ValueSet::new().with("phoneAppsSettings", ValueSet::new()));
    assert_eq!(w.send(phone_content("phoneApps", msg)).await, ok());
    assert_eq!(w.record_flags(), 0);
}

#[tokio::test(start_paused = true)]
async fn malformed_messages_change_nothing() {
    let w = world();
    push(&w, &[app(1, "com.a", "A", 1)]).await;

    let cases = [
        (
            with(items(&[app(2, "com.b", "B", 1)], None), ValueSet::new().with("appNames", Vec::<String>::new())),
            "appNames missing or not parallel",
        ),
        (
            with(items(&[app(2, "com.b", "B", 1)], None), ValueSet::new().with("iconSizes", vec![5i64])),
            "icon sizes exceed content",
        ),
        (
            with(items(&[app(2, "com.b", "B", 1)], None), ValueSet::new().with("sequenceNumber", 3i32)),
            "sequenceNumber is not an Int64",
        ),
        (
            with(items(&[app(2, "com.b", "B", 1)], None), ValueSet::new().with("actions", vec![0i32, 2])),
            "actions not parallel",
        ),
        // Another package already holds id 1.
        (items(&[app(1, "com.other", "O", 1)], None), "an item's id is held by another package"),
        (metadata(&[1, 2], &[1], None), "checksums missing or shorter than phoneAppsIds"),
        (
            with(ValueSet::new().with("metadata", 1i32), ValueSet::new().with("phoneAppsIds", vec![1i32])),
            "phoneAppsIds is not an Int64Array",
        ),
    ];

    for (payload, reason) in cases {
        // Each carries settings, which must not be applied either.
        let answer = w.send(phone_content("phoneApps", with(payload, settings(RECENT)))).await;
        assert_eq!(answer, rejected(reason), "{reason}");
        assert_eq!(w.packages(), ["com.a"], "{reason}");
        assert_eq!(w.record_flags(), 0, "{reason}");
    }

    // A rejected item message discards its sync: the next item message is processed on its own.
    w.send(phone_content("phoneApps", metadata(&[1, 3], &[1, 3], Some(9)))).await;

    let bad = with(items(&[app(3, "com.c", "C", 3)], None), ValueSet::new().with("checksums", vec![1i64, 2]));
    assert_eq!(w.send(phone_content("phoneApps", bad)).await, rejected("checksums missing or not parallel"));
    push(&w, &[app(3, "com.c", "C", 3)]).await;
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.current, -1, "the discarded sync's sequence number is gone");
}

#[tokio::test(start_paused = true)]
async fn item_messages_need_their_batch_sizes() {
    let w = world();
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.a", "A", 1)]).await;

    let mut no_count = items(&[app(2, "com.b", "B", 1)], None);
    no_count.remove("itemCount");
    assert_eq!(w.send(phone_content("phoneApps", no_count)).await, rejected("itemCount missing or not an Int32"));
    assert_eq!(w.packages(), ["com.a"]);

    let mut no_recent_count = recent(&[entry(10, "com.a", 1)], None);
    no_recent_count.remove("recentAppsItemCount");
    assert_eq!(
        w.send(phone_content("phoneApps", no_recent_count)).await,
        rejected("recentAppsItemCount missing or not an Int32")
    );
    assert!(w.apps.recent().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_recent_app_carries_its_apps_favorite_and_a_task_never_does() {
    let w = world();
    w.set_remoting(RECENT);
    push(&w, &[app(1, "com.a", "A", 1), app(2, "com.b", "B", 2)]).await;
    w.apps.add_favorite("com.a").unwrap();
    w.apps.add_favorite("com.b").unwrap();

    let mut task = entry(20, "com.b", 2);
    task.description = "Report.docx";
    w.send(phone_content("phoneApps", recent(&[entry(10, "com.a", 1), task], None))).await;

    let r = w.apps.recent().unwrap();
    assert_eq!(
        r.iter().map(|e| (e.recent_id, e.main_activity, e.favorite)).collect::<Vec<_>>(),
        [(10, true, true), (20, false, false)]
    );

    w.apps.remove_favorite("com.a").unwrap();
    assert!(!w.apps.recent().unwrap()[0].favorite, "the flag follows the app");
}

#[tokio::test(start_paused = true)]
async fn an_item_message_on_its_own_needs_a_correlation_vector_and_keeps_its_sequence_number() {
    let w = world();
    let vs = with(
        with(
            ValueSet::new().with("contentType", "phoneApps").with("contractVersion", 3.99f64),
            items(&[app(1, "com.a", "A", 1)], None),
        ),
        ValueSet::new().with("sequenceNumber", 12i64),
    );
    let msg = crate::support::app_world::message(
        &[("_requestId", "req-1"), ("_route", "/legacy/phonecontent")],
        valueset::encode(&vs),
    );

    assert_eq!(w.send(msg).await, rejected("item message without a correlationVector"));
    assert!(w.packages().is_empty());

    let with_cv = with(items(&[app(1, "com.a", "A", 1)], None), ValueSet::new().with("sequenceNumber", 12i64));
    assert_eq!(w.send(phone_content("phoneApps", with_cv)).await, ok());
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.current, 12);
}

fn connect_payload(w: &World) -> ValueSet {
    let reqs = w.h.messenger.requests_on("/connect/v1");
    valueset::decode(&reqs.last().unwrap().message.payload).unwrap()
}

#[tokio::test(start_paused = true)]
async fn the_connect_message_carries_the_stored_state_and_the_reply_sets_expected() {
    let w = world();
    w.h.messenger.answer_all(response_with(
        ValueSet::new()
            .with("result", 0i32)
            .with("phoneApps", 50i64)
            .with("recentApps", 80i64)
            .with("capabilities", ValueSet::new().with("remoting", RECENT | 0x4)),
    ));

    assert!(matches!(w.h.app.send_connect(ConnectKind::Background, "Test").await, ConnectSendResult::Sent(_)));

    let vs = connect_payload(&w);
    let conf = vs.object("configuration").unwrap();
    assert_eq!(conf.bool("RecentApps"), Some(false), "nothing stored before the first reply");
    assert_eq!(conf.bool("AppHandoff"), Some(false));
    assert_eq!(conf.bool("Exp006"), Some(true));

    let seq = vs.object("sequenceNumbers").unwrap();
    assert_eq!(seq.get("phoneApps"), Some(&Value::Int64(-1)));
    assert!(!seq.contains_key("recentApps"));
    // The reply's remoting capabilities came with DeviceInfo, so both expected numbers were read.
    let st = w.apps.status().unwrap();
    assert_eq!(st.sync.phone_apps.expected, 50);
    assert_eq!(st.sync.recent_apps.expected, 80);
    assert!(!st.sync.up_to_date());
    assert!(st.syncing);

    // The next connect sends what the reply taught.
    assert!(matches!(w.h.app.send_connect(ConnectKind::Background, "Test").await, ConnectSendResult::Sent(_)));

    let vs = connect_payload(&w);
    let conf = vs.object("configuration").unwrap();
    assert_eq!(conf.bool("RecentApps"), Some(true));
    assert_eq!(conf.bool("AppHandoff"), Some(true));

    let seq = vs.object("sequenceNumbers").unwrap();
    assert_eq!(seq.get("recentApps"), Some(&Value::Int64(-1)));

    // A reply with another `result` sets nothing; mistyped numbers read -1.
    w.h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32).with("phoneApps", 5i32)));
    w.h.app.send_connect(ConnectKind::Background, "Test").await;
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.expected, -1);

    w.h.messenger.answer_all(response_with(ValueSet::new().with("result", 2i32).with("phoneApps", 9i64)));
    w.h.app.send_connect(ConnectKind::Background, "Test").await;
    assert_eq!(w.apps.status().unwrap().sync.phone_apps.expected, -1);
}

#[tokio::test(start_paused = true)]
async fn the_syncing_state_shows_for_at_most_two_minutes() {
    let w = world();
    w.h.messenger.answer_all(response_with(ValueSet::new().with("result", 0i32).with("phoneApps", 7i64)));
    w.h.app.send_connect(ConnectKind::Background, "Test").await;
    assert!(w.apps.status().unwrap().syncing);

    // The timer starts once its task runs.
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(119)).await;
    tokio::task::yield_now().await;
    assert!(w.apps.status().unwrap().syncing);

    tokio::time::advance(Duration::from_secs(2)).await;
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }

    let st = w.apps.status().unwrap();
    assert!(!st.syncing, "shown at most 2 minutes");
    assert!(!st.sync.up_to_date());

    // Catching up clears it for good.
    w.send(phone_content("phoneApps", metadata(&[], &[], Some(7)))).await;
    assert!(w.apps.status().unwrap().sync.up_to_date());
}

#[tokio::test(start_paused = true)]
async fn the_apps_view_follows_the_gates() {
    let w = world();
    assert!(!w.apps.status().unwrap().apps_view_visible, "a phone without an OEM integration");
    w.h.app.update_phone_record(|r| {
        r.client_model_name = Some(launch_gates::client_model::SG);
        r.client_model_revision = Some(1);
    });

    let st = w.apps.status().unwrap();
    assert!(st.apps_view_visible && st.phone_screen_apps && st.remoting_supported);
    assert!(!st.hosted_apps);

    w.set_remoting(0x1);
    assert!(w.apps.status().unwrap().hosted_apps, "Android 14, contract 3.99");
}
