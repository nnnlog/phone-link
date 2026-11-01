use super::*;
use phonelink_core::edge::testing::ManualClock;
use phonelink_core::types::Environment;
use phonelink_features::shared::db::StoreLocation;
use phonelink_features::shared::notifier::testing::RecordingNotifier;
use std::sync::Weak;
use std::time::Duration;

/// A photos core without an App layer, over in-memory stores and `runtime`.
fn bare_inner(runtime: Handle) -> Arc<PhotosInner> {
    Arc::new(PhotosInner {
        config: PhotosConfig::default(),
        app: Weak::new(),
        clock: Arc::new(ManualClock::fixed()),
        notifier: RecordingNotifier::new(),
        stores: PhoneStores::new(StoreLocation::Memory, store::FEATURE, store::MIGRATIONS),
        phones: Mutex::new(HashMap::new()),
        last_view: Mutex::new(None),
        events: broadcast::channel(16).0,
        runtime,
        shutdown: CancellationToken::new(),
        new_photo_gate: OnceLock::new(),
    })
}

/// The connect task stores `expected` and refreshes; the inbound worker completes the sync and
/// refreshes, finding nothing to change. Whatever the interleaving, the indicator must end as the
/// store says: the connect task's refresh reads the state only once it holds the indicator.
#[test]
fn the_syncing_state_is_read_under_the_indicator_lock() {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let inner = bare_inner(rt.handle().clone());
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    let ps = inner.phone_state(&phone).unwrap();
    assert_eq!(ps.ui.lock_or_panic().indicator.up_to_date, Some(true));

    store::set_expected(&ps.db, 5).unwrap();
    let held = ps.ui.lock_or_panic();
    let (connect_inner, connect_ps) = (inner.clone(), ps.clone());
    let connect_refresh = std::thread::spawn(move || connect_inner.refresh_sync_state(&connect_ps));

    // Let the connect task's refresh run as far as it can while the indicator is held elsewhere.
    std::thread::sleep(Duration::from_millis(200));

    // Meanwhile the sync completes (current = 5); its own refresh found the indicator already
    // "up to date" and changed nothing.
    let completed = wire::Metadata {
        correlation_vector: "cv".into(),
        ids: Vec::new(),
        times: Vec::new(),
        checksums: None,
        settings: None,
        sequence_number: Some(5),
        repeated: 0,
    };
    assert!(store::reconcile(&ps.db, &completed, None, 0).unwrap().outgoing.is_empty());
    drop(held);
    connect_refresh.join().unwrap();

    assert!(store::state(&ps.db).unwrap().up_to_date());
    let ui = ps.ui.lock_or_panic();
    assert_eq!(ui.indicator.up_to_date, Some(true), "the indicator follows the store");
    assert!(!ui.indicator.updating, "up to date: no syncing indicator");
}

#[test]
fn save_as_names() {
    assert_eq!(save_as_name("IMG_1.jpeg"), SaveAsName { stem: "IMG_1".into(), extension: ".jpeg".into() });
    assert_eq!(save_as_name("IMG_1"), SaveAsName { stem: "IMG_1".into(), extension: ".jpg".into() });
    assert_eq!(save_as_name("IMG_1."), SaveAsName { stem: "IMG_1".into(), extension: ".jpg".into() });
    assert_eq!(save_as_name(""), SaveAsName { stem: "Photo".into(), extension: ".jpg".into() });
    assert_eq!(save_as_name("DCIM/a.png"), SaveAsName { stem: "a".into(), extension: ".png".into() });
}

#[test]
fn config_validation() {
    assert!(validate_config(&PhotosConfig::default()).is_ok());
    assert!(validate_config(&PhotosConfig { auto_hydrated_thumbnails_count: -1, ..Default::default() }).is_err());
    assert!(validate_config(&PhotosConfig { full_concurrency: 0, ..Default::default() }).is_err());
}
