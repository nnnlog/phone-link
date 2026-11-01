use super::*;

#[test]
fn config_sync_bytes() {
    assert_eq!(config_sync_request(), vec![0x0a, 0x00]);
}

#[test]
fn incremental_encodes_every_optional() {
    let r = metadata_sync_request(false, 1234, false, Some(false));
    let bytes = r.encode_to_vec();
    // files_metadata_sync_request { 08 02 10 d2 09 18 00 30 00 }
    assert_eq!(&bytes[..11], &[0x12, 0x09, 0x08, 0x02, 0x10, 0xd2, 0x09, 0x18, 0x00, 0x30, 0x00]);
    assert_eq!(bytes[11], 0x22);
    assert_eq!(bytes[12], 36);
}

#[test]
fn full_sync_bytes() {
    let bytes = metadata_sync_request(true, -1, false, None).encode_to_vec();
    let mut want = vec![0x12, 0x0f, 0x08, 0x01, 0x10];
    want.extend([0xff; 9]);
    want.extend([0x01, 0x18, 0x00, 0x22, 36]);
    assert_eq!(&bytes[..want.len()], want.as_slice());
}

#[test]
fn status_thirteen_is_directory_not_empty() {
    assert_eq!(Status::from_wire(13), Status::DirectoryNotEmpty);
    assert_eq!(Status::from_wire(1), Status::Success);
    assert_eq!(Status::from_wire(0), Status::Other(0));
    assert_eq!(Status::from_wire(42), Status::Other(42));
}

#[test]
fn upload_id_by_version() {
    assert_eq!(upload_id(Some("1.24061.0.0"), "\\storage\\a"), "\\storage\\a");
    assert_ne!(upload_id(Some("1.24062.83.0"), "\\storage\\a"), "\\storage\\a");
    assert_ne!(upload_id(Some("1.0.0"), "\\storage\\a"), "\\storage\\a");
    assert_eq!(upload_id(None, "\\storage\\a"), "\\storage\\a");
}
