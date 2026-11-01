use super::*;

#[test]
fn catalog_has_fifty_unique_entries_and_classifies_exactly() {
    let mut names = content_type::CATALOG.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 50);

    assert_eq!(content_type::CATALOG[9], "heartbeat");
    assert_eq!(content_type::CATALOG[43], "permissions");

    assert_eq!(content_type::classify(Some("heartbeat")), "heartbeat");
    assert_eq!(content_type::classify(Some("Heartbeat")), "test");
    assert_eq!(content_type::classify(Some("connect")), "test");
    assert_eq!(content_type::classify(Some("Permissions")), "test");
    assert_eq!(content_type::classify(None), "test");
}

#[test]
fn default_capability_sets_are_the_named_bits() {
    use capability::*;
    let phone = PHOTOS
        | MESSAGES
        | MIRROR
        | PHONE_APPS
        | CALLS
        | NOTIFICATIONS_LISTENER
        | CALL_LOGS
        | WALLPAPER
        | SHARED_CONTENT
        | CONTACTS
        | AUDIO_CONTROL;

    assert_eq!(phone, ANDROID_PHONE_DEFAULT);
    assert_eq!(phone & !(MESSAGES | CALLS | CALL_LOGS), ANDROID_TABLET_DEFAULT);
    assert_eq!(permission::NOTIFICATIONS_SENSITIVE as i32, i32::MIN);
}
