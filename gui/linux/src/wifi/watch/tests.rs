use super::*;

#[test]
fn the_network_changes_with_the_access_point_or_the_connection() {
    let changed = |name: &str| HashMap::from([(name.to_owned(), OwnedValue::from(ObjectPath::try_from("/").unwrap()))]);
    assert_eq!(device_change(NM_WIRELESS, &changed("ActiveAccessPoint")), DeviceChange::AccessPoint);
    assert_eq!(device_change(NM_DEVICE, &changed("ActiveConnection")), DeviceChange::Connection);
    assert_eq!(device_change(NM_WIRELESS, &changed("LastScan")), DeviceChange::Other);
    assert_eq!(device_change(NM_DEVICE, &changed("ActiveAccessPoint")), DeviceChange::Other);
}
