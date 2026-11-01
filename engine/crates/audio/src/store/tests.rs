use super::*;
use phonelink_core::types::Environment;

#[test]
fn default_on_persisted_per_phone() {
    let dir = tempfile::tempdir().unwrap();
    let location = StoreLocation::Directory(dir.path().to_path_buf());
    let phone = DcgClient::new("0F8FAD5B-d9cb-469f-a165-70867728950e", Environment::Prod);
    let other = DcgClient::new("7c9e6679-7425-40de-944b-e07fc1f90ae7", Environment::Prod);

    {
        let store = AudioStore::open(&location);
        assert!(store.setting(&phone).unwrap());
        assert!(!store.set_setting(&phone, true).unwrap(), "the default: no change");
        assert!(store.set_setting(&phone, false).unwrap());
    }

    let store = AudioStore::open(&location);
    assert!(!store.setting(&DcgClient::new(phone.id.to_ascii_uppercase(), phone.env)).unwrap());
    assert!(store.setting(&other).unwrap());
}
