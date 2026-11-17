use super::*;

#[test]
fn gsettings_strings_are_unquoted_and_unescaped() {
    assert_eq!(
        gvariant_string("'file:///usr/share/backgrounds/a.png'\n").as_deref(),
        Some("file:///usr/share/backgrounds/a.png")
    );
    assert_eq!(gvariant_string("\"file:///home/u/it's.jpg\"").as_deref(), Some("file:///home/u/it's.jpg"));
    assert_eq!(gvariant_string(r"'a\'b\\cé'").as_deref(), Some("a'b\\cé"));
    assert_eq!(gvariant_string("''").as_deref(), Some(""));
    assert_eq!(gvariant_string("'zoom'").as_deref(), Some("zoom"));
    assert_eq!(gvariant_string("uint32 3"), None);
    assert_eq!(gvariant_string("'unterminated"), None);
    assert_eq!(gvariant_string("'a' 'b'"), None);
}
