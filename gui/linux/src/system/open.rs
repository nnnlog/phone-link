//! Opening a received file in its default application, and showing it in the file manager.

use std::thread;

use tracing::{info, warn};
use zbus::blocking::{Connection, Proxy};

/// Opens a received file in its default application, off the GUI thread.
pub fn open_path(path: &str) {
    let path = path.to_owned();
    thread::spawn(move || {
        if let Err(e) = std::process::Command::new("xdg-open").arg(&path).status() {
            warn!(%path, error = %e, "a file or folder was not opened");
        }
    });
}

/// Shows a received file in the file manager (`org.freedesktop.FileManager1`), or opens its folder
/// when no file manager offers that, off the GUI thread.
pub fn reveal_path(path: &str) {
    let path = std::path::PathBuf::from(path);
    thread::spawn(move || {
        let url = crate::images::file_url(&path);
        let shown = Connection::session().and_then(|bus| {
            Proxy::new(
                &bus,
                "org.freedesktop.FileManager1",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1",
            )?
            .call::<_, _, ()>("ShowItems", &(vec![url.as_str()], ""))
        });
        if let Err(e) = shown {
            info!(path = %path.display(), error = %e, "no file manager showed the file; opening its folder");
            let folder = path.parent().map_or_else(|| path.clone(), std::path::Path::to_path_buf);
            if let Err(e) = std::process::Command::new("xdg-open").arg(&folder).status() {
                warn!(folder = %folder.display(), error = %e, "the file's folder was not opened");
            }
        }
    });
}
