use super::*;

#[test]
fn milliseconds_on_both_sides_of_the_epoch() {
    for ms in [0, 1, 1_700_000_000_123, -1, -86_400_000] {
        assert_eq!(unix_ms(system_time(ms)), ms, "{ms}");
    }
    assert_eq!(checked_system_time(i64::MAX).map(unix_ms), checked_system_time(i64::MAX).map(|_| i64::MAX));
}

#[test]
fn seconds_round_down_toward_zero() {
    assert_eq!(unix_secs(system_time(1_500)), 1);
    assert_eq!(unix_secs(system_time(-1_500)), -1);
}

#[test]
fn ticks_and_filetime_meet_at_the_epoch() {
    assert_eq!(ticks_from_unix_ms(0), UNIX_EPOCH_TICKS);
    assert_eq!(unix_ms_from_ticks(ticks_from_unix_ms(1_760_781_600_123)), 1_760_781_600_123);
    assert_eq!(unix_ms_from_filetime(filetime_from_unix_ms(1_700_000_000_123)), 1_700_000_000_123);
    assert_eq!(unix_ms_from_filetime(UNIX_EPOCH_FILETIME), 0);
    assert_eq!(unix_ms_from_filetime(0), FILETIME_ZERO_UNIX_MS);
    assert_eq!(FILETIME_ZERO_UNIX_MS, -11_644_473_600 * 1000);
    assert_eq!(filetime(UNIX_EPOCH), UNIX_EPOCH_FILETIME);
    assert_eq!(filetime(system_time(-5)), UNIX_EPOCH_FILETIME, "before the epoch counts as the epoch");
}

#[test]
fn calendar_round_trips() {
    for (y, m, d) in [(1970, 1, 1), (2000, 2, 29), (1969, 12, 31), (2024, 12, 31), (1600, 3, 1)] {
        let days = days_from_civil(y, m, d);
        assert_eq!(civil_from_days(days), (y, m, d));
    }

    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_in_month(2024, 2), 29);
    assert_eq!(days_in_month(1900, 2), 28);
    assert_eq!(days_in_month(2000, 2), 29);
    assert_eq!(days_in_month(2025, 4), 30);
}

#[test]
fn date_times_need_a_valid_date_time_and_an_offset() {
    let base = 1_792_454_400_000;
    for (text, want) in [
        ("2026-10-20T00:00:00Z", Some(base)),
        ("2026-10-20t00:00:00z", Some(base)),
        ("2026-10-20T00:00:00.1234567+00:00", Some(base + 123)),
        ("2026-10-20T09:00:00+09:00", Some(base)),
        ("2026-10-19T19:30:00-04:30", Some(base)),
        ("2026-10-20T00:00:00", None),
        ("2026-10-20T00:00:00Z ", None),
        ("2026-10-20T00:00:00.Z", None),
        ("2026-10-20T00:00:00+15:00", None),
        ("2026-02-30T00:00:00Z", None),
        ("2026-15-01T00:00:00Z", None),
        ("2026-10-20T24:00:00Z", None),
        ("2026-10-20T00:60:00Z", None),
        ("+026-10-20T00:00:00Z", None),
        ("2026-10-20T00:00:00+0\u{e9}:00", None),
        ("20 Oct 2026", None),
    ] {
        assert_eq!(parse_date_time(text), want, "{text:?}");
    }
}
