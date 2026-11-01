use super::*;

#[test]
fn invalid_file_name_characters() {
    let name: String = "a<b>c:d\"e|f?g*h\\i/j\u{1}k".chars().filter(|c| !is_invalid_file_name_char(*c)).collect();
    assert_eq!(name, "abcdefghijk");
}

#[test]
fn carrier_limit_defaults() {
    let l = CarrierLimits::of(None);
    assert_eq!((l.max_image_width, l.max_image_height, l.max_message_size), (1944, 2592, 614_400));
    assert_eq!((l.max_text_length, l.max_recipients), (None, None));

    let s = SubscriptionRow {
        max_image_width: 0,
        max_image_height: 100,
        max_message_size: -1,
        max_message_text_length: 160,
        recipient_limit: 1,
        ..Default::default()
    };

    let l = CarrierLimits::of(Some(&s));
    assert_eq!((l.max_image_width, l.max_image_height, l.max_message_size), (1944, 100, 614_400));
    assert_eq!((l.max_text_length, l.max_recipients), (Some(160), None));

    assert!(is_google_rcs(&["x@RCS.google.com".into()]));
}
