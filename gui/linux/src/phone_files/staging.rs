//! New files before their upload: the private folder they are staged in, and the staging file that
//! holds a created file's content until a close sends it to the phone.

use std::ffi::OsString;
use std::fs::{DirBuilder, File, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use fuser::TimeOrNow;

/// The folder new files are staged in until their upload: `$XDG_CACHE_HOME/phonelink/uploads`, or
/// `$HOME/.cache/phonelink/uploads` when `XDG_CACHE_HOME` is unset, empty or relative (the XDG base
/// directory rules ignore a relative path). `None` when `HOME` is not an absolute path either.
pub(super) fn staging_dir(cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let absolute = |value: Option<OsString>| value.map(PathBuf::from).filter(|path| path.is_absolute());
    let cache = absolute(cache_home).or_else(|| absolute(home).map(|home| home.join(".cache")))?;
    Some(cache.join("phonelink").join("uploads"))
}

/// Makes `dir` (and its missing parents) with mode `0700`, and narrows an existing one to `0700`: the
/// staged files are the user's content.
pub(super) fn private_dir(dir: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    if std::fs::metadata(dir)?.permissions().mode() & 0o777 != 0o700 {
        std::fs::set_permissions(dir, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Gives a staging file a new size and times. The upload sends the file's own times, so times set here
/// (by a copy that keeps them) reach the phone.
pub(super) fn restage(
    file: &File,
    size: Option<u64>,
    atime: Option<TimeOrNow>,
    mtime: Option<TimeOrNow>,
) -> io::Result<()> {
    if let Some(size) = size {
        file.set_len(size)?;
    }

    let at = |time: TimeOrNow| match time {
        TimeOrNow::SpecificTime(time) => time,
        TimeOrNow::Now => SystemTime::now(),
    };
    let mut times = std::fs::FileTimes::new();
    if let Some(time) = atime {
        times = times.set_accessed(at(time));
    }
    if let Some(time) = mtime {
        times = times.set_modified(at(time));
    }
    if atime.is_some() || mtime.is_some() {
        file.set_times(times)?;
    }
    Ok(())
}

/// A file created on this mount. Its content is written into a staging file and uploaded whole when a
/// descriptor of it is closed: the protocol makes a file in one piece and cannot change it later.
pub(super) struct Created {
    /// The phone identifier of the folder it goes into.
    pub(super) parent: String,
    pub(super) name: String,
    /// The staging file's path. UTF-8, as the engine takes an upload's source as a string.
    pub(super) staging: String,
    pub(super) file: Arc<File>,
    /// Held across each write, size or time change, and the whole upload: the upload reads a complete
    /// copy, and nothing changes the file once it is on the phone. It is the only lock under which
    /// another is taken (the inode table, when the upload ends or the file is unlinked), so the inode
    /// table is never held while waiting for it.
    pub(super) stage: Mutex<Stage>,
}

pub(super) enum Stage {
    /// The content is only in the staging file.
    Staged,
    /// The content is on the phone; the file is read-only like every other phone file.
    Uploaded,
    /// Unlinked before its upload: it never goes to the phone.
    Discarded,
}
