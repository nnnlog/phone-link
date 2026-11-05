use super::*;

fn row(id: i64) -> Row {
    Row {
        id,
        uri: format!("content://media/external/file/{id}"),
        name: format!("f{id}"),
        location: format!("storage\\f{id}"),
        file_type: 1,
        size: 1,
        created: None,
        modified: None,
        checksum: 7,
        is_directory: false,
    }
}

#[test]
fn a_batch_item_answered_directory_not_empty_counts_as_deleted_but_stays() {
    let cases = [
        (Status::Success, BatchItem::Deleted),
        (Status::DirectoryNotEmpty, BatchItem::Kept),
        (Status::NotFound, BatchItem::Failed),
        (Status::ChecksumMismatch, BatchItem::Failed),
        (Status::NotSupported, BatchItem::Failed),
        (Status::Other(0), BatchItem::Failed),
        (Status::Other(14), BatchItem::Failed),
    ];

    for (status, want) in cases {
        assert_eq!(batch_item(status), want, "{status:?}");
    }
}

#[test]
fn a_single_delete_answered_directory_not_empty_fails() {
    let cases = [
        (Status::Success, Ok(())),
        (Status::NotFound, Ok(())),
        (Status::DirectoryNotEmpty, Err(OpFailure::Status(Status::DirectoryNotEmpty))),
        (Status::NotSupported, Err(OpFailure::Status(Status::NotSupported))),
        (Status::ChecksumMismatch, Err(OpFailure::Status(Status::ChecksumMismatch))),
    ];

    for (status, want) in cases {
        assert_eq!(single_result(status), want, "{status:?}");
    }
}

#[test]
fn a_withdrawn_item_leaves_the_batch_and_its_delete_ends_not_done() {
    let mut queue = DeleteQueue::default();
    let (tx1, mut rx1) = oneshot::channel();
    let (tx2, mut rx2) = oneshot::channel();
    queue.pending.push(PendingDelete { row: row(1), reply: tx1 });
    queue.pending.push(PendingDelete { row: row(2), reply: tx2 });

    assert!(queue.withdraw("content://media/external/file/1"));
    assert!(!queue.withdraw("content://media/external/file/1"), "it is no longer in the batch");
    assert!(!queue.withdraw("content://media/external/file/9"), "never in the batch");

    let left: Vec<i64> = queue.pending.iter().map(|d| d.row.id).collect();
    assert_eq!(left, vec![2]);
    assert_eq!(rx1.try_recv(), Ok(Err(OpFailure::Status(Status::Cancelled))));
    assert!(rx2.try_recv().is_err(), "the other item still waits for the batch");
}
