//! The file system's unit tests: the items the mount refuses and the permission bits it gives.

use std::ffi::OsStr;

use fuser::INodeNo;

use super::attrs::permissions;
use super::entries::refused;
use crate::phone_files::inodes::ROOT;

#[test]
fn trash_folders_are_refused_in_the_root_only() {
    let root = INodeNo(ROOT);
    assert!(refused(root, OsStr::new(".Trash")));
    assert!(refused(root, OsStr::new(".Trash-1000")));
    assert!(!refused(root, OsStr::new(".Trashcan")));
    assert!(!refused(root, OsStr::new(".trash-1000")));
    assert!(!refused(root, OsStr::new("Trash")));
    assert!(!refused(root, OsStr::new("x.Trash-1000")));
    assert!(!refused(INodeNo(ROOT + 1), OsStr::new(".Trash-1000")));
}

#[test]
fn files_are_read_only_and_folders_take_new_items() {
    assert_eq!(permissions(false), 0o444);
    assert_eq!(permissions(true), 0o755);
}
