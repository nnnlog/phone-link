//! The app's own lifecycle: reconnecting, the outcome of a connect, quitting (and the signals that quit
//! it), the About window's icon, and saving the log.

use std::pin::Pin;
use std::time::Duration;

use cxx_qt::{CxxQtType, Threading};
use phonelink_engine::api::RequestOutcome;
use phonelink_engine::api::logging::export_log;
use phonelink_engine::engine::EngineError;
use serde_json::json;
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, warn};

use crate::engine_bridge::EngineBridge;
use crate::engine_bridge::features::{Failure, Revision};
use crate::{images, system, user_text};

/// How long quitting waits at most for the phone session to end: the phone notices a computer that vanished.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// The app icon the packages install, for the About window; `None` when it is not installed.
pub(super) fn installed_icon() -> Option<String> {
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|dirs| !dirs.is_empty());
    crate::engine_bridge::data_home()
        .ok()
        .into_iter()
        .chain(data_dirs.as_deref().unwrap_or("/usr/local/share:/usr/share").split(':').map(std::path::PathBuf::from))
        .map(|dir| dir.join("icons/hicolor/512x512/apps/phonelink.png"))
        .find(|path| path.is_file())
        .map(|path| images::file_url(&path))
}

impl EngineBridge {
    /// Reconnect: an immediate connect while the engine runs, a start when none ran or the last one
    /// ended, and nothing while a start is under way (it connects when it finishes) or signed out.
    pub(super) fn shell_refresh(self: Pin<&mut Self>) {
        let phase = self.phase.to_string();
        match phase.as_str() {
            "running" => {
                let Some(engine) = self.engine.clone() else { return };
                if let Err(e) = engine.refresh() {
                    warn!(error = %e, "the reconnect did not start");
                    self.failed_with(&user_text::engine_error(&e), &e.to_string());
                }
            }
            "idle" | "failed" | "accountRemoved" => self.start(),
            _ => {}
        }
    }

    /// SIGTERM (the desktop session ending, `systemctl --user stop`, `kill`) and SIGINT (Ctrl+C in a
    /// terminal) quit the way the Quit command does: the phone session stops, for a bounded time, before
    /// the app exits. A second signal exits at once.
    pub(in crate::engine_bridge) fn watch_quit_signals(mut self: Pin<&mut Self>) {
        if self.areas.shell.quit_signals_watched {
            return;
        }
        self.as_mut().rust_mut().areas.shell.quit_signals_watched = true;

        // A signal stream registers with the Tokio runtime it is made in; the Qt thread runs none, so the
        // engine's runtime is entered first.
        let _runtime = self.runtime.enter();
        for kind in [SignalKind::terminate(), SignalKind::interrupt()] {
            let mut signals = match signal(kind) {
                Ok(signals) => signals,
                Err(e) => {
                    warn!(error = %e, "a quit signal is not watched; it ends the app at once");
                    continue;
                }
            };
            let thread = self.qt_thread();
            self.runtime.spawn(async move {
                if signals.recv().await.is_none() {
                    return;
                }
                info!("a quit signal arrived; Phone Link quits");
                let _ = thread.queue(|bridge| bridge.shell_quit());
                if signals.recv().await.is_some() {
                    warn!("a second quit signal arrived; Phone Link exits at once");
                    std::process::exit(1);
                }
            });
        }
    }

    /// Ends the phone session, but never holds up quitting for long; then the shell's QML quits.
    pub(super) fn shell_quit(mut self: Pin<&mut Self>) {
        if self.areas.shell.quitting {
            return;
        }
        self.as_mut().rust_mut().areas.shell.quitting = true;

        let Some(engine) = self.engine.clone() else {
            self.emit_area_event("shell", "quit", &json!({}));
            return;
        };
        let thread = self.qt_thread();
        self.runtime.spawn(async move {
            match tokio::time::timeout(STOP_TIMEOUT, engine.stop()).await {
                Ok(Ok(())) | Ok(Err(EngineError::NotStarted)) => {}
                Ok(Err(e)) => warn!(error = %e, "the phone session did not stop cleanly"),
                Err(_) => info!("the phone session did not stop in time; quitting anyway"),
            }
            let _ = thread.queue(|bridge| bridge.emit_area_event("shell", "quit", &json!({})));
        });
    }

    /// A connect to the phone ended. The failure of one the user asked for (`immediate`) is shown until
    /// a connect succeeds; a background connect's failure is retried without the user, so it is not
    /// news. The shell's QML also hears `connectCompleted`.
    pub(in crate::engine_bridge) fn connect_completed(
        mut self: Pin<&mut Self>,
        immediate: bool,
        outcome: RequestOutcome,
    ) {
        info!(immediate, detail = ?outcome, "a connect to the phone completed");

        let failure = user_text::outcome_failure(&outcome);
        match &failure {
            Some(text) if immediate => self.as_mut().rust_mut().areas.shell.connect_failure = Some(text.clone()),
            Some(_) => {}
            None => self.as_mut().rust_mut().areas.shell.connect_failure = None,
        }

        self.as_mut().emit_area_event(
            "shell",
            "connectCompleted",
            &json!({ "immediate": immediate, "failure": failure }),
        );
        self.bump(Revision::Settings);
    }

    /// Saves a copy of the log at the `file://` URL `url`, then shows it in the file manager.
    pub(super) fn shell_save_log(self: Pin<&mut Self>, url: &str) {
        let Some(path) = images::local_path(url) else {
            warn!("logs.save takes a file URL");
            return;
        };

        let thread = self.qt_thread();
        self.runtime.spawn_blocking(move || {
            let result = export_log(path.clone());
            let _ = thread.queue(move |mut bridge| {
                match result {
                    Ok(()) => system::reveal_path(&path),
                    Err(e) => {
                        warn!(error = %e, "the log was not saved");
                        bridge.as_mut().show_failure(Failure::feature("Phone Link Couldn't Save the Log", &e));
                    }
                }
                bridge.bump(Revision::Settings);
            });
        });
    }
}
