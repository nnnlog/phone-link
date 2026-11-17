//! The phone's files mounted at `~/PhoneLink`.
//!
//! The mount follows the phone's files state; the Files settings ask the phone for access to the files;
//! the engine's phone-files events update both.

use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::engine::Engine;
use phonelink_engine::api::phone_files::{PhoneFilesChange, PhoneFilesPermissionOrigin, PhoneFilesPermissionResult};
use tracing::warn;

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::l10n::translated;

impl EngineBridge {
    /// The Files settings' request for access to the phone's files (the phone prompts).
    pub(super) fn request_files_permission(self: Pin<&mut Self>, engine: Arc<Engine>) {
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            let result = engine.phone_files().request_permission(PhoneFilesPermissionOrigin::StatusUi).await;
            let _ = thread.queue(move |mut bridge| {
                match result {
                    Ok(PhoneFilesPermissionResult::Granted) => {
                        bridge.as_mut().rust_mut().areas.files.permission_requested = false;
                    }
                    Ok(PhoneFilesPermissionResult::Required) => {
                        bridge.as_mut().rust_mut().areas.files.permission_requested = true;
                    }
                    Ok(PhoneFilesPermissionResult::Failed { reason }) => {
                        warn!(%reason, "the phone-files permission was not requested");
                        bridge.as_mut().show_failure(Failure::new(
                            "Couldn't Ask Your Phone",
                            translated("Make sure your phone is nearby and connected, then try again."),
                        ));
                    }
                    Err(e) => {
                        warn!(error = %e, "the phone-files permission was not requested");
                        bridge.as_mut().show_failure(Failure::feature("Couldn't Ask Your Phone", &e));
                    }
                }
                bridge.bump(Revision::Settings);
            });
        });
    }

    /// Mounts the phone's files at `~/PhoneLink`, or unmounts them.
    ///
    /// They are mounted while the phone offers them and the user allowed it. Another phone's files, or
    /// the same phone's from a new engine, replace the mounted ones.
    pub(in crate::engine_bridge) fn sync_phone_mount(mut self: Pin<&mut Self>) {
        let Some(engine) = self.engine.clone() else { return };
        let wanted = engine
            .clone()
            .phone_files()
            .state()
            .ok()
            .filter(|state| state.capable && state.enabled && state.running && state.permission_granted);
        let link = self.areas.files.mount.as_ref();
        let current = match (&wanted, link) {
            (Some(state), Some(link)) => link.serves(&state.domain, &engine),
            (None, None) => true,
            _ => false,
        };
        if current && link.is_some() == self.phone_mount.is_some() {
            return;
        }

        // The running transfers are cancelled before the files are unmounted, so none keeps them busy.
        self.as_mut().rust_mut().areas.files.mount = None;
        self.as_mut().rust_mut().phone_mount = None;
        if let Some(state) = wanted {
            let handle = self.runtime.handle().clone();
            if let Some((session, link)) = crate::phone_files::mount(engine, handle, state.domain) {
                self.as_mut().rust_mut().phone_mount = Some(session);
                self.as_mut().rust_mut().areas.files.mount = Some(link);
            }
        }
    }

    pub(in crate::engine_bridge) fn phone_files_changed(mut self: Pin<&mut Self>, change: PhoneFilesChange) {
        match change {
            PhoneFilesChange::ItemsChanged { domain } => {
                if let Some(link) = &self.areas.files.mount {
                    link.items_changed(&domain);
                }
                return;
            }
            PhoneFilesChange::RecycleBinNotice { .. } => {
                let off = self
                    .engine
                    .clone()
                    .and_then(|engine| engine.phone_files().state().ok())
                    .is_none_or(|s| s.recycle_notice_off);
                if !off {
                    self.as_mut().rust_mut().areas.files.recycle_notice = true;
                }
            }
            PhoneFilesChange::StateChanged | PhoneFilesChange::Running { .. } => {
                self.as_mut().sync_phone_mount();
                let granted = self
                    .engine
                    .clone()
                    .and_then(|engine| engine.phone_files().state().ok())
                    .is_some_and(|s| s.permission_granted);
                if granted {
                    self.as_mut().rust_mut().areas.files.permission_requested = false;
                }
            }
        }
        self.bump(Revision::Settings);
    }
}
