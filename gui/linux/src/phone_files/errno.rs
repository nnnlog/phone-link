//! The `errno` each failure of a phone-files operation reports to the kernel.

use fuser::Errno;
use phonelink_engine::api::phone_files::{PhoneFileError, PhoneFileFailure};

/// The `errno` that reports a failed phone-files operation.
pub(super) fn errno(error: &PhoneFileError) -> Errno {
    let PhoneFileError::Failure { failure, .. } = error;
    match failure {
        PhoneFileFailure::UnknownItem | PhoneFileFailure::FileNotFound => Errno::ENOENT,
        PhoneFileFailure::Exists => Errno::EEXIST,
        PhoneFileFailure::NotSupported | PhoneFileFailure::Ignored => Errno::ENOTSUP,
        PhoneFileFailure::PermissionRequired | PhoneFileFailure::LtwOff | PhoneFileFailure::PcLocked => Errno::EACCES,
        PhoneFileFailure::InProgress => Errno::EBUSY,
        PhoneFileFailure::Cancelled => Errno::EINTR,
        PhoneFileFailure::NotRunning
        | PhoneFileFailure::NotInSync
        | PhoneFileFailure::PhoneUnavailable
        | PhoneFileFailure::Metered
        | PhoneFileFailure::TimedOut
        | PhoneFileFailure::Partial
        | PhoneFileFailure::Failed => Errno::EIO,
    }
}
