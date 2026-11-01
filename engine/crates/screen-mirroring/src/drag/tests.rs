use super::*;
use Phase::*;
use PhoneDragEvent as E;

#[test]
fn the_table() {
    assert_eq!(next(Ready, E::StartPhone), Next::To(WatchingPointer));
    assert_eq!(next(Ready, E::Complete), Next::Completed);
    assert_eq!(next(Ready, E::PhoneCancel), Next::Cancelled);
    assert_eq!(next(WatchingPointer, E::OutOfBounds), Next::To(RequestingOwnership));
    assert_eq!(next(WatchingPointer, E::UserCancelled), Next::Cancelled);
    assert_eq!(next(RequestingOwnership, E::ClipData), Next::To(DragStarted));
    assert_eq!(next(DragStarted, E::Dropped), Next::To(Drop));
    assert_eq!(next(DragStarted, E::NoItems), Next::Cancelled);
    assert_eq!(next(DragStarted, E::NoDropTarget), Next::CancelBroadcast);
    assert_eq!(next(DragStarted, E::Failure), Next::CancelBroadcast);
    assert_eq!(next(Drop, E::AllWritten), Next::Completed);
    assert_eq!(next(Drop, E::Complete), Next::Completed);
    assert_eq!(next(Drop, E::UserCancelled), Next::To(CancelWait));
    assert_eq!(next(Drop, E::Dropped), Next::To(Drop));
    assert_eq!(next(CancelWait, E::UserCancelled), Next::Cancelled);
}

#[test]
fn pairs_outside_the_table_broadcast() {
    assert_eq!(next(Ready, E::ClipData), Next::CancelBroadcast);
    assert_eq!(next(RequestingOwnership, E::OutOfBounds), Next::CancelBroadcast);
    assert_eq!(next(RequestingOwnership, E::Complete), Next::CancelBroadcast);
    assert_eq!(next(DragStarted, E::Complete), Next::CancelBroadcast);
    assert_eq!(next(CancelWait, E::PhoneCancel), Next::CancelBroadcast);
    assert_eq!(next(WatchingPointer, E::StartPhone), Next::CancelBroadcast);
}

#[test]
fn notification_texts() {
    assert_eq!(progress_title(1, 0), "Transferring 1 file");
    assert_eq!(progress_title(3, 1), "Transferring 2 of 3 files");
    assert_eq!(completion_text(1, 1).0, "Successfully transferred 1 file");
    assert_eq!(
        completion_text(1, 0),
        ("We ran into an issue transferring this file".into(), "Please try again".into())
    );
    assert_eq!(completion_text(3, 3).0, "Successfully transferred 3 files");
    assert_eq!(
        completion_text(3, 1),
        ("Successfully transferred 1 of 3 files".into(), "Failed to transfer 2 of 3 files".into())
    );
    assert_eq!(completion_text(3, 0).0, "We ran into an issue transferring files");

    assert_eq!(size_text(512, 1000), "512.00/1000.00 B");
    assert_eq!(size_text(1_048_576, 3 * 1_048_576), "1.00/3.00 MB");
}

#[test]
fn names() {
    assert_eq!(mac_file_name("IMG_1.jpg"), "IMG_1.jpg");
    assert_eq!(mac_file_name("a/b:c"), "a_b_c");
    assert_eq!(mac_file_name(""), "_");
    assert_eq!(mac_file_name(".."), "_");
    assert_eq!(mac_file_name("."), "_");
}
