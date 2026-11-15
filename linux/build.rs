use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};
use serde_json::Value;

const QML_FILES: [&str; 32] = [
    "qml/Main.qml",
    "qml/MainView.qml",
    "qml/Sidebar.qml",
    "qml/ControlTile.qml",
    "qml/MediaCard.qml",
    "qml/TransferCard.qml",
    "qml/NotificationList.qml",
    "qml/NotificationDetail.qml",
    "qml/ConversationList.qml",
    "qml/ConversationDetail.qml",
    "qml/CallList.qml",
    "qml/CallDetail.qml",
    "qml/ContactList.qml",
    "qml/ContactDetail.qml",
    "qml/PhotoGrid.qml",
    "qml/AppGrid.qml",
    "qml/SettingsWindow.qml",
    "qml/EngineStateView.qml",
    "qml/SignInView.qml",
    "qml/EmptyState.qml",
    "qml/Icon.qml",
    "qml/IconButton.qml",
    "qml/PillButton.qml",
    "qml/CapsuleField.qml",
    "qml/MacSwitch.qml",
    "qml/Monogram.qml",
    "qml/Avatar.qml",
    "qml/AppGlyph.qml",
    "qml/CallBanner.qml",
    "qml/MirrorWindow.qml",
    "qml/MirrorOverlay.qml",
    "qml/CameraControlWindow.qml",
];

const QML_SINGLETONS: [&str; 3] = ["qml/Theme.qml", "qml/Dates.qml", "qml/Preferences.qml"];

const MAC_STRINGS: &str = "../gui/PhoneLink/Localizable.xcstrings";
const LINUX_STRINGS: &str = "i18n/ko.json";

/// The QML sources write "..." where the macOS strings write U+2026, so keys are matched in that form.
fn normalized_key(key: &str) -> String {
    key.replace('\u{2026}', "...")
}

fn mac_korean(path: &Path) -> BTreeMap<String, String> {
    let catalog: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("the macOS string catalog reads"))
        .expect("the macOS string catalog is JSON");
    let strings = catalog["strings"].as_object().expect("the catalog has strings");
    strings
        .iter()
        .filter_map(|(key, entry)| {
            let korean = &entry["localizations"]["ko"];
            let value = korean["stringUnit"]["value"]
                .as_str()
                .or_else(|| korean["variations"]["plural"]["other"]["stringUnit"]["value"].as_str())?;
            Some((normalized_key(key), value.to_owned()))
        })
        .collect()
}

fn linux_korean(path: &Path) -> BTreeMap<String, String> {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the Linux strings read"))
        .expect("the Linux strings are a JSON object")
}

fn write_translations() {
    println!("cargo::rerun-if-changed={MAC_STRINGS}");
    println!("cargo::rerun-if-changed={LINUX_STRINGS}");
    let mut korean = mac_korean(Path::new(MAC_STRINGS));
    korean.extend(linux_korean(Path::new(LINUX_STRINGS)));

    let mut source = String::from("pub static KOREAN: &[(&str, &str)] = &[\n");
    for (key, value) in &korean {
        writeln!(source, "    ({key:?}, {value:?}),").expect("writing to a String succeeds");
    }
    source.push_str("];\n");
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("translations.rs");
    std::fs::write(out, source).expect("the translation table writes");
}

fn main() {
    write_translations();
    let module = QmlModule::new("PhoneLink")
        .qml_files(QML_FILES)
        .qml_files(QML_SINGLETONS.map(|path| QmlFile::from(path).singleton(true)));
    CxxQtBuilder::new_qml_module(module)
        .qt_module("Quick")
        .qt_module("Widgets")
        .files(["src/application.rs", "src/engine_bridge.rs", "src/l10n.rs", "src/mirror_video.rs"])
        .build();
}
