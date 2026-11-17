use super::*;

#[test]
fn an_activation_ends_once_its_connection_is_up_or_going_down() {
    assert_eq!(activation(0), Activation::Pending);
    assert_eq!(activation(1), Activation::Pending);
    assert_eq!(activation(2), Activation::Activated);
    assert_eq!(activation(3), Activation::Ended);
    assert_eq!(activation(4), Activation::Ended);
}

#[test]
fn the_hotspot_profile_is_volatile() {
    assert_eq!(join_options().get("persist"), Some(&Value::from("volatile")));
}

#[test]
fn the_scan_names_the_hotspot() {
    let options = scan_options("Pixel Hotspot");
    assert_eq!(options.get("ssids"), Some(&Value::from(vec![b"Pixel Hotspot".to_vec()])));
}

#[test]
fn the_hotspot_profile_leaves_the_key_management_to_the_access_point() {
    let settings = join_settings("Pixel Hotspot", "secret pass", "minji");
    let security = &settings["802-11-wireless-security"];
    assert_eq!(security.get("psk"), Some(&Value::from("secret pass")));
    assert_eq!(security.get("key-mgmt"), None);

    let wireless = &settings["802-11-wireless"];
    assert_eq!(wireless.get("hidden"), Some(&Value::from(true)));
    assert_eq!(wireless.get("ssid"), Some(&Value::from(b"Pixel Hotspot".to_vec())));

    let connection = &settings["connection"];
    assert_eq!(connection.get("permissions"), Some(&Value::from(vec!["user:minji".to_owned()])));
    assert_eq!(connection.get("autoconnect"), Some(&Value::from(false)));
}
