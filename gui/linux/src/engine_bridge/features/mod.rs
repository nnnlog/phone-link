//! The bridge's shared plumbing: failures shown in the window (`failure`), the revisions views re-read
//! on (`revision`), and the routing of `query` and `perform` names to the part of the app that owns
//! them (`routing`): the `area_*` modules for messages, notifications, calls and contacts, photos and
//! apps, files, media playback, the camera and the shell (settings, account, the phone's status), and
//! `mirroring` for screen mirroring.

mod failure;
mod revision;
mod routing;

pub(super) use failure::{Failure, fail_later};
pub(super) use revision::Revision;
