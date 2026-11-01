use super::*;
use prost_types::Timestamp;

fn item(seconds: i64, nanos: i32) -> MediaMetadata {
    MediaMetadata { date_modified: Some(Timestamp { seconds, nanos }), ..Default::default() }
}

#[test]
fn timestamps_outside_the_valid_range_are_invalid() {
    assert_eq!(date_modified_ns(&item(1, 250)), Some(1_000_000_200));
    assert_eq!(date_modified_ns(&item(MAX_SECONDS, 0)), Some(i128::from(MAX_SECONDS) * 1_000_000_000));
    assert_eq!(date_modified_ns(&item(MAX_SECONDS + 1, 0)), None);
    assert_eq!(date_modified_ns(&item(MIN_SECONDS - 1, 0)), None);
    assert_eq!(date_modified_ns(&item(0, -1)), None);
    assert_eq!(date_modified_ns(&item(0, 1_000_000_000)), None);
    assert_eq!(date_modified_ns(&MediaMetadata::default()), None);
}

#[test]
fn the_window_is_signed() {
    let w = Duration::from_secs(120);
    assert!(within(1_000, 0, w));
    assert!(!within(120_000_000_000, 0, w));
    assert!(within(119_999_999_999, 0, w));
    assert!(within(0, 50_000_000_000, w), "a time in the future is within the window");
}

#[test]
fn permission_replies() {
    let reply = |status: PermissionsRequestStatus, photos: bool| {
        PermissionsResponseMessage {
            request_status: status as i32,
            feature_permissions: if photos { vec![FeaturePermissions::Photos as i32] } else { vec![] },
            ..Default::default()
        }
        .encode_to_vec()
    };

    assert_eq!(permission_outcome(&reply(PermissionsRequestStatus::Accepted, true)), PermissionOutcome::Granted);
    assert_eq!(permission_outcome(&reply(PermissionsRequestStatus::Accepted, false)), PermissionOutcome::RequestSent);
    assert!(matches!(
        permission_outcome(&reply(PermissionsRequestStatus::OtherFailure, true)),
        PermissionOutcome::Failed(_)
    ));
    assert!(matches!(permission_outcome(&[0xff]), PermissionOutcome::Failed(_)));
    assert!(matches!(permission_outcome(&[]), PermissionOutcome::Failed(_)));
}

#[test]
fn media_changes() {
    let m = |change_type: i32, n: usize| {
        MediaChangeMessage {
            change_type,
            changed_media: (0..n).map(|i| MediaMetadata { media_id: i as i64 + 7, ..Default::default() }).collect(),
        }
        .encode_to_vec()
    };

    assert!(matches!(parse_media_change(&m(1, 2)), Ok(MediaChange::NewPhoto(first)) if first.media_id == 7));
    assert_eq!(parse_media_change(&m(1, 0)), Ok(MediaChange::NewPhotoWithoutItem));
    assert_eq!(parse_media_change(&m(0, 1)), Ok(MediaChange::Other(0)));
    assert_eq!(parse_media_change(&m(5, 1)), Ok(MediaChange::Other(5)));
    assert_eq!(parse_media_change(&[]), Ok(MediaChange::Other(0)));
    assert!(parse_media_change(&[0x12, 0x05]).is_err());
}
