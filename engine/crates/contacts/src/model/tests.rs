use super::*;

fn d(y: i32, m: u32, day: u32) -> CivilDate {
    CivilDate::new(y, m, day).unwrap()
}

#[test]
fn prompt_order() {
    assert_eq!(ContactsPrompt::from_state(false, false, true), ContactsPrompt::PcConsentRequired);
    assert_eq!(ContactsPrompt::from_state(true, false, false), ContactsPrompt::DeviceConsentRequired);
    assert_eq!(ContactsPrompt::from_state(true, false, true), ContactsPrompt::DeviceConsentDenied);
    assert_eq!(ContactsPrompt::from_state(true, true, false), ContactsPrompt::None);
}

#[test]
fn dates() {
    let today = d(2026, 10, 21);
    let date = |text: &str| parse_display_date(text, today).map(|p| (p.date, p.ticks));

    assert_eq!(date("2026-10-22"), Some((d(2026, 10, 22), 0)));
    assert_eq!(date(" 2026/1/2 "), Some((d(2026, 1, 2), 0)));
    assert_eq!(date("12/25/1990"), Some((d(1990, 12, 25), 0)));
    assert_eq!(date("10/30"), Some((d(2026, 10, 30), 0)));
    assert_eq!(date("10.30.26"), Some((d(2026, 10, 30), 0)));
    assert_eq!(date("1/1/49"), Some((d(2049, 1, 1), 0)), "TwoDigitYearMax 2049");
    assert_eq!(date("1/1/50"), Some((d(1950, 1, 1), 0)));
    assert_eq!(date("10/22/0026"), Some((d(26, 10, 22), 0)), "four digits as written");
    assert_eq!(date("2026-10-22T12:30"), Some((d(2026, 10, 22), 45_000 * TICKS_PER_SECOND)));
    assert_eq!(date("2026-10-22 00:00:01.5"), Some((d(2026, 10, 22), 15_000_000)));
    assert_eq!(date("oct. 22, 26"), Some((d(2026, 10, 22), 0)));
    assert_eq!(date("2026, Oct 22"), Some((d(2026, 10, 22), 0)));
    assert_eq!(date("May. 3"), Some((d(2026, 5, 3), 0)));

    for unparsable in [
        "--10-22",
        "2026-02-30",
        "tomorrow",
        "10/22-2026",
        "2026-10",
        "22 Oct",
        "October. 22",
        "Sept 22",
        "10/22/126",
        "2026-10-22T24:00",
        "2026-10-22T9:05",
        "2026-Oct-22",
        "Oct-22",
        "2026-10-22T00:00:00Z",
        "Thursday, October 22, 2026",
    ] {
        assert_eq!(date(unparsable), None, "{unparsable}");
    }

    assert_eq!(date("2024-02-29"), Some((d(2024, 2, 29), 0)));
    assert_eq!(CivilDate::new(2026, 13, 1), None);
}

#[test]
fn banner_window() {
    let today = d(2026, 12, 31);

    assert_eq!(birthday_banner("2026-12-30", today), BirthdayBanner::Yesterday);
    assert_eq!(birthday_banner("2026-12-29", today), BirthdayBanner::None);
    assert_eq!(birthday_banner("12/31", today), BirthdayBanner::Today);
    assert_eq!(birthday_banner("2027-01-01", today), BirthdayBanner::Tomorrow);
    assert_eq!(birthday_banner("2027-01-15", today), BirthdayBanner::Soon);
    assert_eq!(birthday_banner("2027-01-16", today), BirthdayBanner::None);

    // A birth year in the past is days in the past: no banner (as on Windows).
    assert_eq!(birthday_banner("1990-12-31", today), BirthdayBanner::None);
    assert_eq!(birthday_banner("", today), BirthdayBanner::None);

    // A time of day counts, truncated toward zero: noon yesterday is still "today".
    assert_eq!(birthday_banner("2026-12-30T12:00", today), BirthdayBanner::Today);
    assert_eq!(birthday_banner("2026-12-30T00:00", today), BirthdayBanner::Yesterday);
    assert_eq!(birthday_banner("2027-01-15T23:59:59.9999999", today), BirthdayBanner::Soon);
}
