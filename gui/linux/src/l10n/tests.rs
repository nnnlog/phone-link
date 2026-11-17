use super::*;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|a| (*a).to_owned()).collect()
}

#[test]
fn fills_sequential_and_positional_specifiers() {
    assert_eq!(fill("%@ Camera", &args(&["Fold"])), "Fold Camera");
    assert_eq!(fill("%2$@GB of %1$@GB", &args(&["10", "64"])), "64GB of 10GB");
    assert_eq!(fill("%lld new", &args(&["3"])), "3 new");
    assert_eq!(fill("%u files sent to your phone.", &args(&["3"])), "3 files sent to your phone.");
    assert_eq!(fill("100% sure", &[]), "100% sure");
}

#[test]
fn a_doubled_percent_sign_is_written_once() {
    assert_eq!(fill("Battery %d%%", &args(&["76"])), "Battery 76%");
    assert_eq!(fill("%d%% · Charging", &args(&["76"])), "76% · Charging");
    assert_eq!(fill("%%@ stays", &args(&["unused"])), "%@ stays");
}

#[test]
fn plural_keys_take_their_english_singular_for_one() {
    let sent = "%u files sent to your phone.";
    assert_eq!(fill(translate_plural(sent, true, false), &args(&["1"])), "1 file sent to your phone.");
    assert_eq!(fill(translate_plural(sent, false, false), &args(&["2"])), "2 files sent to your phone.");
    assert_eq!(fill(translate_plural(sent, false, false), &args(&["0"])), "0 files sent to your phone.");
    // A singular may leave the count out.
    let save = "Choose a folder to save %lld photos in.";
    assert_eq!(fill(translate_plural(save, true, false), &args(&["1"])), "Choose a folder to save the photo in.");
}

#[test]
fn a_plural_substitution_takes_its_singular_in_place() {
    let tile = "%@, %lld notifications";
    assert_eq!(fill(translate_plural(tile, true, false), &args(&["Mail", "1"])), "Mail, 1 notification");
    assert_eq!(fill(translate_plural(tile, false, false), &args(&["Mail", "3"])), "Mail, 3 notifications");
}

#[test]
fn korean_has_one_form_for_every_count() {
    let sent = "%u files sent to your phone.";
    assert_eq!(translate_plural(sent, true, true), translate_plural(sent, false, true));
    assert_eq!(fill(translate_plural(sent, true, true), &args(&["1"])), "파일 1개를 휴대폰으로 보냈어요.");
}

#[test]
fn a_key_without_plural_variants_is_left_as_it_is() {
    assert_eq!(translate_plural("Settings", true, false), "Settings");
}

#[test]
fn tables_are_sorted_for_lookup() {
    assert!(table::KOREAN.windows(2).all(|pair| pair[0].0 < pair[1].0));
    assert!(table::ENGLISH_ONE.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

/// The double-quoted QML string literal `source` starts with, unescaped, and the text after it.
fn qml_literal(source: &str) -> Option<(String, &str)> {
    let body = source.strip_prefix('"')?;
    let mut text = String::new();
    let mut chars = body.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Some((text, &body[index + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => text.push('\n'),
                't' => text.push('\t'),
                escaped @ ('"' | '\'' | '\\') => text.push(escaped),
                other => panic!("a QML text uses the escape \\{other}, which this check does not read"),
            },
            _ => text.push(c),
        }
    }
    None
}

/// Every `L10n.t(...)`, `L10n.f(...)` and `L10n.p(...)` in the QML passes its English text as a
/// literal, and every such text has Korean (from the macOS catalog or an `i18n/ko*.json`); the text
/// of `L10n.p` is a catalog key with plural variants. A computed key would escape the check, so it
/// fails the test too.
#[test]
fn qml_texts_have_korean() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("qml");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("the qml directory reads")
        .map(|entry| entry.expect("a qml directory entry reads").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "qml"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no QML files in {}", dir.display());

    let mut checked = 0;
    let mut missing = Vec::new();
    let mut computed = Vec::new();
    let mut not_plural = Vec::new();
    for path in files {
        let source = std::fs::read_to_string(&path).expect("a QML file reads");
        let name = path.file_name().expect("a QML file has a name").to_string_lossy().into_owned();
        for (offset, _) in source.match_indices("L10n.") {
            let call = &source[offset + "L10n.".len()..];
            let Some((function, arguments)) =
                ["t(", "f(", "p("].into_iter().find_map(|function| Some((function, call.strip_prefix(function)?)))
            else {
                continue;
            };
            let line = source[..offset].matches('\n').count() + 1;
            match qml_literal(arguments.trim_start()) {
                Some((text, after)) if after.trim_start().starts_with([')', ',']) => {
                    checked += 1;
                    if table::KOREAN.binary_search_by(|(key, _)| (*key).cmp(text.as_str())).is_err() {
                        missing.push(format!("{name}:{line}: {text:?}"));
                    }
                    let plural = table::ENGLISH_ONE.binary_search_by(|(key, _)| (*key).cmp(text.as_str())).is_ok();
                    if function == "p(" && !plural {
                        not_plural.push(format!("{name}:{line}: {text:?}"));
                    }
                }
                _ => computed.push(format!("{name}:{line}")),
            }
        }
    }
    assert!(checked > 0, "no L10n.t or L10n.f calls found");
    assert!(computed.is_empty(), "L10n calls whose text is not a single literal:\n{}", computed.join("\n"));
    assert!(missing.is_empty(), "QML texts without Korean (add them to an i18n/ko-*.json):\n{}", missing.join("\n"));
    assert!(
        not_plural.is_empty(),
        "L10n.p texts that are not catalog keys with plural variants (use L10n.f):\n{}",
        not_plural.join("\n")
    );
}
