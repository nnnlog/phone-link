use super::*;
use phonelink_core::types::Environment;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE item (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
    "ALTER TABLE item ADD COLUMN size INTEGER NOT NULL DEFAULT 0;",
];

fn phone() -> DcgClient {
    DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod)
}

#[test]
fn migrations_run_once_and_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());

    {
        let db = FeatureDb::open_phone(&location, &phone(), "sample", MIGRATIONS).unwrap();
        db.transaction(|tx| tx.execute("INSERT INTO item (id, name, size) VALUES (1, 'a', 3)", []).map(|_| ()))
            .unwrap();
    }

    let db = FeatureDb::open_phone(&location, &phone(), "sample", MIGRATIONS).unwrap();
    let size: i64 = db.read(|c| c.query_row("SELECT size FROM item WHERE id = 1", [], |r| r.get(0))).unwrap();
    assert_eq!(size, 3);

    let file = dir.path().join("phones").join(format!("{}.Prod", phone().id)).join("sample.sqlite3");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    // A build that knows fewer migrations refuses the newer database.
    let err = FeatureDb::open_phone(&location, &phone(), "sample", &MIGRATIONS[..1]).unwrap_err();
    assert!(matches!(err, StoreError::NewerSchema { feature: "sample", found: 2, known: 1 }), "{err:?}");
}

#[test]
fn a_failed_transaction_changes_nothing() {
    let db = FeatureDb::open_phone(&StoreLocation::Memory, &phone(), "sample", MIGRATIONS).unwrap();

    let r: Result<(), StoreError> = db.transaction(|tx| {
        tx.execute("INSERT INTO item (id, name) VALUES (1, 'a')", [])?;
        Err(StoreError::Corrupt("abort".into()))
    });

    assert!(r.is_err());
    let n: i64 = db.read(|c| c.query_row("SELECT COUNT(*) FROM item", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 0);
}

#[test]
fn a_store_stays_usable_after_a_panic_rolled_its_transaction_back() {
    let db = FeatureDb::open_phone(&StoreLocation::Memory, &phone(), "sample", MIGRATIONS).unwrap();

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), StoreError> = db.transaction(|tx| {
            tx.execute("INSERT INTO item (id, name) VALUES (1, 'a')", [])?;
            panic!("a bug while applying phone content");
        });
    }));

    assert!(panicked.is_err());
    let n: i64 = db.read(|c| c.query_row("SELECT COUNT(*) FROM item", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 0, "rolled back, and the store is still usable");
    db.transaction(|tx| tx.execute("INSERT INTO item (id, name) VALUES (2, 'b')", []).map(|_| ())).unwrap();
}

#[test]
fn an_existing_directory_keeps_its_mode_and_created_ones_are_private() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let location = StoreLocation::Directory(root.clone());

    let _global = FeatureDb::open_global(&location, "sample", MIGRATIONS).unwrap();
    let _phone = FeatureDb::open_phone(&location, &phone(), "sample", MIGRATIONS).unwrap();

    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o755, "the caller's directory is left as it is");
    assert_eq!(mode(&root.join("phones")), 0o700);
    assert_eq!(mode(&root.join("phones").join(format!("{}.Prod", phone().id))), 0o700);

    let fresh = dir.path().join("new").join("data");
    let _ = FeatureDb::open_global(&StoreLocation::Directory(fresh.clone()), "sample", MIGRATIONS).unwrap();
    assert_eq!(mode(&fresh), 0o700, "a directory the engine creates is private");
}

#[test]
fn names_that_could_escape_are_refused() {
    let bad = DcgClient::new("../x", Environment::Prod);

    assert!(matches!(
        FeatureDb::open_phone(&StoreLocation::Memory, &bad, "sample", MIGRATIONS),
        Err(StoreError::InvalidName(_))
    ));
    assert!(matches!(
        FeatureDb::open_global(&StoreLocation::Memory, "../x", MIGRATIONS),
        Err(StoreError::InvalidName(_))
    ));
}

#[test]
fn phone_stores_keep_one_memory_database_per_phone() {
    let stores = PhoneStores::new(StoreLocation::Memory, "sample", MIGRATIONS);
    let a = stores.get(&phone()).unwrap();
    a.transaction(|tx| tx.execute("INSERT INTO item (id, name) VALUES (7, 'x')", []).map(|_| ())).unwrap();

    let b = stores.get(&phone()).unwrap();
    let n: i64 = b.read(|c| c.query_row("SELECT COUNT(*) FROM item", [], |r| r.get(0))).unwrap();
    assert_eq!(n, 1, "the same phone gets the same database");
}
