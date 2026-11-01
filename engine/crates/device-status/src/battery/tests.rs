use std::time::UNIX_EPOCH;

use super::*;
use phonelink_core::types::Environment;
use phonelink_features::shared::db::StoreLocation;

#[test]
fn rule() {
    assert!(is_low(Some(9), false, 9));
    assert!(is_low(Some(0), false, 9));
    assert!(!is_low(Some(10), false, 9));
    assert!(!is_low(Some(-1), false, 9), "-1 is unknown");
    assert!(!is_low(None, false, 9));
    assert!(!is_low(Some(5), true, 9), "charging");

    let t0 = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let hour = Duration::from_secs(3600);
    assert!(interval_passed(None, t0, hour));
    assert!(!interval_passed(Some(t0), t0 + hour - Duration::from_secs(1), hour));
    assert!(interval_passed(Some(t0), t0 + hour, hour));
    assert!(!interval_passed(Some(t0 + hour), t0, hour), "a last time in the future blocks");
}

#[test]
fn settings_round_trip() {
    let phone = DcgClient::new("0f8fad5b-d9cb-469f-a165-70867728950e", Environment::Prod);
    let db = FeatureDb::open_phone(&StoreLocation::Memory, &phone, "device_status", MIGRATIONS).unwrap();

    assert_eq!(load(&db).unwrap(), LowBatterySettings::default());
    let at = UNIX_EPOCH + Duration::from_millis(1_792_454_400_123);
    save(&db, LowBatterySettings { enabled: false, last_shown: Some(at) }).unwrap();
    assert_eq!(load(&db).unwrap(), LowBatterySettings { enabled: false, last_shown: Some(at) });

    assert_eq!(notification_id(&phone), "device_status/low-battery/0f8fad5b-d9cb-469f-a165-70867728950e.Prod");
    let n = notification(&phone, "Pixel", 9);
    assert_eq!(notification_phone(&n.user_info), Some(phone));
    assert_eq!(notification_phone(&Default::default()), None);
}
