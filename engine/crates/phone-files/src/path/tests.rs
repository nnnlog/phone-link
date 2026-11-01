use super::*;

#[test]
fn maps_the_storage_prefix() {
    assert_eq!(map_path("/storage/emulated/0/DCIM/Camera/a.jpg").unwrap(), "storage\\DCIM\\Camera\\a.jpg");
    assert_eq!(map_path("/Storage/Emulated/0/x").unwrap(), "storage\\x");
    // No component boundary check.
    assert_eq!(map_path("/storage/emulated/01/x").unwrap(), "storage1\\x");
    assert_eq!(map_path("/storage/1234-ABCD/x").unwrap(), "storage\\1234-ABCD\\x");
}

#[test]
fn rejects_dots_and_the_root() {
    assert_eq!(map_path("/storage/emulated/0/../x"), Err(Rejected::DotComponent));
    assert_eq!(map_path("/storage/emulated/0/.../x"), Err(Rejected::DotComponent));
    assert_eq!(map_path("/storage/emulated/0"), Err(Rejected::StorageRoot));
    assert_eq!(map_path("/storage/emulated/"), Err(Rejected::StorageRoot));
    assert!(map_path("/storage/emulated/0/.hidden").is_ok());
}

#[test]
fn cleans_names() {
    assert_eq!(clean_name(" a:b*?.txt "), "ab.txt");
    assert_eq!(clean_name("con"), "_con");
    assert_eq!(clean_name("COM7"), "_COM7");
    assert_eq!(clean_name("COM10"), "COM10");
    assert_eq!(clean_name("a\u{1}b"), "ab");
}

#[test]
fn location_parts() {
    assert_eq!(parent("storage\\DCIM\\a.jpg"), "storage\\DCIM");
    assert_eq!(parent("storage"), "");
    assert_eq!(leaf("storage\\DCIM\\a.jpg"), "a.jpg");
    assert_eq!(remote("storage\\Download\\r.pdf"), "\\storage\\Download\\r.pdf");
    assert!(is_within("storage\\DCIM\\a", "Storage\\dcim"));
    assert!(!is_within("storage\\DCIMX", "storage\\DCIM"));
    assert_eq!(recycle_bin("Recycle bin - Connected device").unwrap(), "storage\\Recycle bin - Connected device");
    assert_eq!(recycle_bin(""), None);
}

#[test]
fn ignored_names() {
    assert!(ignored(".DS_Store"));
    assert!(ignored("a.TMP"));
    assert!(ignored("desktop.ini"));
    assert!(!ignored("report.pdf"));
}
