use super::*;
use phonelink_core::types::Environment;

#[test]
fn default_on_persisted_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    let phone = DcgClient::new("0F8FAD5B-d9cb-469f-a165-70867728950e", Environment::Prod);
    let switches = PhoneSwitches::open(&location).unwrap();
    let mut rx = switches.subscribe();

    assert!(switches.is_enabled(&phone).unwrap());

    switches.set_enabled(&phone, true).unwrap();
    assert!(rx.try_recv().is_err(), "no change, no event");

    switches.set_enabled(&phone, false).unwrap();
    assert_eq!(rx.try_recv().unwrap(), PhoneSwitchChanged { phone: phone.clone(), enabled: false });

    drop(switches);
    let reopened = PhoneSwitches::open(&location).unwrap();
    let lower = DcgClient::new(phone.id.to_ascii_lowercase(), Environment::Prod);
    assert!(!reopened.is_enabled(&lower).unwrap(), "persisted, ids compare case-insensitively");

    PhoneSwitches::forget_all(&location).unwrap();
    assert!(reopened.is_enabled(&phone).unwrap(), "forgotten: on again");
}
