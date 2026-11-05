use super::*;
use crate::edge::testing::MemoryStore;

#[test]
fn logical_device_id_is_minted_once_and_the_session_id_is_the_processs() {
    let store = MemoryStore::new();
    let config = EngineConfig::default();
    let a = ClientIdentification::load_or_create(&config, &store).unwrap();
    let b = ClientIdentification::load_or_create(&config, &MemoryStore::new()).unwrap();
    let again = ClientIdentification::load_or_create(&config, &store).unwrap();

    assert_eq!(a.logical_device_id, again.logical_device_id);
    assert_ne!(a.logical_device_id, b.logical_device_id, "per store");
    assert_eq!(a.logical_device_id.len(), 44);

    // One DCG-SessionId per process, whatever loads it: a rebuilt engine core keeps it.
    assert_eq!(a.session_id, b.session_id);
    assert_eq!(a.session_id, process_session_id("WEA"));
    assert_ne!(process_session_id("PL"), process_session_id("WEA"), "one per PC identity");
    assert_eq!(a.session_id.len(), 36);
    assert_eq!(a.session_id, a.session_id.to_lowercase());

    assert_eq!(a.app_id, "MicrosoftWindows.CrossDevice_cw5n1h2txyewy");
    assert_eq!(a.app_version, "1.26092.74.0");
    assert_eq!(a.headers()[0].0, "DCG-LogicalDeviceId");
}
