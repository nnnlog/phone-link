use super::*;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_owned()).collect()
}

fn sorted(locale: Option<&str>, list: &[&str]) -> Vec<String> {
    let collator = NameCollator::new(locale);
    let given = names(list);
    collator.order(&given).into_iter().map(|index| given[index as usize].clone()).collect()
}

#[test]
fn case_diacritics_kana_and_width_are_ignored() {
    let collator = NameCollator::new(None);
    assert_eq!(collator.compare("camera", "Camera"), Ordering::Equal);
    assert_eq!(collator.compare("Éclair", "eclair"), Ordering::Equal);
    assert_eq!(collator.compare("ひらがな", "ヒラガナ"), Ordering::Equal);
    assert_eq!(collator.compare("ｶﾒﾗ", "カメラ"), Ordering::Equal);
    assert_eq!(collator.compare("Ｍａｐｓ", "maps"), Ordering::Equal);
}

#[test]
fn equal_names_keep_their_order() {
    let collator = NameCollator::new(None);
    assert_eq!(collator.order(&names(&["b", "A", "a", "B"])), [1, 2, 0, 3]);
    assert!(collator.order(&[]).is_empty());
}

#[test]
fn names_sort_by_the_locale() {
    assert_eq!(sorted(Some("en-US"), &["Zoom", "Ärzte", "Bank"]), ["Ärzte", "Bank", "Zoom"]);
    // Swedish puts Ä after Z.
    assert_eq!(sorted(Some("sv-SE"), &["Zoom", "Ärzte", "Bank"]), ["Bank", "Zoom", "Ärzte"]);
    // Korean puts Hangul before the Latin script.
    assert_eq!(
        sorted(Some("ko-KR"), &["카메라", "갤러리", "Maps", "나의 앱"]),
        ["갤러리", "나의 앱", "카메라", "Maps"]
    );
    assert_eq!(sorted(None, &["카메라", "Maps"]), ["Maps", "카메라"]);
}

#[test]
fn session_locale_names_become_bcp47() {
    assert_eq!(collation_locale_name(Some("ko_KR.UTF-8")).as_deref(), Some("ko-KR"));
    assert_eq!(collation_locale_name(Some("sr_RS@latin")).as_deref(), Some("sr-RS"));
    assert_eq!(collation_locale_name(Some("C.UTF-8")), None);
    assert_eq!(collation_locale_name(Some("POSIX")), None);
    assert_eq!(collation_locale_name(None), None);
}

#[test]
fn an_unreadable_locale_sorts_in_the_root_order() {
    assert_eq!(sorted(Some("not a locale!"), &["b", "a"]), ["a", "b"]);
}
