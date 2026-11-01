use super::*;

#[test]
fn flags_round_trip_and_keep_bits_the_phone_reported() {
    let reported = AppFlags(0b1101); // feed, no banner, two unknown bits
    let ui: AppNotificationFlags = reported.into();
    assert_eq!(ui, AppNotificationFlags { feed: true, banner: false, other_bits: 0b1100 });
    assert_eq!(AppFlags::from(ui), reported);

    let changed = AppNotificationFlags { banner: true, ..ui };
    assert_eq!(AppFlags::from(changed), AppFlags(0b1111));
}
