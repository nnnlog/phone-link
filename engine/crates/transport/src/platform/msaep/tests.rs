use super::*;

#[test]
fn device_filter() {
    assert!(DeviceFilter::Any.accepts("x"));
    assert!(DeviceFilter::Only(vec!["a".into()]).accepts("a"));
    assert!(!DeviceFilter::Only(vec!["a".into()]).accepts("A"));
    assert!(DeviceFilter::Only(vec![]).accepts("a"), "the empty list means any publisher");
    assert!(!DeviceFilter::RejectAll.accepts("a"));
}

#[test]
fn tag_widening_sign_extends() {
    assert_eq!(i32::from(-1i16), -1);
    assert_eq!(i32::from(9i16), 9);
}
