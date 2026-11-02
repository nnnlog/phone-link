//! Golden checks of values a refactor must leave unchanged: the configuration defaults, every store's
//! migrations, and the texts of the errors the engine hands the app. Each check compares against a
//! file under `golden/`; run with `UPDATE_GOLDEN=1` to rewrite the files, then review their diff.

use std::path::PathBuf;

/// Compares `actual` with `golden/<name>`, or writes it there when `UPDATE_GOLDEN` is set.
fn check_golden(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("golden").join(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).expect("write the golden file");
        return;
    }

    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_GOLDEN=1 to create it)", path.display()));
    assert!(
        expected == actual,
        "{} differs from the current value; run with UPDATE_GOLDEN=1 and review the diff",
        path.display()
    );
}

#[test]
fn configuration_defaults_are_unchanged() {
    let config = crate::config::EngineConfig::default();
    let production = crate::config::EngineConfig::production();
    let dump = format!("default:\n{config:#?}\n\nproduction:\n{production:#?}\n");
    check_golden("config.txt", &with_home_as_placeholder(&dump));
}

/// Writes the home folder in `dump` as `$HOME`, because the defaults derive the received-files folder
/// from it and the golden file must match on any machine.
fn with_home_as_placeholder(dump: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => dump.replace(&format!("\"{home}/"), "\"$HOME/"),
        _ => dump.to_owned(),
    }
}

#[test]
fn store_migrations_are_unchanged() {
    use crate::features as f;
    let stores: &[(&str, &str, &[&str])] = &[
        ("global", "phone_switch", f::shared::phone_switch::MIGRATIONS),
        ("global", f::clipboard::store::FEATURE, f::clipboard::store::GLOBAL_MIGRATIONS),
        ("phone", f::clipboard::store::FEATURE, f::clipboard::store::PHONE_MIGRATIONS),
        ("phone", f::audio::FEATURE, f::audio::store::PHONE_MIGRATIONS),
        ("phone", f::calling::store::FEATURE, f::calling::store::MIGRATIONS),
        ("phone", f::contacts::store::FEATURE, f::contacts::store::MIGRATIONS),
        ("phone", f::connected_camera::store::FEATURE, f::connected_camera::store::MIGRATIONS),
        ("phone", f::device_status::FEATURE, f::device_status::battery::MIGRATIONS),
        ("phone", f::file_transfer::store::FEATURE, f::file_transfer::store::MIGRATIONS),
        ("phone", f::messaging::consts::FEATURE, f::messaging::store::MIGRATIONS),
        ("phone", f::notifications::FEATURE, f::notifications::store::MIGRATIONS),
        ("phone", f::phone_apps::store::FEATURE, f::phone_apps::store::MIGRATIONS),
        ("phone", f::phone_files::store::FEATURE, f::phone_files::store::MIGRATIONS),
        ("phone", f::photos::store::FEATURE, f::photos::store::MIGRATIONS),
        ("phone", f::remote_control::store::FEATURE, f::remote_control::store::PHONE_MIGRATIONS),
        ("phone", f::remote_snipping::store::FEATURE, f::remote_snipping::store::MIGRATIONS),
        ("phone", f::screen_mirroring::store::FEATURE, f::screen_mirroring::store::MIGRATIONS),
        ("phone", f::wallpaper::FEATURE, f::wallpaper::store::MIGRATIONS),
    ];

    let mut out = String::new();
    for (scope, feature, migrations) in stores {
        out.push_str(&format!("== {scope} {feature}: {} migrations\n", migrations.len()));
        for (i, m) in migrations.iter().enumerate() {
            out.push_str(&format!("-- {i}\n{m}\n"));
        }
    }

    check_golden("migrations.sql", &out);
}

#[test]
fn error_texts_across_the_ffi_are_unchanged() {
    use crate::api::FeatureApiError;
    use crate::api::bluetooth::BluetoothError;
    use crate::api::hotspot::{HotspotError, HotspotUnavailable};
    use crate::api::phone_files::{PhoneFileError, PhoneFileFailure};
    use crate::edge::{EdgeError, WebSignInError};
    use crate::engine::EngineError;
    use crate::features::bluetooth::edge::{PairingError, RfcommError};
    use crate::features::hotspot::edge::WifiError;

    let x = || "<x>".to_string();
    let feature = || FeatureApiError::NotAvailable { reason: x() };
    let errors: Vec<(&str, Box<dyn std::fmt::Display>)> = vec![
        ("EdgeError::Store", Box::new(EdgeError::Store { message: x() })),
        ("EdgeError::Unexpected", Box::new(EdgeError::Unexpected { message: x() })),
        ("WebSignInError::Declined", Box::new(WebSignInError::Declined)),
        ("WebSignInError::Failed", Box::new(WebSignInError::Failed { reason: x() })),
        ("WifiError::Unavailable", Box::new(WifiError::Unavailable)),
        ("WifiError::Failed", Box::new(WifiError::Failed { reason: x() })),
        ("WifiError::Unexpected", Box::new(WifiError::Unexpected { message: x() })),
        ("PairingError::NotFound", Box::new(PairingError::NotFound)),
        ("PairingError::Failed", Box::new(PairingError::Failed { reason: x() })),
        ("PairingError::Unexpected", Box::new(PairingError::Unexpected { message: x() })),
        ("RfcommError::Unreachable", Box::new(RfcommError::Unreachable)),
        ("RfcommError::ServiceNotFound", Box::new(RfcommError::ServiceNotFound)),
        ("RfcommError::ConnectFailed", Box::new(RfcommError::ConnectFailed { reason: x() })),
        ("RfcommError::Io", Box::new(RfcommError::Io { reason: x() })),
        ("RfcommError::TimedOut", Box::new(RfcommError::TimedOut)),
        ("RfcommError::Unexpected", Box::new(RfcommError::Unexpected { message: x() })),
        ("FeatureApiError::NotStarted", Box::new(FeatureApiError::NotStarted)),
        ("FeatureApiError::NoPhone", Box::new(FeatureApiError::NoPhone)),
        ("FeatureApiError::UnknownItem", Box::new(FeatureApiError::UnknownItem { what: x() })),
        ("FeatureApiError::NotAvailable", Box::new(feature())),
        ("FeatureApiError::InvalidInput", Box::new(FeatureApiError::InvalidInput { reason: x() })),
        ("FeatureApiError::Store", Box::new(FeatureApiError::Store { message: x() })),
        ("FeatureApiError::Internal", Box::new(FeatureApiError::Internal { message: x() })),
        ("HotspotError::Feature", Box::new(HotspotError::Feature { error: feature() })),
        ("HotspotError::Busy", Box::new(HotspotError::Busy)),
        ("HotspotError::NotSupported", Box::new(HotspotError::NotSupported)),
        ("HotspotError::Bluetooth", Box::new(HotspotError::Bluetooth { error: BluetoothError::BluetoothOff })),
        ("HotspotError::ConsentTimedOut", Box::new(HotspotError::ConsentTimedOut)),
        ("HotspotError::NotTrusted", Box::new(HotspotError::NotTrusted { reason: x() })),
        ("HotspotError::Unreachable", Box::new(HotspotError::Unreachable { reason: x() })),
        ("HotspotError::SideChannelNotOpen", Box::new(HotspotError::SideChannelNotOpen)),
        ("HotspotError::NoAnswer", Box::new(HotspotError::NoAnswer)),
        ("HotspotError::PhoneError", Box::new(HotspotError::PhoneError { reason: x() })),
        ("HotspotError::Unavailable", Box::new(HotspotError::Unavailable { reason: HotspotUnavailable::AirplaneMode })),
        ("HotspotError::NoCredentials", Box::new(HotspotError::NoCredentials)),
        ("HotspotError::InvalidCredentials", Box::new(HotspotError::InvalidCredentials { reason: x() })),
        ("HotspotError::NotVisible", Box::new(HotspotError::NotVisible { ssid: x(), passphrase: x() })),
        ("HotspotError::JoinFailed", Box::new(HotspotError::JoinFailed { ssid: x(), passphrase: x(), reason: x() })),
        ("HotspotError::LeaveFailed", Box::new(HotspotError::LeaveFailed { reason: x() })),
        ("HotspotError::Internal", Box::new(HotspotError::Internal { message: x() })),
        (
            "PhoneFileError::Failure",
            Box::new(PhoneFileError::Failure { failure: PhoneFileFailure::NotRunning, message: x() }),
        ),
        ("BluetoothError::BluetoothOff", Box::new(BluetoothError::BluetoothOff)),
        ("BluetoothError::PermissionNotGranted", Box::new(BluetoothError::PermissionNotGranted)),
        ("BluetoothError::NotDiscoverable", Box::new(BluetoothError::NotDiscoverable)),
        ("BluetoothError::PhoneNotFound", Box::new(BluetoothError::PhoneNotFound)),
        ("BluetoothError::PairingFailed", Box::new(BluetoothError::PairingFailed { reason: x() })),
        ("BluetoothError::Internal", Box::new(BluetoothError::Internal { message: x() })),
        ("EngineError::Auth", Box::new(EngineError::Auth { message: x() })),
        ("EngineError::NoPhone", Box::new(EngineError::NoPhone)),
        ("EngineError::Setup", Box::new(EngineError::Setup { message: x() })),
        ("EngineError::NotStarted", Box::new(EngineError::NotStarted)),
    ];

    let out: String = errors.iter().map(|(name, e)| format!("{name}: {e}\n")).collect();
    check_golden("error-texts.txt", &out);
}
