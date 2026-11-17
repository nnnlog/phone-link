//! The phone's storage at `~/PhoneLink`: a FUSE file system over the engine's [`PhoneFilesApi`].
//!
//! The phone-files protocol uploads new files and renames, moves and deletes items, but it has no way
//! to replace a file's content. Files already on the phone are therefore read-only: opening one for
//! writing, or changing its size, fails with `EACCES` and leaves the phone untouched. A new file is
//! written into a private staging file and uploaded when a descriptor of it is closed (`flush`), so a
//! failed upload is the error `close()` returns. `rmdir` deletes only empty folders, and trash folders
//! are refused in the root, so file managers offer to delete a file instead of moving it into a hidden
//! folder on the phone. Requests are served on several threads: a download in `open` or an upload in
//! `flush` holds up one thread, not the whole mount.
//!
//! When the phone's tree changes, the engine's changes since the last generation tell the kernel which
//! entries and attributes it holds are stale; an item deleted or moved on the phone is reported as a
//! deletion, which file managers watching the folder show at once. A download or an upload is
//! cancelled when every process waiting for it is being killed (the kernel cannot interrupt a request
//! the mount already serves), and the running ones are cancelled when the mount goes away.
//!
//! [`PhoneFilesApi`]: phonelink_engine::api::phone_files::PhoneFilesApi

mod errno;
mod fs;
mod inodes;
mod link;
mod mount;
mod notes;
mod requesters;
mod requests;
mod staging;
#[cfg(test)]
mod tests;

pub(crate) use link::MountLink;
pub(crate) use mount::{mount, mount_point};
