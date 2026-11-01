use super::*;
use phonelink_core::types::{DcgClient, Environment};
use phonelink_features::StoreLocation;

fn db() -> FeatureDb {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    FeatureDb::open_phone(&StoreLocation::Memory, &phone, "wallpaper", MIGRATIONS).unwrap()
}

#[test]
fn a_new_store_has_nothing_and_minus_one_sequences() {
    let db = db();
    assert_eq!(
        phone_link_state(&db).unwrap(),
        PhoneLinkState { current_sequence: -1, expected_sequence: -1, device_permission: false }
    );
    assert_eq!(wallpaper(&db, WallpaperCopy::PhoneLink, HOME_SCREEN).unwrap(), None);
    assert_eq!(cross_device_ids(&db).unwrap(), (None, None));
}

#[test]
fn rows_are_replaced_per_type_and_copies_are_separate() {
    let db = db();
    put_phone_link_item(&db, HOME_SCREEN, 3, b"a", 10).unwrap();
    put_phone_link_item(&db, HOME_SCREEN, 4, b"b", 11).unwrap();
    put_phone_link_item(&db, 7, 5, b"c", 12).unwrap();

    assert_eq!(
        wallpaper(&db, WallpaperCopy::PhoneLink, HOME_SCREEN).unwrap(),
        Some(StoredWallpaper { id: 4, bytes: b"b".to_vec() })
    );
    assert_eq!(phone_link_state(&db).unwrap().current_sequence, 12);
    assert_eq!(wallpaper(&db, WallpaperCopy::CrossDevice, HOME_SCREEN).unwrap(), None);

    apply_cross_device(&db, &CrossDeviceChange::Upsert(vec![(LOCK_SCREEN, 9, b"l".to_vec())])).unwrap();
    assert_eq!(cross_device_ids(&db).unwrap(), (None, Some(9)));
    apply_cross_device(&db, &CrossDeviceChange::DeleteAll).unwrap();
    assert_eq!(cross_device_ids(&db).unwrap(), (None, None));
    assert!(wallpaper(&db, WallpaperCopy::PhoneLink, HOME_SCREEN).unwrap().is_some(), "Phone Link's copy is kept");
}

#[test]
fn the_permission_reports_a_change() {
    let db = db();
    assert!(!set_device_permission(&db, false).unwrap());
    assert!(set_device_permission(&db, true).unwrap());
    assert!(!set_device_permission(&db, true).unwrap());
    assert!(phone_link_state(&db).unwrap().device_permission);

    set_expected_sequence(&db, 4).unwrap();
    assert_eq!(phone_link_state(&db).unwrap().expected_sequence, 4);
}
