use super::*;
use phonelink_proto::device_info::v1::WallpaperResponseMetadata;

fn entry(t: WallpaperType, id: i64, payload: Option<&[u8]>) -> WallpaperResponseMetadata {
    WallpaperResponseMetadata { id, wallpaper_type: t as i32, payload: payload.map(<[u8]>::to_vec) }
}

#[test]
fn android_is_the_os_name_or_its_number() {
    let item = |os: Option<&str>| DeviceInfoItem { os_name: os.map(str::to_string), ..Default::default() };

    for os in ["Android", "android", " ANDROID ", "2"] {
        assert!(is_android(&item(Some(os))), "{os:?}");
    }
    for os in [None, Some("iOS"), Some("Windows"), Some("3"), Some("Androids")] {
        assert!(!is_android(&item(os)), "{os:?}");
    }
}

#[test]
fn reply_rules() {
    let ok = |wallpapers| WallpaperResponseMessage { wallpapers, status: 1 }.encode_to_vec();

    assert_eq!(
        read_pull_reply(&ok(vec![
            entry(WallpaperType::Homescreen, 4, Some(b"h")),
            entry(WallpaperType::Lockscreen, 5, None),
        ])),
        Ok(CrossDeviceChange::Upsert(vec![(HOME_SCREEN, 4, b"h".to_vec())])),
        "an entry without payload is unchanged"
    );
    assert_eq!(
        read_pull_reply(&ok(vec![entry(WallpaperType::Lockscreen, 6, Some(b""))])),
        Ok(CrossDeviceChange::Upsert(vec![(LOCK_SCREEN, 6, Vec::new())])),
        "an empty payload is present"
    );
    assert_eq!(
        read_pull_reply(&ok(vec![
            entry(WallpaperType::Homescreen, 4, Some(b"h")),
            entry(WallpaperType::Unspecified, 0, None)
        ])),
        Err(PullReplyError::WallpaperType(0))
    );

    for status in [0, 2, 3] {
        let reply =
            WallpaperResponseMessage { wallpapers: vec![entry(WallpaperType::Homescreen, 1, Some(b"x"))], status };
        assert_eq!(read_pull_reply(&reply.encode_to_vec()), Ok(CrossDeviceChange::DeleteAll), "status {status}");
    }

    assert_eq!(read_pull_reply(&[]), Ok(CrossDeviceChange::DeleteAll), "an empty reply is UNSPECIFIED");
    assert!(matches!(read_pull_reply(&[0x0A, 0x05]), Err(PullReplyError::Decode(_))));
}

#[test]
fn the_pull_payload_is_the_page_vector() {
    assert_eq!(
        pull_payload(None, None).encode_to_vec(),
        [
            0x0A, 0x0D, 0x08, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x10, 0x01, 0x0A, 0x0D, 0x08,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x10, 0x02
        ]
    );
    assert_eq!(pull_payload(Some(7), None).current_wallpapers[0].id, 7);
}
