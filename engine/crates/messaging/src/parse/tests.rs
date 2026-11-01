use super::items::{is_delete, message_row_is_delete, rcs_date, resolve, row_count, split_list};
use super::*;
use crate::consts::{UNIX_MS_MAX, UNIX_MS_MIN};
use phonelink_core::time::FILETIME_ZERO_UNIX_MS;
use std::collections::HashMap;

#[test]
fn date_clamping() {
    assert_eq!(clamp_message_date(1_700_000_000_000), 1_700_000_000_000);
    assert_eq!(clamp_message_date(UNIX_MS_MAX + 1), FILETIME_ZERO_UNIX_MS);
    assert_eq!(clamp_message_date(UNIX_MS_MIN - 1), FILETIME_ZERO_UNIX_MS);
    assert_eq!(clamp_message_date(FILETIME_ZERO_UNIX_MS), FILETIME_ZERO_UNIX_MS, "not after 1601");
    assert_eq!(clamp_message_date(FILETIME_ZERO_UNIX_MS + 1), FILETIME_ZERO_UNIX_MS + 1);
    assert_eq!(clamp_message_date(UNIX_MS_MIN), FILETIME_ZERO_UNIX_MS);

    assert!(rcs_date(UNIX_MS_MAX, "dates").is_ok());
    assert!(rcs_date(UNIX_MS_MAX + 1, "dates").is_err());
    assert_eq!(rcs_date(UNIX_MS_MIN, "dates"), Ok(UNIX_MS_MIN), "RCS dates are not clamped");
}

#[test]
fn row_counts() {
    assert_eq!(row_count(2, &[3, 2], 0, "x"), Ok(2));
    assert_eq!(row_count(2, &[3, 3], 1, "x"), Ok(2), "an absent optional array counts as itemCount");
    assert_eq!(row_count(2, &[3, 3], 0, "x"), Ok(3), "rows run over the shortest present array");
    assert!(row_count(3, &[3, 2], 0, "x").is_err());
    assert_eq!(row_count(-1, &[3], 1, "x"), Ok(0));
}

#[test]
fn message_rows_accept_only_sync_actions() {
    let actions = [-1, 0, 1, 2, 3, -2];
    let at = |i| message_row_is_delete(Some(&actions), i);
    assert_eq!((at(0), at(1), at(2), at(3)), (Ok(false), Ok(false), Ok(false), Ok(true)));
    assert!(at(4).is_err() && at(5).is_err(), "outside SyncAction");
    assert_eq!(message_row_is_delete(None, 0), Ok(false), "no actions: insert or replace");
    assert!(!is_delete(Some(&actions), 4), "the other rows insert or replace on any other value");
}

#[test]
fn lists_split_and_drop_empty_entries() {
    assert_eq!(split_list("a,,b,", ','), vec!["a", "b"]);
    assert_eq!(split_list("a, b", ','), vec!["a", " b"], "no trimming");
    let map: HashMap<String, String> = [("1".into(), "+1".into())].into();
    assert_eq!(resolve(" 1  2 ", &map), vec!["+1"]);
}
