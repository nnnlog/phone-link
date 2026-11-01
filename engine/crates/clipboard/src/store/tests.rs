use super::*;
use phonelink_core::types::Environment;

fn phone() -> DcgClient {
    DcgClient::new("0F8FAD5B-d9cb-469f-a165-70867728950e", Environment::Prod)
}

#[test]
fn defaults() {
    let store = ClipboardStore::open(&StoreLocation::Memory).unwrap();
    assert!(store.global_switch().unwrap());
    assert!(!store.toggle(&phone()).unwrap());
    assert!(store.up_to_date(&phone()).unwrap());
    assert!(global_switch_persisted(&StoreLocation::Memory).unwrap());
}

#[test]
fn writes_report_changes_and_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());

    {
        let store = ClipboardStore::open(&location).unwrap();
        assert!(!store.set_global_switch(true).unwrap(), "the default: no change");
        assert!(store.set_global_switch(false).unwrap());
        assert!(store.set_toggle(&phone(), true).unwrap());
        assert!(!store.set_toggle(&phone(), true).unwrap());
        assert!(store.set_up_to_date(&phone(), false).unwrap());
    }

    assert!(!global_switch_persisted(&location).unwrap(), "readable before the feature is installed");
    let store = ClipboardStore::open(&location).unwrap();
    assert!(!store.global_switch().unwrap());

    let lower = DcgClient::new(phone().id.to_ascii_lowercase(), Environment::Prod);
    assert!(store.toggle(&lower).unwrap(), "persisted; ids compare case-insensitively");
    assert!(!store.up_to_date(&lower).unwrap());

    let other_env = DcgClient::new(phone().id, Environment::Beta);
    assert!(!store.toggle(&other_env).unwrap(), "another environment is another phone");
}

#[test]
fn capability_list() {
    let base = vec!["DEVICE_STATUS".to_owned(), "UNLINKABLE_V1".to_owned()];
    assert_eq!(with_clipboard_capability(&base, true), vec!["CLIPBOARD", "DEVICE_STATUS", "UNLINKABLE_V1"]);
    assert_eq!(with_clipboard_capability(&base, false), base);

    let with = with_clipboard_capability(&base, true);
    assert_eq!(with_clipboard_capability(&with, false), base);
    assert_eq!(with_clipboard_capability(&with, true), with, "never twice");
}
