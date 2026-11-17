use super::*;

#[test]
fn the_internet_path_is_the_primary_connection_type() {
    let primary = "/org/freedesktop/NetworkManager/ActiveConnection/3";
    assert_eq!(internet_path_of(70, primary, "802-11-wireless"), PcInternetPath::WiFi);
    assert_eq!(internet_path_of(70, primary, "802-3-ethernet"), PcInternetPath::WiredEthernet);
    assert_eq!(internet_path_of(70, primary, "gsm"), PcInternetPath::Cellular);
    assert_eq!(internet_path_of(70, primary, "cdma"), PcInternetPath::Cellular);
    assert_eq!(internet_path_of(60, primary, "bluetooth"), PcInternetPath::Other);
    assert_eq!(internet_path_of(70, primary, "wireguard"), PcInternetPath::Other);
    assert_eq!(internet_path_of(50, primary, "802-3-ethernet"), PcInternetPath::WiredEthernet);
}

#[test]
fn there_is_no_internet_path_without_a_primary_connection_or_while_connecting() {
    assert_eq!(internet_path_of(70, "/", ""), PcInternetPath::Unavailable);
    let primary = "/org/freedesktop/NetworkManager/ActiveConnection/3";
    assert_eq!(internet_path_of(40, primary, "802-11-wireless"), PcInternetPath::Unavailable);
    assert_eq!(internet_path_of(20, primary, "802-3-ethernet"), PcInternetPath::Unavailable);
}

#[test]
fn the_interface_of_the_primary_connection_is_reported() {
    let path = |p: &str| OwnedObjectPath::try_from(p).unwrap();
    let associated = [(path("/ac/1"), "wlan0".to_owned()), (path("/ac/2"), "wlan1".to_owned())];
    assert_eq!(reported_interface(&path("/ac/2"), &associated), Some("wlan1"));
    assert_eq!(reported_interface(&path("/ac/9"), &associated), Some("wlan0"));
    assert_eq!(reported_interface(&path("/"), &[]), None);
}
