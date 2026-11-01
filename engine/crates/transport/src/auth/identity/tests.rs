use super::*;

fn metadata(caps: &[&str]) -> DeviceMetadata {
    DeviceMetadata {
        capabilities: caps.iter().map(|c| c.to_string()).collect(),
        client_type: "WEA".into(),
        client_version: "1.26082.74.0".into(),
        consent_version: 0,
        custom_data: String::new(),
        display_name: "mac".into(),
        is_enabled: true,
        manufacture: "Unknown".into(),
        model_name: "Unknown".into(),
        model_version: "Unknown".into(),
        os_name: "Windows".into(),
        os_version: "10.0.26100".into(),
    }
}

#[test]
fn metadata_hash_is_stable_and_sensitive() {
    let a = metadata_hash(&metadata(&["DEVICE_STATUS", "UNLINKABLE_V1"]));

    assert_eq!(a, metadata_hash(&metadata(&["DEVICE_STATUS", "UNLINKABLE_V1"])));
    assert_eq!(a.len(), 64);
    assert_ne!(a, metadata_hash(&metadata(&["DEVICE_STATUS"])));
    assert_ne!(metadata_hash(&metadata(&["AB", "C"])), metadata_hash(&metadata(&["A", "BC"])));

    let mut other = metadata(&["DEVICE_STATUS", "UNLINKABLE_V1"]);
    other.manufacture = "Apple".into();
    assert_eq!(a, metadata_hash(&other), "only the six named fields are hashed");
}
