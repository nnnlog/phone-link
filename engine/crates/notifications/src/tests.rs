use super::*;
use phonelink_core::codec::valueset::Point;
use phonelink_core::time::filetime_from_unix_ms;

#[test]
fn text_forms() {
    assert_eq!(value_text(&Value::DateTime(0)).as_deref(), Some("1601-01-01T00:00:00.0000000+00:00"));

    // 2024-02-29T12:34:56.1234567Z.
    let leap = filetime_from_unix_ms(1_709_210_096_123) + 4_567;
    assert_eq!(value_text(&Value::DateTime(leap)).as_deref(), Some("2024-02-29T12:34:56.1234567+00:00"));
    assert_eq!(value_text(&Value::DateTime(-1)).as_deref(), Some("1600-12-31T23:59:59.9999999+00:00"));

    assert_eq!(value_text(&Value::TimeSpan(0)).as_deref(), Some("00:00:00"));
    assert_eq!(value_text(&Value::TimeSpan(2 * TICKS_PER_DAY + 10_000_000)).as_deref(), Some("2.00:00:01"));
    assert_eq!(value_text(&Value::TimeSpan(i64::MIN)).as_deref(), Some("-10675199.02:48:05.4775808"));

    assert_eq!(value_text(&Value::Point(Point { x: -0.5, y: 3.0 })).as_deref(), Some("-0.5,3"));
    assert_eq!(value_text(&Value::Char16(0xD800)).as_deref(), Some("\u{FFFD}"));
    assert_eq!(value_text(&Value::Empty), None);
    assert_eq!(value_text(&Value::StringArray(vec![])), None);
    assert_eq!(value_text(&Value::Object(Default::default())), None);
}
