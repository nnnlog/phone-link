//! Open and Open With: a photo's file handed to the application the desktop chooses for it, to the one
//! the user chose, or to the one the user picks in the desktop's chooser.

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use cxx_qt::Threading;
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::photos::PhotoAction;
use tracing::{debug, error, warn};

use super::action_file::{action_file, failure_title, fetch};
use super::files;
use super::work::blocking;
use crate::apps_host;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, fail_later};
use crate::l10n::translate;
use crate::{images, user_text};

/// How a photo opens.
pub(super) enum OpenWith {
    /// The application the desktop chooses for its type.
    Default,
    /// The application of this desktop file ID.
    App(String),
    /// The application the user picks in the desktop's chooser.
    Chooser,
}

impl EngineBridge {
    pub(super) fn open_photo(mut self: Pin<&mut Self>, engine: Arc<Engine>, id: i64, with: OpenWith) {
        let action = if matches!(with, OpenWith::Default) { PhotoAction::Open } else { PhotoAction::OpenWith };
        let file = match action_file(&engine, id, action) {
            Ok(file) => file,
            Err(message) => return self.show_failure(Failure::new(failure_title(action), message)),
        };

        let departures = self.areas.photos.departures.clone();
        let left_before = departures.load(Ordering::SeqCst);
        self.as_mut().mark_busy(&[id], true);
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let opened = async {
                let data = fetch(engine, id, action).await?;
                if departures.load(Ordering::SeqCst) != left_before {
                    debug!(id, "the open of a photo is dropped; the gallery was left meanwhile");
                    return Ok(());
                }

                blocking(move || {
                    let path = files::write_photo(id, &file.file_name, &data)?;
                    let url = images::file_url(&path);
                    match with {
                        OpenWith::Default => files::open_externally(&path.to_string_lossy()).map_err(|e| {
                            warn!(id, error = %e, "a photo was not opened");
                            translate("No app on this computer can open it.").to_owned()
                        }),
                        OpenWith::App(app) => apps_host::open_with(&app, &path, &url).map_err(|e| {
                            warn!(id, error = %e, "a photo was not opened with the chosen application");
                            translate("No app on this computer opened it.").to_owned()
                        }),
                        OpenWith::Chooser => choose_application(&path),
                    }
                })
                .await
            }
            .await;

            if let Err(message) = opened {
                fail_later(&thread, Failure::new(failure_title(action), message));
            }
            let _ = thread.queue(move |bridge| bridge.mark_busy(&[id], false));
        });
    }
}

/// Asks the desktop's application chooser to open `path` (the XDG desktop portal's `OpenURI.OpenFile`
/// with `ask`); the user picks the application there.
fn choose_application(path: &Path) -> Result<(), String> {
    use std::os::fd::AsFd;
    use zbus::zvariant::{Fd, OwnedObjectPath, Value as Variant};

    let not_opened = |e: &dyn std::fmt::Display| {
        warn!(error = %e, "the desktop's application chooser did not open");
        translate("No app on this computer can open it.").to_owned()
    };
    let file = std::fs::File::open(path).map_err(|e| {
        error!(error = %e, "the photo file for the application chooser was not opened");
        user_text::something_went_wrong()
    })?;

    let bus = zbus::blocking::Connection::session().map_err(|e| not_opened(&e))?;
    let portal = zbus::blocking::Proxy::new(
        &bus,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.OpenURI",
    )
    .map_err(|e| not_opened(&e))?;

    let options: HashMap<&str, Variant<'_>> = HashMap::from([("ask", Variant::from(true))]);
    portal
        .call::<_, _, OwnedObjectPath>("OpenFile", &("", Fd::from(file.as_fd()), options))
        .map(drop)
        .map_err(|e| not_opened(&e))
}
