//! The Files settings as the QML reads them.
//!
//! `files.settings` holds the *Share* switch and the received-files folder, `phoneFiles` the phone's
//! files mounted on this computer.

use std::sync::Arc;

use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_files::PhoneFilesState;
use serde_json::{Value, json};

use super::state::State;
use crate::l10n::{fill, translate, translated};

/// `path` with the home folder written as `~`, as the Files settings show a folder.
pub(super) fn home_relative(path: &str, home: Option<&str>) -> String {
    match home.filter(|home| !home.is_empty()).and_then(|home| path.strip_prefix(home)) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// The files settings: the *Share* switch, and where received files go.
///
/// The folder setting shows once the phone has sent files; before that the folder is only named.
pub(super) fn file_settings(engine: &Arc<Engine>) -> Value {
    let Ok(settings) = engine.clone().file_transfer().settings() else { return Value::Null };
    let home = std::env::var("HOME").ok();
    let folder = settings.received_files_folder.as_deref();
    json!({
        "shareToggleShown": settings.share_toggle_shown,
        "shareEnabled": settings.share_enabled,
        "folderShown": settings.folder_setting_shown && folder.is_some(),
        "folder": folder.map(|folder| home_relative(folder, home.as_deref())),
    })
}

/// The phone's free and total space, as the phone counts gigabytes.
pub(super) fn storage_status(state: &PhoneFilesState) -> Option<String> {
    if !state.running || state.storage_total <= 0 {
        return None;
    }

    let gigabyte = state.gigabyte_bytes.max(1) as f64;
    let free = format!("{:.1}", state.storage_free as f64 / gigabyte);
    let total = format!("{:.1}", state.storage_total as f64 / gigabyte);
    Some(fill(translate("%@ GB free of %@ GB"), &[free, total]))
}

/// The folder the phone keeps deleted files in, by the last part of its path.
pub(super) fn recycle_bin_name(recycle_bin: Option<&str>) -> String {
    match recycle_bin {
        Some(location) => location.rsplit('\\').next().unwrap_or(location).to_owned(),
        None => translated("Recycle bin"),
    }
}

/// The `phoneFiles` query: the phone's files as the Files settings show them.
///
/// It says whether they are offered, allowed and mounted, and shows the phone's storage and its
/// recycle bin.
pub(super) fn phone_files(engine: &Arc<Engine>, state: &State, mounted: bool) -> Value {
    let Ok(files) = engine.clone().phone_files().state() else { return Value::Null };
    let point = crate::phone_files::mount_point().to_string_lossy().into_owned();
    json!({
        "capable": files.capable,
        "enabled": files.enabled,
        "running": files.running,
        "permission": files.permission_granted,
        "needsPermission": files.running && !files.permission_granted,
        "permissionRequested": state.permission_requested,
        "ltwOff": files.ltw_off,
        "mounted": mounted,
        "path": point,
        "pathShown": home_relative(&point, std::env::var("HOME").ok().as_deref()),
        "storage": storage_status(&files),
        "recycleNotice": state.recycle_notice,
        "recycleBin": recycle_bin_name(files.recycle_bin.as_deref()),
    })
}
