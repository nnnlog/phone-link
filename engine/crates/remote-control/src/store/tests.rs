use super::*;
use phonelink_core::types::Environment;

fn phone() -> DcgClient {
    DcgClient::new("0F8FAD5B-d9cb-469f-a165-70867728950e", Environment::Prod)
}

#[test]
fn default_on_persisted_per_phone() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());

    {
        let store = RemoteControlStore::open(&location);
        assert!(store.toggle(&phone()).unwrap(), "on until the user turns it off");
        assert!(!store.set_toggle(&phone(), true).unwrap(), "the default: no change");
        assert!(store.set_toggle(&phone(), false).unwrap());
        assert!(!store.set_toggle(&phone(), false).unwrap());
    }

    let store = RemoteControlStore::open(&location);
    let lower = DcgClient::new(phone().id.to_ascii_lowercase(), Environment::Prod);
    assert!(!store.toggle(&lower).unwrap(), "persisted; ids compare case-insensitively");
    assert!(store.toggle(&DcgClient::new(phone().id, Environment::Beta)).unwrap(), "another phone");
}
