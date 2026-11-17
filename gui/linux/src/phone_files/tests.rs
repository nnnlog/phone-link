//! The phone files' unit tests: the errno of each failure, the staging folder, the processes being
//! killed, and what the kernel must forget when the phone's tree changes.

use std::ffi::OsString;
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fuser::Errno;
use phonelink_engine::api::phone_files::{PhoneFileChanges, PhoneFileError, PhoneFileFailure, PhoneFileItem};

use super::errno::errno;
use super::inodes::{Inodes, ROOT};
use super::notes::Note;
use super::requesters::kill_pending;
use super::staging::{private_dir, staging_dir};

#[test]
fn failures_map_to_errnos() {
    let cases = [
        (PhoneFileFailure::NotRunning, Errno::EIO),
        (PhoneFileFailure::UnknownItem, Errno::ENOENT),
        (PhoneFileFailure::Exists, Errno::EEXIST),
        (PhoneFileFailure::Ignored, Errno::ENOTSUP),
        (PhoneFileFailure::NotSupported, Errno::ENOTSUP),
        (PhoneFileFailure::NotInSync, Errno::EIO),
        (PhoneFileFailure::PermissionRequired, Errno::EACCES),
        (PhoneFileFailure::PhoneUnavailable, Errno::EIO),
        (PhoneFileFailure::LtwOff, Errno::EACCES),
        (PhoneFileFailure::Metered, Errno::EIO),
        (PhoneFileFailure::PcLocked, Errno::EACCES),
        (PhoneFileFailure::TimedOut, Errno::EIO),
        (PhoneFileFailure::FileNotFound, Errno::ENOENT),
        (PhoneFileFailure::Cancelled, Errno::EINTR),
        (PhoneFileFailure::InProgress, Errno::EBUSY),
        (PhoneFileFailure::Partial, Errno::EIO),
        (PhoneFileFailure::Failed, Errno::EIO),
    ];
    for (failure, expected) in cases {
        let error = PhoneFileError::Failure { failure, message: String::new() };
        assert_eq!(errno(&error), expected, "{failure:?}");
    }
}

#[test]
fn new_files_are_staged_in_the_cache_folder() {
    let home = || Some(OsString::from("/home/u"));
    let from_home = Some(PathBuf::from("/home/u/.cache/phonelink/uploads"));
    assert_eq!(staging_dir(Some("/cache".into()), home()), Some(PathBuf::from("/cache/phonelink/uploads")));
    assert_eq!(staging_dir(None, home()), from_home);
    assert_eq!(staging_dir(Some("".into()), home()), from_home);
    assert_eq!(staging_dir(Some("cache".into()), home()), from_home);
    assert_eq!(staging_dir(None, None), None);
    assert_eq!(staging_dir(Some("cache".into()), Some("home".into())), None);
}

#[test]
fn a_pending_sigkill_marks_a_dying_process() {
    let status = |thread: &str, process: &str| {
        format!("Name:\tcp\nState:\tD (disk sleep)\nSigQ:\t0/63\nSigPnd:\t{thread}\nShdPnd:\t{process}\nSigBlk:\t0\n")
    };
    assert!(!kill_pending(&status("0000000000000000", "0000000000000000")));
    assert!(kill_pending(&status("0000000000000100", "0000000000000000")));
    assert!(kill_pending(&status("0000000000000000", "0000000000000102")));
    // SIGINT alone (bit 2) is not a kill.
    assert!(!kill_pending(&status("0000000000000002", "0000000000000000")));
    assert!(!kill_pending("Name:\tcp\n"));
}

fn item(identifier: &str, parent: &str, name: &str) -> PhoneFileItem {
    PhoneFileItem {
        identifier: identifier.to_owned(),
        parent: parent.to_owned(),
        name: name.to_owned(),
        is_directory: false,
        size: 1,
        created: None,
        modified: None,
        file_type: 7,
        content_version: "1".to_owned(),
        metadata_version: "1".to_owned(),
        has_thumbnail: false,
        synthetic: false,
    }
}

/// An inode table that gave the kernel the folder `d` in the root and the files `a` (looked up as
/// `a.txt` in `d`) and `b` (listed only).
fn known_tree() -> (Inodes, u64, u64, u64) {
    let mut inodes = Inodes::default();
    let d = inodes.of("d");
    inodes.record(d, ROOT, "Docs");
    let a = inodes.of("a");
    inodes.record(a, d, "a.txt");
    let b = inodes.of("b");
    (inodes, d, a, b)
}

#[test]
fn an_updated_item_drops_its_attributes_and_a_cached_miss_of_its_name() {
    let (mut inodes, d, a, _) = known_tree();
    let changes = PhoneFileChanges { updated: vec![item("a", "d", "a.txt")], removed: vec![], generation: 2 };
    assert_eq!(
        inodes.notes_for(&changes),
        vec![Note::Inode { ino: a }, Note::Entry { parent: d, name: "a.txt".to_owned() }, Note::Inode { ino: d }]
    );
    assert_eq!(inodes.entries.get(&a), Some(&(d, "a.txt".to_owned())));
}

#[test]
fn a_renamed_or_moved_item_is_a_deletion_of_its_old_entry() {
    let (mut inodes, d, a, _) = known_tree();
    let changes = PhoneFileChanges { updated: vec![item("a", "", "moved.txt")], removed: vec![], generation: 2 };
    assert_eq!(
        inodes.notes_for(&changes),
        vec![
            Note::Inode { ino: a },
            Note::Delete { parent: d, child: a, name: "a.txt".to_owned() },
            Note::Inode { ino: d },
            Note::Entry { parent: ROOT, name: "moved.txt".to_owned() },
            Note::Inode { ino: ROOT },
        ]
    );
    assert_eq!(inodes.entries.get(&a), None);
}

#[test]
fn a_removed_item_is_a_deletion_where_the_kernel_knew_it() {
    let (mut inodes, d, a, _) = known_tree();
    let changes = PhoneFileChanges {
        updated: vec![],
        removed: vec!["a".to_owned(), "b".to_owned(), "x".to_owned()],
        generation: 3,
    };
    // `b` was only listed, so the kernel holds no entry of it; `x` was never seen.
    assert_eq!(
        inodes.notes_for(&changes),
        vec![Note::Delete { parent: d, child: a, name: "a.txt".to_owned() }, Note::Inode { ino: d }]
    );
}

#[test]
fn a_removed_folder_goes_after_what_it_held() {
    let (mut inodes, d, a, _) = known_tree();
    let changes = PhoneFileChanges { updated: vec![], removed: vec!["d".to_owned(), "a".to_owned()], generation: 3 };
    assert_eq!(
        inodes.notes_for(&changes),
        vec![
            Note::Delete { parent: d, child: a, name: "a.txt".to_owned() },
            Note::Inode { ino: d },
            Note::Delete { parent: ROOT, child: d, name: "Docs".to_owned() },
            Note::Inode { ino: ROOT },
        ]
    );
}

#[test]
fn a_moved_folder_drops_its_old_entry_without_a_deletion() {
    let (mut inodes, d, ..) = known_tree();
    let mut folder = item("d", "", "Documents");
    folder.is_directory = true;
    let changes = PhoneFileChanges { updated: vec![folder], removed: vec![], generation: 2 };
    assert_eq!(
        inodes.notes_for(&changes),
        vec![
            Note::Inode { ino: d },
            Note::Entry { parent: ROOT, name: "Docs".to_owned() },
            Note::Inode { ino: ROOT },
            Note::Entry { parent: ROOT, name: "Documents".to_owned() },
        ]
    );
}

#[test]
fn a_new_item_in_an_unknown_folder_needs_nothing() {
    let (mut inodes, ..) = known_tree();
    let changes = PhoneFileChanges { updated: vec![item("n", "elsewhere", "new.txt")], removed: vec![], generation: 2 };
    assert!(inodes.notes_for(&changes).is_empty());
}

#[test]
fn unknown_changes_drop_every_entry_and_inode() {
    let (inodes, d, a, b) = known_tree();
    assert_eq!(
        inodes.notes_for_all(),
        vec![
            Note::Entry { parent: ROOT, name: "Docs".to_owned() },
            Note::Entry { parent: d, name: "a.txt".to_owned() },
            Note::Inode { ino: ROOT },
            Note::Inode { ino: d },
            Note::Inode { ino: a },
            Note::Inode { ino: b },
        ]
    );
}

#[test]
fn the_staging_folder_is_private() {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let base = std::env::temp_dir().join(format!("phonelink-test-{}-{nanos}", std::process::id()));
    let dir = base.join("phonelink").join("uploads");
    let mode = |path: &Path| std::fs::metadata(path).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);

    private_dir(&dir).unwrap();
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&base.join("phonelink")), 0o700);

    std::fs::set_permissions(&dir, Permissions::from_mode(0o755)).unwrap();
    private_dir(&dir).unwrap();
    assert_eq!(mode(&dir), 0o700);

    std::fs::remove_dir_all(&base).unwrap();
}
