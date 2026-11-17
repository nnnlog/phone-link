use super::*;

#[test]
fn region_comes_from_the_locale_territory() {
    assert_eq!(region_of_locale("ko_KR.UTF-8").as_deref(), Some("KR"));
    assert_eq!(region_of_locale("en_US@euro").as_deref(), Some("US"));
    assert_eq!(region_of_locale("es_419.UTF-8"), None);
    assert_eq!(region_of_locale("C.UTF-8"), None);
}

/// The region for an environment holding `variables`.
fn region_with(variables: &[(&str, &str)]) -> Option<String> {
    region_of_environment(|name| {
        variables.iter().find(|(variable, _)| *variable == name).map(|(_, value)| (*value).to_owned())
    })
}

#[test]
fn the_region_setting_wins_over_the_display_language() {
    let set = [("LANG", "en_US.UTF-8"), ("LC_MESSAGES", "en_US.UTF-8"), ("LC_MONETARY", "ko_KR.UTF-8")];
    assert_eq!(region_with(&set).as_deref(), Some("KR"));
    assert_eq!(region_with(&[("LC_MESSAGES", "de_DE.UTF-8")]), None);
}

#[test]
fn the_first_region_variable_with_a_value_decides() {
    let all = [
        ("LANG", "en_US.UTF-8"),
        ("LC_MONETARY", "de_DE.UTF-8"),
        ("LC_ADDRESS", "fr_FR.UTF-8"),
        ("LC_TELEPHONE", "ja_JP.UTF-8"),
    ];
    assert_eq!(region_with(&all).as_deref(), Some("JP"));
    assert_eq!(region_with(&all[..3]).as_deref(), Some("FR"));
    assert_eq!(region_with(&all[..2]).as_deref(), Some("DE"));
    assert_eq!(region_with(&all[..1]).as_deref(), Some("US"));
    assert_eq!(region_with(&[("LC_ALL", "C.UTF-8"), ("LC_TELEPHONE", "ja_JP.UTF-8")]), None);
    assert_eq!(region_with(&[("LC_ALL", ""), ("LC_TELEPHONE", "ja_JP.UTF-8")]).as_deref(), Some("JP"));
}

#[test]
fn the_host_name_ends_at_its_nul() {
    assert_eq!(name_in(b"phonelink-pc\0\0\0"), "phonelink-pc");
    assert_eq!(name_in(b"no-nul"), "no-nul");
    assert_eq!(name_in(b"caf\xc3\xa9\0"), "café");
    assert_eq!(name_in(b"bad\xff\0"), "bad\u{fffd}");
}

#[test]
fn the_host_name_is_read() {
    assert!(!LinuxHost.host_name().is_empty());
}
