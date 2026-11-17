//! The files part's `query` and `perform` names, routed to what answers or runs them.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::CxxQtType;
use phonelink_engine::api::engine::Engine;
use serde_json::{Value, json};
use tracing::warn;

use super::receiving::open_received_folder;
use super::settings_view::{file_settings, phone_files};
use super::transfer_view::transfers;
use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::translated;
use crate::{images, system};

impl EngineBridge {
    pub(in crate::engine_bridge) fn query_files(&self, engine: &Arc<Engine>, name: &str, _arg: &str) -> Option<Value> {
        let value = match name {
            "transfers" => transfers(engine, &self.areas.files),
            "phoneFiles" => phone_files(engine, &self.areas.files, self.phone_mount.is_some()),
            "files.settings" => file_settings(engine),
            _ => return None,
        };
        Some(value)
    }

    pub(in crate::engine_bridge) fn perform_files(
        mut self: Pin<&mut Self>,
        engine: Arc<Engine>,
        name: &str,
        arg: &str,
    ) -> bool {
        let flag = arg == "true";
        let setting = |e| Failure::setting(&e);
        let done: Result<(), Failure> = match name {
            "files.showSendWindow" => {
                self.as_mut().emit_area_event("files", "showSendWindow", &json!({}));
                if !arg.is_empty() {
                    self.as_mut().send_from_window(engine, arg);
                }
                Ok(())
            }
            "files.sendFromWindow" => {
                self.as_mut().send_from_window(engine, arg);
                Ok(())
            }
            "files.send" => {
                self.as_mut().send_from_outside(engine, arg);
                Ok(())
            }
            "files.cancel" => {
                if let Err(e) = engine.file_transfer().cancel_send() {
                    warn!(error = %e, "the file send was not cancelled");
                }
                Ok(())
            }
            "files.requestPermission" => {
                self.as_mut().request_send_permission(engine);
                Ok(())
            }
            "files.shareEnabled" => engine.file_transfer().set_share_enabled(flag).map_err(setting),
            "files.setFolder" => match images::local_path(arg) {
                Some(folder) => engine
                    .file_transfer()
                    .set_received_files_folder(folder)
                    .map_err(|e| Failure::feature("Couldn't Change the Folder", &e)),
                None => {
                    warn!(%arg, "files.setFolder takes a file URL");
                    Ok(())
                }
            },
            "files.openFolder" => {
                open_received_folder(&engine);
                Ok(())
            }
            "phoneFiles.enable" => engine.clone().phone_files().set_enabled(flag).map_err(setting),
            "phoneFiles.requestPermission" => {
                self.as_mut().request_files_permission(engine);
                Ok(())
            }
            "phoneFiles.dismissRecycleNotice" => {
                self.as_mut().rust_mut().areas.files.recycle_notice = false;
                if arg == "never"
                    && let Err(e) = engine.clone().phone_files().set_recycle_notice_off()
                {
                    warn!(error = %e, "the recycle-bin notice was not turned off; it shows again");
                }
                Ok(())
            }
            "phoneFiles.open" => {
                if self.phone_mount.is_some() {
                    system::open_path(&crate::phone_files::mount_point().to_string_lossy());
                    Ok(())
                } else {
                    Err(Failure::new(
                        "Couldn't Open the Folder",
                        translated("Your phone's files aren't in the file manager yet."),
                    ))
                }
            }
            _ => return false,
        };

        if let Err(failure) = done {
            self.as_mut().show_failure(failure);
        }
        if name.starts_with("phoneFiles.") || matches!(name, "files.shareEnabled" | "files.setFolder") {
            self.bump(Revision::Settings);
        }
        true
    }
}
