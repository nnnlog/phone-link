use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use cxx_qt_build::{CxxQtBuilder, QmlFile, QmlModule};
use serde_json::Value;

const QML_DIR: &str = "qml";
const QML_SINGLETONS: [&str; 10] = [
    "qml/Theme.qml",
    "qml/Dates.qml",
    "qml/Preferences.qml",
    "qml/ShellPreferences.qml",
    "qml/IconsMessages.qml",
    "qml/IconsNotifications.qml",
    "qml/IconsCalls.qml",
    "qml/IconsMedia.qml",
    "qml/IconsMirror.qml",
    "qml/IconsShell.qml",
];

/// Qt libraries only the C++ helpers use. The linker sees the cxx-qt library holding those helpers after
/// Qt's own `-l` flags, so with `--as-needed` GNU ld drops these libraries before anything refers to them;
/// naming them again at the end of the link line keeps them.
const LATE_QT_LIBRARIES: [&str; 3] = ["Qt6Widgets", "Qt6WebEngineQuick", "Qt6WebEngineCore"];

const MAC_STRINGS: &str = "../mac/PhoneLink/Localizable.xcstrings";
/// The Linux app's own Korean: `ko.json` and one `ko-<area>.json` per part of the app. They add to the
/// macOS catalog's Korean; a key that two of these files (the catalog included) give different values
/// fails the build, so the order they are read in decides nothing.
const LINUX_STRINGS_DIR: &str = "i18n";

/// The QML sources write "..." where the macOS strings write U+2026, so keys are matched in that form.
fn normalized_key(key: &str) -> String {
    key.replace('\u{2026}', "...")
}

fn mac_catalog(path: &Path) -> Value {
    let catalog: Value = serde_json::from_str(&std::fs::read_to_string(path).expect("the macOS string catalog reads"))
        .expect("the macOS string catalog is JSON");
    assert_eq!(catalog["sourceLanguage"], "en", "the macOS string catalog's source language is English");
    catalog
}

/// The catalog's Korean. Korean has no plural forms, so a key with plural variants has only `other`.
fn mac_korean(strings: &serde_json::Map<String, Value>) -> BTreeMap<String, String> {
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

/// The English singular (`one`) form of each catalog key with plural variants: the key's own variants,
/// or its text with each plural substitution (`%#@name@`) written in its `one` form, where `%arg` stands
/// for the substitution's argument (`%2$lld` for argument 2 with the specifier `lld`).
fn mac_english_singulars(strings: &serde_json::Map<String, Value>) -> BTreeMap<String, String> {
    let one = |variations: &Value| variations["plural"]["one"]["stringUnit"]["value"].as_str().map(str::to_owned);
    strings
        .iter()
        .filter_map(|(key, entry)| {
            let english = &entry["localizations"]["en"];
            let singular = match english["substitutions"].as_object() {
                None => one(&english["variations"])?,
                Some(substitutions) => {
                    let mut text = english["stringUnit"]["value"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{key:?} has substitutions but no English text"))
                        .to_owned();
                    for (name, substitution) in substitutions {
                        let variant = one(&substitution["variations"])
                            .unwrap_or_else(|| panic!("{key:?}'s substitution {name} has no `one` form"));
                        let argument = format!(
                            "%{}${}",
                            substitution["argNum"].as_u64().expect("a substitution numbers its argument"),
                            substitution["formatSpecifier"].as_str().expect("a substitution names its specifier"),
                        );
                        let placeholder = format!("%#@{name}@");
                        assert!(text.contains(&placeholder), "{key:?} does not use its substitution {name}");
                        text = text.replace(&placeholder, &variant.replace("%arg", &argument));
                    }
                    text
                }
            };
            Some((normalized_key(key), normalized_key(&singular)))
        })
        .collect()
}

fn linux_korean(path: &Path) -> BTreeMap<String, String> {
    serde_json::from_str(&std::fs::read_to_string(path).expect("the Linux strings read"))
        .unwrap_or_else(|e| panic!("{} is a JSON object of strings: {e}", path.display()))
}

/// `i18n/ko.json` and every `i18n/ko-*.json`, in name order (`ko.json` last, as `-` sorts before `.`).
fn linux_string_files() -> Vec<std::path::PathBuf> {
    println!("cargo::rerun-if-changed={LINUX_STRINGS_DIR}");
    let mut files: Vec<_> = std::fs::read_dir(LINUX_STRINGS_DIR)
        .expect("the i18n directory reads")
        .map(|entry| entry.expect("an i18n entry reads").path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            name == "ko.json" || (name.starts_with("ko-") && name.ends_with(".json"))
        })
        .collect();
    files.sort();
    files
}

/// `table` as the Rust source of a static slice named `name`, sorted by key for binary search.
fn write_table(source: &mut String, name: &str, table: &BTreeMap<String, String>) {
    writeln!(source, "pub static {name}: &[(&str, &str)] = &[").expect("writing to a String succeeds");
    for (key, value) in table {
        writeln!(source, "    ({key:?}, {value:?}),").expect("writing to a String succeeds");
    }
    source.push_str("];\n");
}

fn write_translations() {
    println!("cargo::rerun-if-changed={MAC_STRINGS}");
    let catalog = mac_catalog(Path::new(MAC_STRINGS));
    let strings = catalog["strings"].as_object().expect("the catalog has strings");
    let mut korean = mac_korean(strings);
    let mut sources: BTreeMap<String, String> =
        korean.keys().map(|key| (key.clone(), MAC_STRINGS.to_owned())).collect();
    for file in linux_string_files() {
        println!("cargo::rerun-if-changed={}", file.display());
        for (key, value) in linux_korean(&file) {
            if let Some(earlier) = korean.get(&key).filter(|earlier| **earlier != value) {
                panic!("{key:?} is {earlier:?} in {} but {value:?} in {}", sources[&key], file.display());
            }
            sources.insert(key.clone(), file.display().to_string());
            korean.insert(key, value);
        }
    }

    let mut source = String::new();
    write_table(&mut source, "KOREAN", &korean);
    write_table(&mut source, "ENGLISH_ONE", &mac_english_singulars(strings));
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("translations.rs");
    std::fs::write(out, source).expect("the translation table writes");
}

/// Every QML file of the module that is not a singleton, in a stable order.
fn qml_files() -> Vec<String> {
    println!("cargo::rerun-if-changed={QML_DIR}");
    let mut files: Vec<String> = std::fs::read_dir(QML_DIR)
        .expect("the qml directory reads")
        .map(|entry| entry.expect("a qml directory entry reads").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "qml"))
        .map(|path| path.to_str().expect("QML file names are UTF-8").to_owned())
        .filter(|path| !QML_SINGLETONS.contains(&path.as_str()))
        .collect();
    files.sort();
    files
}

fn main() {
    write_translations();
    let module = QmlModule::new("PhoneLink")
        .qml_files(qml_files())
        .qml_files(QML_SINGLETONS.map(|path| QmlFile::from(path).singleton(true)));
    CxxQtBuilder::new_qml_module(module)
        .qt_module("Quick")
        .qt_module("Widgets")
        .qt_module("WebEngineCore")
        .qt_module("WebEngineQuick")
        // Qt reads each bridge by its file name alone, so every bridge of the module sits in one folder.
        .files([
            "src/bridges/application.rs",
            "src/bridges/engine_bridge.rs",
            "src/bridges/l10n.rs",
            "src/bridges/mirror_video.rs",
        ])
        .build();
    for library in LATE_QT_LIBRARIES {
        println!("cargo::rustc-link-arg=-l{library}");
    }
}
